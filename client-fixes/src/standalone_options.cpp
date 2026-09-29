#include "standalone_options.hpp"

#include <windows.h>
#include <bcrypt.h>
#include <tlhelp32.h>
#include <atomic>
#include <cstring>
#include <limits>
#include <vector>

namespace standalone_options {
bool MatchesOriginal(const unsigned char* time, const unsigned char* ai) {
    return time && ai &&
        std::memcmp(time, kTimeOriginal.data(), kTimeOriginal.size()) == 0 &&
        std::memcmp(ai, kAiOriginal.data(), kAiOriginal.size()) == 0;
}

bool RelativeBranch(std::uintptr_t instruction, std::uintptr_t target,
                    unsigned char opcode, std::array<unsigned char, 5>& bytes) {
    if (opcode != 0xe8 && opcode != 0xe9) return false;
    if (instruction > std::numeric_limits<std::uintptr_t>::max() - 5) return false;
    const auto next = instruction + 5;
    // Subtract in the appropriate direction without overflowing signed pointer arithmetic.
    const bool forward = target >= next;
    const auto distance = forward ? target - next : next - target;
    if (distance > (forward ? 0x7fffffffULL : 0x80000000ULL)) return false;
    const auto displacement = static_cast<std::uint32_t>(forward ? distance : 0ULL - distance);
    bytes[0] = opcode;
    for (int i = 0; i != 4; ++i) bytes[i + 1] = static_cast<unsigned char>(displacement >> (8 * i));
    return true;
}

namespace {
bool ReadBytes(const void* address, void* output, std::size_t size) {
    SIZE_T count = 0;
    return ReadProcessMemory(GetCurrentProcess(), address, output, size, &count) && count == size;
}

struct PausedThreads {
    std::vector<HANDLE> handles;
    ~PausedThreads() {
        for (auto thread : handles) { ResumeThread(thread); CloseHandle(thread); }
    }
    bool Pause(std::uintptr_t first, std::uintptr_t last) {
        HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if (snapshot == INVALID_HANDLE_VALUE) return false;
        THREADENTRY32 entry{};
        entry.dwSize = sizeof(entry);
        bool okay = Thread32First(snapshot, &entry) != FALSE;
        if (okay) {
            std::size_t count = 0;
            do {
                if (entry.th32OwnerProcessID == GetCurrentProcessId() &&
                    entry.th32ThreadID != GetCurrentThreadId()) ++count;
            } while (Thread32Next(snapshot, &entry));
            if (GetLastError() != ERROR_NO_MORE_FILES) okay = false;
            else {
                handles.reserve(count); // No heap allocation after the first thread is suspended.
                entry.dwSize = sizeof(entry);
                okay = Thread32First(snapshot, &entry) != FALSE;
            }
        }
        while (okay) {
            if (entry.th32OwnerProcessID == GetCurrentProcessId() &&
                entry.th32ThreadID != GetCurrentThreadId()) {
                HANDLE thread = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT |
                                           THREAD_QUERY_INFORMATION, FALSE, entry.th32ThreadID);
                if (!thread) { okay = false; break; }
                if (SuspendThread(thread) == static_cast<DWORD>(-1)) {
                    CloseHandle(thread); okay = false; break;
                }
                handles.push_back(thread);
                CONTEXT context{};
                context.ContextFlags = CONTEXT_CONTROL;
                if (!GetThreadContext(thread, &context) ||
                    (context.Rip >= first && context.Rip < last)) { okay = false; break; }
            }
            if (!Thread32Next(snapshot, &entry)) {
                okay = GetLastError() == ERROR_NO_MORE_FILES;
                break;
            }
        }
        CloseHandle(snapshot);
        return okay;
    }
};
}

