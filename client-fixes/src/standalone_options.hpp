#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

namespace standalone_options {
// Exact StartStandalonePlay default-option mov r8d, imm32 instructions.
constexpr std::array<unsigned char, 6> kTimeOriginal{0x41, 0xb8, 0xb4, 0, 0, 0};
constexpr std::array<unsigned char, 6> kTimeConfigured{0x41, 0xb8, 10, 0, 0, 0};
constexpr std::array<unsigned char, 6> kAiOriginal{0x41, 0xb8, 20, 0, 0, 0};
constexpr std::array<unsigned char, 6> kAiConfigured{0x41, 0xb8, 50, 0, 0, 0};

bool MatchesOriginal(const unsigned char* time, const unsigned char* ai);
enum class InstallResult { Installed, InstructionMismatch, ThreadPauseFailed, ProtectionFailed, CacheFlushFailed, StateUncertain };
InstallResult Install(std::uintptr_t base);

// The standalone option formatter's double constant, not the unrelated .data copy.
constexpr std::array<unsigned char, 8> kRateOriginal{0, 0, 0, 0xa0, 0x99, 0x99, 0xe9, 0x3f};
constexpr std::array<unsigned char, 8> kRateConfigured{0, 0, 0, 0, 0, 0, 0xe0, 0x3f};
enum class RateResult { Installed, InstructionMismatch, ThreadPauseFailed, ProtectionFailed, CacheFlushFailed, StateUncertain };
RateResult InstallRate(std::uintptr_t base);

// Build-specific AddAIPlayerByTimer scheduler callsite; independent of Install.
constexpr std::array<unsigned char, 17> kAiTimerOriginal{
    0x41, 0x0f, 0x28, 0xd8, 0x4c, 0x8d, 0x44, 0x24, 0x60,
    0x48, 0x8b, 0xcb, 0xe8, 0xa8, 0x4b, 0xa1, 0x02};
constexpr std::array<unsigned char, 4> kAiIntervalConfigured{0, 0, 0, 0x3f};
bool RelativeBranch(std::uintptr_t instruction, std::uintptr_t target,
                    unsigned char opcode, std::array<unsigned char, 5>& bytes);
enum class TimerResult { Installed, InstructionMismatch, NoNearMemory, ThreadPauseFailed,
                         ProtectionFailed, CacheFlushFailed, StateUncertain };
TimerResult InstallAiTimer(std::uintptr_t base);

// Blue zone selection for standalone bot matches. InitTableSetting(int32) is a
// GameMode virtual; its vtable slot is replaced (the function body is untouched).
// Match start passes -1 ("random over all rows"); a bot match then picks a
// uniform console index from [kBlueZoneFirst, kBlueZoneLast] instead.
constexpr std::uintptr_t kInitTableSettingRva = 0x1cb7d80;
constexpr std::uintptr_t kInitTableSettingSlotRva = 0x59e8860;
constexpr std::int32_t kBlueZoneFirst = 34;
constexpr std::int32_t kBlueZoneLast = 53;
constexpr std::int32_t kBlueZoneRandomIndex = -1;
constexpr std::uintptr_t kOptionsStringOffset = 0x310; // FString: Data ptr; Num int32 at +8
constexpr std::uintptr_t kOptionsNumOffset = 0x318;
constexpr std::uintptr_t kChosenRowNameOffset = 0x644; // FName: ComparisonIndex, Number
// The travel options begin with ~1.3K characters of party JSON, so ai_count= sits far
// into the string. It is scanned in chunks (overlapping by the needle length).
constexpr std::int32_t kMaxOptionsChars = 65536;        // options past this are not scanned
constexpr std::int32_t kOptionsChunkChars = 512;
constexpr std::int32_t kAiCountNeedleChars = 9;         // length of "ai_count="
constexpr std::size_t kMaxSelectionDraws = 64;

using ByteSource = bool (*)(unsigned char* bytes, std::size_t count);
// Uniform index in [first, last] by rejection sampling one byte at a time.
// Returns false if the source fails or every bounded draw is rejected.
bool PickUniformIndex(ByteSource source, std::int32_t first, std::int32_t last, std::int32_t& index);
// True if the first num UTF-16 characters contain "ai_count=", ASCII case-insensitive.
bool OptionsContainAiCount(const char16_t* text, std::int32_t num);

// Called after the original ran; receives the row FName the game stored.
using ZoneRowFunction = void (*)(std::int32_t selected, std::uint32_t comparisonIndex, std::uint32_t number);
enum class BlueZoneResult { Installed, InstructionMismatch, ThreadPauseFailed, ProtectionFailed, StateUncertain };
BlueZoneResult InstallBlueZone(std::uintptr_t base, ZoneRowFunction zoneRow);
}