InstallResult Install(std::uintptr_t base) {
    // Only the default-options branch uses these immediates. The native JSON,
    // inherited-options branch, identity appends and OpenLevel call stay intact.
    auto* time = reinterpret_cast<unsigned char*>(base + 0x1ce4989);
    auto* ai = reinterpret_cast<unsigned char*>(base + 0x1ce4a29);
    constexpr std::array<unsigned char, 5> openLevelCall{0xe8, 0x96, 0x6e, 0x57, 0x02};
    std::array<unsigned char, 5> actualCall{};
    std::array<unsigned char, 6> beforeTime{}, beforeAi{};
    if (!ReadBytes(reinterpret_cast<void*>(base + 0x1ce4d95), actualCall.data(), actualCall.size()) ||
        actualCall != openLevelCall ||
        !ReadBytes(time, beforeTime.data(), beforeTime.size()) ||
        !ReadBytes(ai, beforeAi.data(), beforeAi.size()) ||
        !MatchesOriginal(beforeTime.data(), beforeAi.data())) return InstallResult::InstructionMismatch;

    PausedThreads paused;
    if (!paused.Pause(reinterpret_cast<std::uintptr_t>(time),
                      reinterpret_cast<std::uintptr_t>(ai) + kAiOriginal.size()) ||
        !ReadBytes(time, beforeTime.data(), beforeTime.size()) ||
        !ReadBytes(ai, beforeAi.data(), beforeAi.size()) ||
        !MatchesOriginal(beforeTime.data(), beforeAi.data())) return InstallResult::ThreadPauseFailed;
    DWORD timeProtection = 0, aiProtection = 0;
    if (!VirtualProtect(time, kTimeOriginal.size(), PAGE_EXECUTE_READWRITE, &timeProtection)) return InstallResult::ProtectionFailed;
    if (!VirtualProtect(ai, kAiOriginal.size(), PAGE_EXECUTE_READWRITE, &aiProtection)) {
        DWORD ignored = 0;
        return VirtualProtect(time, kTimeOriginal.size(), timeProtection, &ignored)
            ? InstallResult::ProtectionFailed : InstallResult::StateUncertain;
    }
    std::memcpy(time, kTimeConfigured.data(), kTimeConfigured.size());
    std::memcpy(ai, kAiConfigured.data(), kAiConfigured.size());
    const bool flushed = FlushInstructionCache(GetCurrentProcess(), time, kTimeConfigured.size()) &&
                         FlushInstructionCache(GetCurrentProcess(), ai, kAiConfigured.size());
    bool rollbackFlushed = true;
    if (!flushed) {
        std::memcpy(time, kTimeOriginal.data(), kTimeOriginal.size());
        std::memcpy(ai, kAiOriginal.data(), kAiOriginal.size());
        const bool timeFlushed = FlushInstructionCache(GetCurrentProcess(), time, kTimeOriginal.size()) != FALSE;
        const bool aiFlushed = FlushInstructionCache(GetCurrentProcess(), ai, kAiOriginal.size()) != FALSE;
        rollbackFlushed = timeFlushed && aiFlushed;
    }
    DWORD ignored = 0;
    const bool aiRestored = VirtualProtect(ai, kAiOriginal.size(), aiProtection, &ignored) != FALSE;
    const bool timeRestored = VirtualProtect(time, kTimeOriginal.size(), timeProtection, &ignored) != FALSE;
    if (!aiRestored || !timeRestored || !rollbackFlushed) return InstallResult::StateUncertain;
    return flushed ? InstallResult::Installed : InstallResult::CacheFlushFailed;
}

RateResult InstallRate(std::uintptr_t base) {
    constexpr std::array<unsigned char, 8> load{0xf2, 0x0f, 0x10, 0x15, 0x57, 0x65, 0xd0, 0x03};
    auto* instruction = reinterpret_cast<unsigned char*>(base + 0x1ce48c9);
    auto* rate = reinterpret_cast<unsigned char*>(base + 0x59eae28);
    auto valid = [&]() {
        std::array<unsigned char, 8> actualInstruction{}, actualRate{};
        return ReadBytes(instruction, actualInstruction.data(), actualInstruction.size()) &&
               ReadBytes(rate, actualRate.data(), actualRate.size()) &&
               actualInstruction == load && actualRate == kRateOriginal;
    };
    if (!valid()) return RateResult::InstructionMismatch;
    PausedThreads paused;
    if (!paused.Pause(reinterpret_cast<std::uintptr_t>(rate),
                      reinterpret_cast<std::uintptr_t>(rate) + kRateOriginal.size()) || !valid())
        return RateResult::ThreadPauseFailed;
    DWORD protection = 0;
    if (!VirtualProtect(rate, kRateOriginal.size(), PAGE_READWRITE, &protection))
        return RateResult::ProtectionFailed;
    std::memcpy(rate, kRateConfigured.data(), kRateConfigured.size());
    if (!FlushInstructionCache(GetCurrentProcess(), rate, kRateConfigured.size())) {
        std::memcpy(rate, kRateOriginal.data(), kRateOriginal.size());
        const bool rolledBack = FlushInstructionCache(GetCurrentProcess(), rate, kRateOriginal.size()) != FALSE;
        DWORD ignored = 0;
        const bool restored = VirtualProtect(rate, kRateOriginal.size(), protection, &ignored) != FALSE;
        return rolledBack && restored ? RateResult::CacheFlushFailed : RateResult::StateUncertain;
    }
    DWORD ignored = 0;
    return VirtualProtect(rate, kRateOriginal.size(), protection, &ignored)
        ? RateResult::Installed : RateResult::StateUncertain;
}

TimerResult InstallAiTimer(std::uintptr_t base) {
    constexpr std::uintptr_t siteRva = 0x1ca3063;
    constexpr std::uintptr_t schedulerRva = 0x46b7c10;
    constexpr std::size_t thunkSize = 17;
    auto* site = reinterpret_cast<unsigned char*>(base + siteRva);
    std::array<unsigned char, kAiTimerOriginal.size() + 1> original{};
    constexpr std::array<unsigned char, 5> next{0xe8, 0xa8, 0x4b, 0xa1, 0x02};
    auto valid = [&]() {
        return ReadBytes(site - 12, original.data(), original.size()) &&
               std::memcmp(original.data(), kAiTimerOriginal.data(), kAiTimerOriginal.size()) == 0 &&
               original.back() == 0x90;
    };
    if (!valid()) return TimerResult::InstructionMismatch;

    SYSTEM_INFO info{};
    GetSystemInfo(&info);
    if (!info.dwAllocationGranularity) return TimerResult::NoNearMemory;
    const auto granularity = static_cast<std::uintptr_t>(info.dwAllocationGranularity);
    const auto siteAddress = reinterpret_cast<std::uintptr_t>(site);
    if (siteAddress > std::numeric_limits<std::uintptr_t>::max() - 5 ||
        base > std::numeric_limits<std::uintptr_t>::max() - schedulerRva)
        return TimerResult::NoNearMemory;
    const auto nextAddress = siteAddress + 5;
    auto lower = nextAddress >= 0x80000000ULL ? nextAddress - 0x80000000ULL : 0;
    auto upper = nextAddress <= std::numeric_limits<std::uintptr_t>::max() - 0x7fffffffULL
        ? nextAddress + 0x7fffffffULL : std::numeric_limits<std::uintptr_t>::max();
    const auto scheduler = base + schedulerRva;
    const auto schedulerLower = scheduler >= 13 + 0x7fffffffULL
        ? scheduler - 13 - 0x7fffffffULL : 0;
    const auto schedulerUpper = scheduler <= std::numeric_limits<std::uintptr_t>::max() - 0x80000000ULL
        ? scheduler + 0x80000000ULL - 13 : std::numeric_limits<std::uintptr_t>::max();
    if (lower < schedulerLower) lower = schedulerLower;
    if (upper > schedulerUpper) upper = schedulerUpper;
    if (lower > upper) return TimerResult::NoNearMemory;
    unsigned char* thunk = nullptr;
    std::array<unsigned char, 5> branch{}, returnBranch{};
    // Query entire regions, not granularity-sized addresses within the mapped EXE.
    // Prefer free space above the image, then try below the callsite.
    auto findFree = [&](std::uintptr_t first, std::uintptr_t last) {
        for (auto cursor = first; cursor <= last;) {
            MEMORY_BASIC_INFORMATION region{};
            if (!VirtualQuery(reinterpret_cast<void*>(cursor), &region, sizeof(region))) return false;
            const auto start = reinterpret_cast<std::uintptr_t>(region.BaseAddress);
            if (region.RegionSize > std::numeric_limits<std::uintptr_t>::max() - start) return false;
            const auto end = start + region.RegionSize;
            if (end <= cursor) return false;
            if (region.State == MEM_FREE) {
                const auto offset = cursor % granularity;
                const auto padding = offset ? granularity - offset : 0;
                if (cursor <= std::numeric_limits<std::uintptr_t>::max() - padding) {
                    const auto candidate = cursor + padding;
                    if (candidate <= last && candidate <= std::numeric_limits<std::uintptr_t>::max() - 13 &&
                        candidate < end && end - candidate >= info.dwPageSize &&
                        RelativeBranch(siteAddress, candidate, 0xe8, branch) &&
                        RelativeBranch(candidate + 8, scheduler, 0xe9, returnBranch)) {
                        thunk = static_cast<unsigned char*>(VirtualAlloc(reinterpret_cast<void*>(candidate),
                                                                           info.dwPageSize, MEM_RESERVE | MEM_COMMIT,
                                                                           PAGE_READWRITE));
                        if (thunk) return true;
                    }
                }
            }
            cursor = end;
        }
        return false;
    };
    if (siteAddress <= upper) {
        if (!findFree(siteAddress < lower ? lower : siteAddress, upper) && lower < siteAddress)
            findFree(lower, siteAddress - 1);
    } else {
        findFree(lower, upper);
    }
    if (!thunk) return TimerResult::NoNearMemory;

    // movss xmm3, dword ptr [rip+5]; jmp scheduler; literal 0.5f.
    constexpr std::array<unsigned char, 8> load{0xf3, 0x0f, 0x10, 0x1d, 5, 0, 0, 0};
    std::memcpy(thunk, load.data(), load.size());
    std::memcpy(thunk + 8, returnBranch.data(), returnBranch.size());
    std::memcpy(thunk + 13, kAiIntervalConfigured.data(), kAiIntervalConfigured.size());
    DWORD thunkProtection = 0;
    if (!VirtualProtect(thunk, thunkSize, PAGE_EXECUTE_READ, &thunkProtection)) {
        VirtualFree(thunk, 0, MEM_RELEASE);
        return TimerResult::ProtectionFailed;
    }
    if (!FlushInstructionCache(GetCurrentProcess(), thunk, thunkSize)) {
        VirtualFree(thunk, 0, MEM_RELEASE);
        return TimerResult::CacheFlushFailed;
    }

    PausedThreads paused;
    if (!paused.Pause(reinterpret_cast<std::uintptr_t>(site),
                      reinterpret_cast<std::uintptr_t>(site) + next.size()) || !valid()) {
        VirtualFree(thunk, 0, MEM_RELEASE);
        return TimerResult::ThreadPauseFailed;
    }
    DWORD protection = 0;
    if (!VirtualProtect(site, next.size(), PAGE_EXECUTE_READWRITE, &protection)) {
        VirtualFree(thunk, 0, MEM_RELEASE);
        return TimerResult::ProtectionFailed;
    }
    std::memcpy(site, branch.data(), branch.size());
    if (!FlushInstructionCache(GetCurrentProcess(), site, branch.size())) {
        std::memcpy(site, next.data(), next.size());
        const bool restored = FlushInstructionCache(GetCurrentProcess(), site, next.size()) != FALSE;
        DWORD ignored = 0;
        const bool protectedAgain = VirtualProtect(site, next.size(), protection, &ignored) != FALSE;
        // The patched call may have been fetched before rollback; keep its target alive.
        return restored && protectedAgain ? TimerResult::CacheFlushFailed : TimerResult::StateUncertain;
    }
    DWORD ignored = 0;
    if (!VirtualProtect(site, next.size(), protection, &ignored)) {
        std::memcpy(site, next.data(), next.size());
        // Retain the thunk if a stale patched call could still be executed.
        FlushInstructionCache(GetCurrentProcess(), site, next.size());
        return TimerResult::StateUncertain;
    }
    // Keep the RX thunk for the lifetime of the process: calls may enter it later.
    return TimerResult::Installed;
}

bool PickUniformIndex(ByteSource source, std::int32_t first, std::int32_t last, std::int32_t& index) {
    if (!source || last < first) return false;
    const auto span = static_cast<std::uint32_t>(last - first) + 1;
    if (span > 256) return false;
    // Reject the biased tail so every value in the span is equally likely.
    const std::uint32_t limit = 256 - (256 % span);
    for (std::size_t draw = 0; draw != kMaxSelectionDraws; ++draw) {
        unsigned char byte = 0;
        if (!source(&byte, 1)) return false;
        if (byte < limit) {
            index = first + static_cast<std::int32_t>(byte % span);
            return true;
        }
    }
    return false;
}

bool OptionsContainAiCount(const char16_t* text, std::int32_t num) {
    constexpr char16_t needle[] = u"ai_count=";
    constexpr std::int32_t length = kAiCountNeedleChars;
    if (!text || num < length) return false;
    for (std::int32_t start = 0; start + length <= num; ++start) {
        std::int32_t i = 0;
        for (; i != length; ++i) {
            char16_t letter = text[start + i];
            if (letter >= u'A' && letter <= u'Z') letter = static_cast<char16_t>(letter + 32);
            if (letter != needle[i]) break;
        }
        if (i == length) return true;
    }
    return false;
}

namespace {
using InitTableSetting = std::uintptr_t(__fastcall*)(void*, std::int32_t);
std::atomic<InitTableSetting> gOriginalInit{nullptr};
std::atomic<ZoneRowFunction> gZoneRow{nullptr};

bool RandomBytes(unsigned char* bytes, std::size_t count) {
    return BCRYPT_SUCCESS(BCryptGenRandom(nullptr, bytes, static_cast<ULONG>(count),
                                          BCRYPT_USE_SYSTEM_PREFERRED_RNG));
}

// Guarded reads only; any failure means "not a bot match" and the game default runs.
bool IsBotMatch(std::uintptr_t gameMode) {
    std::uintptr_t data = 0;
    std::int32_t num = 0;
    if (!ReadBytes(reinterpret_cast<void*>(gameMode + kOptionsStringOffset), &data, sizeof(data)) ||
        !ReadBytes(reinterpret_cast<void*>(gameMode + kOptionsNumOffset), &num, sizeof(num)) ||
        !data || num <= 0) return false;
    const auto total = num < kMaxOptionsChars ? num : kMaxOptionsChars;
    char16_t text[kOptionsChunkChars];
    for (std::int32_t start = 0; start < total;) {
        const auto count = total - start < kOptionsChunkChars ? total - start : kOptionsChunkChars;
        if (!ReadBytes(reinterpret_cast<void*>(data + static_cast<std::uintptr_t>(start) * sizeof(char16_t)),
                       text, static_cast<std::size_t>(count) * sizeof(char16_t))) return false;
        if (OptionsContainAiCount(text, count)) return true;
        if (start + count >= total) break;
        start += count - (kAiCountNeedleChars - 1); // overlap so a boundary match is not missed
    }
    return false;
}

std::uintptr_t __fastcall BlueZoneHook(void* gameMode, std::int32_t index) {
    const auto original = gOriginalInit.load();
    if (!original) return 0; // Unreachable: the slot is patched only after this is set.
    // Only the game's own "random" request in a bot match is replaced; every other
    // call (console SelectBlueZone, normal matches, any failure) runs unchanged.
    std::int32_t selected = 0;
    if (index != kBlueZoneRandomIndex || !gameMode ||
        !IsBotMatch(reinterpret_cast<std::uintptr_t>(gameMode)) ||
        !PickUniformIndex(RandomBytes, kBlueZoneFirst, kBlueZoneLast, selected))
        return original(gameMode, index);
    const auto result = original(gameMode, selected);
    std::uint32_t name[2]{};
    if (const auto report = gZoneRow.load();
        report && ReadBytes(reinterpret_cast<void*>(reinterpret_cast<std::uintptr_t>(gameMode) +
                                                     kChosenRowNameOffset), name, sizeof(name)))
        report(selected, name[0], name[1]);
    return result;
}
}

BlueZoneResult InstallBlueZone(std::uintptr_t base, ZoneRowFunction zoneRow) {
    auto* slot = reinterpret_cast<std::uintptr_t*>(base + kInitTableSettingSlotRva);
    const std::uintptr_t expected = base + kInitTableSettingRva;
    auto valid = [&]() {
        std::uintptr_t actual = 0;
        return ReadBytes(slot, &actual, sizeof(actual)) && actual == expected;
    };
    if (!valid()) return BlueZoneResult::InstructionMismatch;
    gZoneRow.store(zoneRow);
    gOriginalInit.store(reinterpret_cast<InitTableSetting>(expected));
    const auto hook = reinterpret_cast<std::uintptr_t>(&BlueZoneHook);

    PausedThreads paused;
    if (!paused.Pause(reinterpret_cast<std::uintptr_t>(slot),
                      reinterpret_cast<std::uintptr_t>(slot) + sizeof(*slot)) || !valid())
        return BlueZoneResult::ThreadPauseFailed;
    DWORD protection = 0;
    if (!VirtualProtect(slot, sizeof(*slot), PAGE_READWRITE, &protection))
        return BlueZoneResult::ProtectionFailed;
    std::memcpy(slot, &hook, sizeof(hook));
    std::uintptr_t written = 0;
    bool verified = ReadBytes(slot, &written, sizeof(written)) && written == hook;
    if (!verified) std::memcpy(slot, &expected, sizeof(expected));
    DWORD ignored = 0;
    const bool restored = VirtualProtect(slot, sizeof(*slot), protection, &ignored) != FALSE;
    if (!restored) return BlueZoneResult::StateUncertain;
    if (!verified) {
        std::uintptr_t actual = 0;
        return ReadBytes(slot, &actual, sizeof(actual)) && actual == expected
            ? BlueZoneResult::ProtectionFailed : BlueZoneResult::StateUncertain;
    }
    return BlueZoneResult::Installed;
}
}
