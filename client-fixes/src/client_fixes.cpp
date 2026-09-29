#include <windows.h>
#include <bcrypt.h>

#include "custom_pak_signing.hpp"
#include "standalone_options.hpp"

#include <array>
#include <atomic>
#include <algorithm>
#include <cstdint>
#include <cwchar>
#include <string>
#include <unordered_map>
#include <vector>

// Build-specific client patches for the preserved BravoHotel client.
//
// After build, the launcher embeds this DLL, installs it
// next to the game executable, and loads it into the game process. Run() first
// hashes that executable; none of the offsets below are read before it passes.
//
// After validation, one worker tracks the local world and class eligibility,
// one tracks the merged capsule item table and one tracks First Blood audio.
// Cheat menu translations are supplied by the signed PAK.
// The launcher keeps the DLL loaded until process exit; there is no mid-game
// unload protocol for stopping workers or restoring their temporary writes.
namespace {
// BravoHotel 1.3.0.473797 only. RVAs are relative to the shipping EXE's
// loaded image base, so ASLR changes the final addresses each run.
constexpr wchar_t kSha256[] = L"16b8b421371457d936e5cc1810ff707b5f5984126973dbdc4e6b5c714522051f";
constexpr std::uintptr_t kObjectsRva = 0x762f708;
constexpr std::uintptr_t kTableRva = 0x6b3c0f8;
constexpr std::uint64_t kPointerXor = 0xba146ab1e38e3211;
constexpr LONG kClassLevel = 5;
constexpr std::uintptr_t kNamePoolRva = 0x7603240;
constexpr std::uintptr_t kAnsiNameDecoderRva = 0x2a5dfb0;
constexpr std::uintptr_t kWideNameDecoderRva = 0x2a6a710;
// Run() publishes only controllers that pass Validate(). The First Blood
// worker reads this pointer but still checks the widget's class before use.
std::atomic<std::uintptr_t> gLocalController{0};

// Write diagnostics to the optional DLL console and a debugger, if attached.
void Log(const wchar_t* message) {
    OutputDebugStringW(message);
    HANDLE handle = GetStdHandle(STD_OUTPUT_HANDLE);
    if (handle && handle != INVALID_HANDLE_VALUE) {
        DWORD written = 0;
        WriteConsoleW(handle, message, static_cast<DWORD>(wcslen(message)), &written, nullptr);
    }
}

// These reads use ReadProcessMemory even though the DLL is in the same process.
// A failed read returns false instead of dereferencing a stale Unreal object.
template<class T> bool Read(std::uintptr_t address, T& value) {
    SIZE_T count = 0;
    return address && ReadProcessMemory(GetCurrentProcess(), reinterpret_cast<void*>(address),
                                        &value, sizeof(T), &count) && count == sizeof(T);
}
// Read an entire GObjects chunk or pointer substitution table.
bool ReadBlock(std::uintptr_t address, void* data, SIZE_T size) {
    SIZE_T count = 0;
    return address && ReadProcessMemory(GetCurrentProcess(), reinterpret_cast<void*>(address),
                                        data, size, &count) && count == size;
}
// Change an aligned 32-bit field only if it still has the expected value. This
// is used for both UserLevel and capsule FName indices. The compare/exchange
// avoids overwriting a game update between our read and write; SEH handles an
// object disappearing at the write site.
bool CompareDword(std::uintptr_t address, LONG before, LONG after) {
    __try {
        return InterlockedCompareExchange(reinterpret_cast<volatile LONG*>(address),
                                          after, before) == before;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

// Hash the actual EXE backing this process, not a configured install path.
// All fixes fail closed if the file differs from the researched build.
bool SupportedBuild() {
    std::vector<wchar_t> path(32768);
    DWORD length = GetModuleFileNameW(nullptr, path.data(), static_cast<DWORD>(path.size()));
    if (!length || length >= path.size()) return false;
    HANDLE file = CreateFileW(path.data(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE,
                              nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return false;
    BCRYPT_ALG_HANDLE algorithm = nullptr;
    BCRYPT_HASH_HANDLE hash = nullptr;
    DWORD objectSize = 0, returned = 0;
    std::array<UCHAR, 32> digest{};
    bool okay = false;
    if (BCRYPT_SUCCESS(BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0)) &&
        BCRYPT_SUCCESS(BCryptGetProperty(algorithm, BCRYPT_OBJECT_LENGTH,
                       reinterpret_cast<PUCHAR>(&objectSize), sizeof(objectSize), &returned, 0)) &&
        objectSize && objectSize < 4096) {
        std::vector<UCHAR> hashStorage(objectSize);
        if (BCRYPT_SUCCESS(BCryptCreateHash(algorithm, &hash, hashStorage.data(),
                                           objectSize, nullptr, 0, 0))) {
            std::array<UCHAR, 65536> buffer{};
            bool complete = true;
            for (;;) {
                DWORD got = 0;
                if (!ReadFile(file, buffer.data(), static_cast<DWORD>(buffer.size()), &got, nullptr)) {
                    complete = false; break;
                }
                if (!got) break;
                if (!BCRYPT_SUCCESS(BCryptHashData(hash, buffer.data(), got, 0))) {
                    complete = false; break;
                }
            }
            if (complete && BCRYPT_SUCCESS(BCryptFinishHash(hash, digest.data(),
                                                             static_cast<ULONG>(digest.size()), 0))) {
                constexpr wchar_t hex[] = L"0123456789abcdef";
                std::array<wchar_t, 65> actual{};
                for (size_t i = 0; i < digest.size(); ++i) {
                    actual[2*i] = hex[digest[i] >> 4];
                    actual[2*i+1] = hex[digest[i] & 15];
                }
                okay = wcscmp(actual.data(), kSha256) == 0;
            }
        }
    }
    if (hash) BCryptDestroyHash(hash);
    if (algorithm) BCryptCloseAlgorithmProvider(algorithm, 0);
    CloseHandle(file);
    return okay;
}

// The preserved build stores UObject pointers in 40-byte GObjects items,
// grouped into chunks of up to 65,536. The header below is the live array
// layout recovered by Preservation/tools/dump_objects.py.
struct ObjectArray {
    std::uintptr_t chunks, preallocated;
    std::int32_t maximum, count, maxChunks, numChunks;
};
static_assert(sizeof(ObjectArray) == 32);

// Cache the active controller and the objects whose identities must remain
// stable while the temporary class-level change is active.
struct LocalPlayer {
    std::uintptr_t controller = 0, world = 0, info = 0;
};

// These offsets are Engine.World fields in the preserved SDK. A non-null
// AuthorityGameMode with no NetDriver or DemoNetDriver was observed in local
// play; requiring all three excludes online and replay worlds here.
bool Standalone(std::uintptr_t world) {
    std::uintptr_t net = 0, demo = 0, mode = 0, level = 0;
    return Read(world+88, net) && !net && Read(world+304, demo) && !demo &&
           Read(world+464, mode) && mode && Read(world+80, level) && level;
}

// A transition may invalidate the old player objects. Restore UserLevel only
// while the old controller still points to the same info component and its
// world has not acquired a network driver.
bool SafeToRestore(const LocalPlayer& active) {
    std::uintptr_t net=0, demo=0, state=0, info=0;
    return active.world && active.controller && active.info &&
           Read(active.world+88, net) && !net &&
           Read(active.world+304, demo) && !demo &&
           Read(active.controller+912, state) && state &&
           Read(state+1496, info) && info == active.info;
}

// Resolve the local player's BHReplicatedPlayerInfo without relying on an
// object's address from a previous run. The forward/back pointers and the
// GameInstance.LocalPlayers array exclude AI controllers and unrelated UObjects.
// PersistentLevel and ViewportClient.World must agree so a stale world fails.
bool Validate(std::uintptr_t controller, LocalPlayer& result) {
    std::uintptr_t player=0, backlink=0, viewport=0, level=0, world=0;
    std::uintptr_t persistent=0, viewportWorld=0, instance=0, viewportInstance=0;
    std::uintptr_t entries=0, state=0, info=0, connection=0;
    std::int32_t count=0;
    // Key SDK fields: Controller.PlayerState +912, PlayerController.Player
    // +1608, Player.PlayerController +56, LocalPlayer.ViewportClient +120,
    // Level.OwningWorld +720, World.OwningGameInstance +560, and
    // GameInstance.LocalPlayers +192. PlayerState.ReplicatedPlayerInfo is +1496.
    if (!Read(controller+1608, player) || !player ||
        !Read(player+56, backlink) || backlink != controller ||
        !Read(player+120, viewport) || !viewport ||
        !Read(controller+40, level) || !level ||
        !Read(level+720, world) || !world ||
        !Read(world+80, persistent) || persistent != level ||
        !Read(viewport+128, viewportWorld) || viewportWorld != world ||
        !Read(world+560, instance) || !instance ||
        !Read(viewport+136, viewportInstance) || viewportInstance != instance ||
        !Read(instance+192, entries) || !entries ||
        !Read(instance+200, count) || count < 1 || count > 4 ||
        !Read(controller+1368, connection) || connection ||
        !Standalone(world)) return false;
    bool inLocalPlayers = false;
    for (std::int32_t i=0; i<count; ++i) {
        std::uintptr_t entry=0;
        if (!Read(entries+i*8, entry)) return false;
        inLocalPlayers |= entry == player;
    }
    if (!inLocalPlayers || !Read(controller+912, state) || !state ||
        !Read(state+1496, info) || !info) return false;
    result = {controller, world, info};
    return true;
}

// This build substitutes every byte of an encoded GObjects pointer, then XORs
// the result. Validate that the 256-byte table is a permutation before using it.
bool PointerTable(std::uintptr_t base, std::array<UCHAR,256>& table) {
    std::uintptr_t address=0;
    if (!Read(base+kTableRva, address) || !address ||
        !ReadBlock(address+0x100, table.data(), table.size())) return false;
    std::array<bool,256> seen{};
    for (UCHAR byte : table) {
        if (seen[byte]) return false;
        seen[byte] = true;
    }
    return true;
}

// Decode one obfuscated UObject pointer from a GObjects item.
std::uintptr_t Decode(const UCHAR* bytes, const std::array<UCHAR,256>& table) {
    std::uint64_t value=0;
    for (int i=0; i<8; ++i) value |= std::uint64_t(table[bytes[i]]) << (8*i);
    return static_cast<std::uintptr_t>(value ^ kPointerXor);
}

// Search GObjects for the controller that passes every local-world relationship
// check. First verify the object's internal index to reject freed/reused slots.
// Run() keeps the result and only rescans when that controller stops validating.
LocalPlayer FindLocalPlayer(std::uintptr_t base) {
    ObjectArray objects{};
    if (!Read(base+kObjectsRva, objects) || objects.count <= 0 ||
        objects.count > objects.maximum || objects.maximum > 0x1000000 ||
        objects.numChunks <= 0 || objects.numChunks > objects.maxChunks ||
        objects.maxChunks >= 2048) return {};
    std::array<UCHAR,256> table{};
    if (!PointerTable(base, table)) return {};
    for (int c=0; c<objects.numChunks; ++c) {
        std::uintptr_t chunk=0;
        if (!Read(objects.chunks+c*8, chunk) || !chunk) break;
        const int remaining=objects.count-c*65536;
        if (remaining <= 0) break;
        const int n=remaining < 65536 ? remaining : 65536;
        std::vector<UCHAR> items(static_cast<size_t>(n)*40);
        if (!ReadBlock(chunk, items.data(), items.size())) break;
        for (int i=0; i<n; ++i) {
            std::uintptr_t object=Decode(items.data()+i*40+8, table);
            if (!object) continue;
            std::int32_t index=-1;
            if (!Read(object+12, index) || index != c*65536+i) continue;
            LocalPlayer candidate{};
            if (Validate(object, candidate)) return candidate;
        }
    }
    return {};
}

// The FName pool stores encrypted text. The two native decoder RVAs above are
// the ones emulated by Preservation/tools/dump_objects.py. Calling them here
// makes row and asset lookup independent of FName indices assigned this run.
// Keep the SEH boundary small: an invalid pool entry must fail lookup.
bool CallNameDecoder(std::uintptr_t function, std::uintptr_t entry,
                     void* output, std::size_t length) {
    using Decoder = void(__fastcall*)(const void*, void*, std::size_t);
    __try {
        reinterpret_cast<Decoder>(function)(reinterpret_cast<const void*>(entry),
                                            output, length);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

// Resolve an FName index to its base text. The FName instance number is stored
// separately and is intentionally ignored; for example, a generated
// DataTable_123 object has the base name "DataTable".
// All names used by these fixes are ASCII; reject other decoded text here.
bool Name(std::uintptr_t base, std::uint32_t index, std::string& result) {
    std::uintptr_t block=0;
    if (!Read(base+kNamePoolRva+16+8*(index>>16), block) || !block) return false;
    const auto entry=block+2*(index&0xffff);
    std::uint16_t header=0;
    if (!Read(entry, header)) return false;
    const std::size_t length=header>>6;
    if (!length || length>512) return false;
    std::array<UCHAR, 2048> output{};
    const bool wide=(header&1)!=0;
    if (!CallNameDecoder(base+(wide?kWideNameDecoderRva:kAnsiNameDecoderRva),
                         entry, output.data(), length)) return false;
    result.clear();
    result.reserve(length);
    if (wide) {
        for (std::size_t i=0; i<length; ++i) {
            const auto letter=std::uint16_t(output[2*i]) |
                              (std::uint16_t(output[2*i+1])<<8);
            if (!letter || letter>0x7f) return false;
            result.push_back(static_cast<char>(letter));
        }
    } else {
        for (std::size_t i=0; i<length; ++i) {
            if (!output[i] || output[i]>0x7f) return false;
            result.push_back(static_cast<char>(output[i]));
        }
    }
    return true;
}

// Read the base FName of a UObject, such as DataTable or TBL-BuffData.
bool ObjectNamed(std::uintptr_t base, std::uintptr_t object, const char* wanted) {
    std::uint32_t index=0;
    std::string name;
    return Read(object+16, index) && Name(base, index, name) && name==wanted;
}

struct RowMap {
    std::uintptr_t entries=0;
    std::int32_t count=0, capacity=0;
};
// DataTable.RowMap is at +0x38 in this build. Its sparse TMap storage has
// 32-byte slots; count can be smaller than capacity.
bool ReadRowMap(std::uintptr_t table, RowMap& map) {
    return Read(table+0x38, map) && map.entries && map.count>0 &&
           map.count<=map.capacity && map.capacity<=20000;
}

struct NamedRows {
    std::uintptr_t first=0, second=0;
    std::uint32_t firstKey=0, secondKey=0;
    std::int32_t firstPosition=-1, secondPosition=-1;
    std::uintptr_t mapEntries=0;
};

// Walk every allocated row-map slot, including gaps in the sparse array.
// Save each matching row's key index, pointer, and slot position so later
// checks can tell if the merged table was rebuilt or its storage moved.
NamedRows FindNamedRows(std::uintptr_t base, std::uintptr_t table,
                        const char* firstName, const char* secondName) {
    RowMap map{};
    NamedRows rows{};
    if (!ReadRowMap(table, map)) return rows;
    rows.mapEntries=map.entries;
    for (int i=0; i<map.capacity && (!rows.first || !rows.second); ++i) {
        const auto slot=map.entries+static_cast<std::uintptr_t>(i)*32;
        std::uint32_t key=0;
        std::uintptr_t row=0;
        if (!Read(slot, key) || !Read(slot+16, row) || !row) continue;
        std::string name;
        if (!Name(base, key, name)) continue;
        std::uint64_t rowStart=0;
        if (!Read(row, rowStart)) continue;
        if (name==firstName) { rows.first=row; rows.firstKey=key; rows.firstPosition=i; }
        if (name==secondName) { rows.second=row; rows.secondKey=key; rows.secondPosition=i; }
    }
    return rows;
}

// Read a short UE FString without trusting its pointer or capacity. The buff
// table's Param01/Param02 values validate that the replacement rows still
// mean "All" skills and +2/+3 levels.
bool StringEquals(std::uintptr_t address, const wchar_t* expected) {
    std::uintptr_t data=0;
    std::int32_t count=0, capacity=0;
    if (!Read(address, data) || !Read(address+8, count) ||
        !Read(address+12, capacity) || !data ||
        count<1 || count>capacity || capacity>64) return false;
    const auto length=wcslen(expected);
    if (count!=static_cast<std::int32_t>(length) &&
        count!=static_cast<std::int32_t>(length+1)) return false;
    std::array<wchar_t,64> contents{};
    return ReadBlock(data, contents.data(), count*sizeof(wchar_t)) &&
           wcscmp(contents.data(), expected)==0;
}

struct CapsuleRows {
    std::uintptr_t table=0, white=0, gold=0;
    std::uintptr_t mapEntries=0;
    std::int32_t whitePosition=-1, goldPosition=-1;
    std::uint32_t whiteKey=0, goldKey=0;
    std::uint32_t whiteBuff=0, goldBuff=0;
};

// Find the generated merged item DataTable, not merely the packaged TBL-Item
// source table. The live experiment only established that changing the merged
// Tablet_White/Tablet_Black rows fixes capsule use. Also find TBL-BuffData and
// confirm that its 220000104/220000105 rows have the expected parameters.
// Returned FName indices and addresses are resolved afresh for this process.
CapsuleRows FindCapsuleRows(std::uintptr_t base) {
    CapsuleRows result{};
    std::string zero;
    if (!Name(base, 0, zero) || zero!="None") return result;
    ObjectArray objects{};
    std::array<UCHAR,256> pointerTable{};
    if (!Read(base+kObjectsRva, objects) || objects.count<=0 ||
        objects.count>objects.maximum || objects.maximum>0x1000000 ||
        objects.numChunks<=0 || objects.numChunks>objects.maxChunks ||
        objects.maxChunks>=2048 || !PointerTable(base, pointerTable)) return result;
    NamedRows items{}, buffs{};
    std::uintptr_t itemTable=0;
    std::unordered_map<std::uintptr_t,bool> dataTableClass;
    for (int c=0; c<objects.numChunks; ++c) {
        std::uintptr_t chunk=0;
        if (!Read(objects.chunks+c*8, chunk) || !chunk) break;
        const int remaining=objects.count-c*65536;
        if (remaining<=0) break;
        const int n=remaining<65536?remaining:65536;
        std::vector<UCHAR> slots(static_cast<std::size_t>(n)*40);
        if (!ReadBlock(chunk, slots.data(), slots.size())) break;
        for (int i=0; i<n; ++i) {
            const auto object=Decode(slots.data()+i*40+8, pointerTable);
            if (!object) continue;
            std::int32_t index=-1;
            std::uintptr_t cls=0;
            if (!Read(object+12, index) || index!=c*65536+i ||
                !Read(object+32, cls) || !cls) continue;
            auto known=dataTableClass.find(cls);
            if (known==dataTableClass.end())
                known=dataTableClass.emplace(cls, ObjectNamed(base,cls,"DataTable") ||
                                                     ObjectNamed(base,cls,"CompositeDataTable")).first;
            if (!known->second) continue;
            std::uintptr_t rowStruct=0;
            if (!Read(object+48, rowStruct) || !rowStruct) continue;
            if (!itemTable && ObjectNamed(base,object,"DataTable") &&
                ObjectNamed(base,rowStruct,"InventoryItemDetailInfo")) {
                auto found=FindNamedRows(base,object,"Tablet_White","Tablet_Black");
                if (found.first && found.second) { itemTable=object; items=found; }
            }
            if (!buffs.first && ObjectNamed(base,object,"TBL-BuffData") &&
                ObjectNamed(base,rowStruct,"BuffData")) {
                auto found=FindNamedRows(base,object,"220000104","220000105");
                // These are the All-skills +2 and +3 buffs verified in the
                // live memory experiment.
                if (found.first && found.second &&
                    StringEquals(found.first+376,L"All") &&
                    StringEquals(found.first+392,L"2") &&
                    StringEquals(found.second+376,L"All") &&
                    StringEquals(found.second+392,L"3")) buffs=found;
            }
            if (itemTable && buffs.first) break;
        }
        if (itemTable && buffs.first) break;
    }
    if (!itemTable || !buffs.first) return {};
    result={itemTable,items.first,items.second,items.mapEntries,
            items.firstPosition,items.secondPosition,items.firstKey,items.secondKey,
            buffs.firstKey,buffs.secondKey};
    return result;
}

// Confirm the map still uses the same allocation and that both keys still
// point at the same rows before reading or rewriting their UsingBuffName data.
bool RowsStillMapped(const CapsuleRows& rows) {
    RowMap map{};
    if (!rows.table || !ReadRowMap(rows.table,map) ||
        map.entries!=rows.mapEntries ||
        rows.whitePosition<0 || rows.whitePosition>=map.capacity ||
        rows.goldPosition<0 || rows.goldPosition>=map.capacity) return false;
    auto matches=[&map](std::int32_t position, std::uint32_t expectedKey,
                        std::uintptr_t expectedRow) {
        const auto slot=map.entries+static_cast<std::uintptr_t>(position)*32;
        std::uint32_t key=0;
        std::uintptr_t row=0;
        return Read(slot,key) && Read(slot+16,row) &&
               key==expectedKey && row==expectedRow;
    };
    return matches(rows.whitePosition,rows.whiteKey,rows.white) &&
           matches(rows.goldPosition,rows.goldKey,rows.gold);
}

// InventoryItemDetailInfo.UsingBuffName is a TArray at row +0x508. The live
// experiment changed only the first FName's four-byte comparison index, not
// the array header or the rest of its 12-byte FName value. Require one entry
// and a recognized broken/fixed buff name before returning the slot address.
bool CapsuleSlots(std::uintptr_t base, const CapsuleRows& rows,
                  std::uintptr_t& whiteSlot, std::uintptr_t& goldSlot) {
    auto slot=[base](std::uintptr_t row, const char* broken,
                     const char* fixed, std::uintptr_t& address) {
        std::uintptr_t array=0;
        std::int32_t count=0, capacity=0;
        if (!Read(row+0x508,array) || !array ||
            !Read(row+0x510,count) || !Read(row+0x514,capacity) ||
            count!=1 || capacity<count || capacity>16) return false;
        std::uint32_t index=0;
        std::string name;
        if (!Read(array,index) || !Name(base,index,name) ||
            (name!=broken && name!=fixed)) return false;
        address=array;
        return true;
    };
    return slot(rows.white,"221000337","220000104",whiteSlot) &&
           slot(rows.gold,"221000338","220000105",goldSlot);
}

struct CapsulePatch {
    CapsuleRows rows{};
    std::uintptr_t whiteSlot=0, goldSlot=0;
    std::uint32_t whiteOriginal=0, goldOriginal=0;
};

// Apply White 221000337 -> 220000104 and Gold 221000338 -> 220000105.
// Verify both broken names before either write; if Gold fails, put White back.
// CompareDword changes exactly one four-byte FName index per item row.
bool ApplyCapsulePatch(std::uintptr_t base, const CapsuleRows& rows,
                       CapsulePatch& patch) {
    std::uintptr_t whiteSlot=0, goldSlot=0;
    if (!rows.whiteBuff || !rows.goldBuff || !RowsStillMapped(rows) ||
        !CapsuleSlots(base,rows,whiteSlot,goldSlot)) return false;
    std::uint32_t white=0,gold=0;
    if (!Read(whiteSlot,white) || !Read(goldSlot,gold)) return false;
    std::string whiteName,goldName;
    if (!Name(base,white,whiteName) || whiteName!="221000337" ||
        !Name(base,gold,goldName) || goldName!="221000338") return false;
    if (!CompareDword(whiteSlot,static_cast<LONG>(white),
                     static_cast<LONG>(rows.whiteBuff))) return false;
    if (!CompareDword(goldSlot,static_cast<LONG>(gold),
                     static_cast<LONG>(rows.goldBuff))) {
        CompareDword(whiteSlot,static_cast<LONG>(rows.whiteBuff),static_cast<LONG>(white));
        return false;
    }
    patch={rows,whiteSlot,goldSlot,white,gold};
    return true;
}

// The merged table may load after injection or be rebuilt during play. Search
// every five seconds until patched, then check its two slots once per second.
// If storage moves, discard those addresses and resolve the new table; if the
// game restores the original indices in place, apply the remap again.
DWORD WINAPI RunCapsules(LPVOID imageBase) {
    const auto base=reinterpret_cast<std::uintptr_t>(imageBase);
    CapsulePatch patch{};
    bool announced=false;
    Log(L"Capsule fix: waiting for the item table.\r\n");
    for (;;) {
        if (patch.rows.table) {
            if (!RowsStillMapped(patch.rows)) {
                patch={};
            } else {
                std::uint32_t white=0, gold=0;
                if (!Read(patch.whiteSlot,white) || !Read(patch.goldSlot,gold) ||
                    (white!=patch.whiteOriginal && white!=patch.rows.whiteBuff) ||
                    (gold!=patch.goldOriginal && gold!=patch.rows.goldBuff)) {
                    patch={};
                } else {
                    // Reapply if the game rebuilt the row values in place.
                    if (white==patch.whiteOriginal)
                        CompareDword(patch.whiteSlot,static_cast<LONG>(white),
                                    static_cast<LONG>(patch.rows.whiteBuff));
                    if (gold==patch.goldOriginal)
                        CompareDword(patch.goldSlot,static_cast<LONG>(gold),
                                    static_cast<LONG>(patch.rows.goldBuff));
                }
            }
        }
        if (!patch.rows.table) {
            const CapsuleRows rows=FindCapsuleRows(base);
            if (rows.table && ApplyCapsulePatch(base,rows,patch) && !announced) {
                Log(L"Capsule fix: White and Gold buff IDs corrected.\r\n");
                announced=true;
            }
        }
        Sleep(patch.rows.table ? 1000 : 5000);
    }
}

struct FirstBloodGraph {
    std::uintptr_t widgetClass=0;
    std::uintptr_t reference=0;
    std::uintptr_t firstSound=0;
};

// Find UW-Inventory_Perk_C.ExecuteUbergraph_UW-Inventory_Perk among loaded
// UFunctions. Its TArray script descriptor is at function +0x70. The two
// references at script +0x1B47 and +0x1C9F must name AK_UI_FirstKill and
// AK_UI_KillBonus respectively; this guards against an offset/layout mismatch.
// The graph's Outer is the widget class used in PerkWidget().
FirstBloodGraph FindFirstBloodGraph(std::uintptr_t base) {
    ObjectArray objects{};
    std::array<UCHAR,256> table{};
    if (!Read(base+kObjectsRva,objects) || objects.count<=0 ||
        objects.count>objects.maximum || objects.maximum>0x1000000 ||
        objects.numChunks<=0 || objects.numChunks>objects.maxChunks ||
        objects.maxChunks>=2048 || !PointerTable(base,table)) return {};
    std::unordered_map<std::uintptr_t,bool> functionClass;
    for (int c=0; c<objects.numChunks; ++c) {
        std::uintptr_t chunk=0;
        if (!Read(objects.chunks+c*8,chunk) || !chunk) break;
        const int remaining=objects.count-c*65536;
        if (remaining<=0) break;
        const int n=remaining<65536?remaining:65536;
        std::vector<UCHAR> items(static_cast<std::size_t>(n)*40);
        if (!ReadBlock(chunk,items.data(),items.size())) break;
        for (int i=0; i<n; ++i) {
            const auto object=Decode(items.data()+i*40+8,table);
            if (!object) continue;
            std::int32_t index=-1;
            std::uintptr_t cls=0, outer=0;
            if (!Read(object+12,index) || index!=c*65536+i ||
                !Read(object+32,cls) || !cls) continue;
            auto known=functionClass.find(cls);
            if (known==functionClass.end())
                known=functionClass.emplace(cls,ObjectNamed(base,cls,"Function")).first;
            if (!known->second ||
                !Read(object+40,outer) || !outer ||
                !ObjectNamed(base,object,"ExecuteUbergraph_UW-Inventory_Perk") ||
                !ObjectNamed(base,outer,"UW-Inventory_Perk_C")) continue;
            std::uintptr_t script=0,first=0,bonus=0;
            std::int32_t count=0,capacity=0;
            if (!Read(object+0x70,script) || !script ||
                !Read(object+0x78,count) || !Read(object+0x7c,capacity) ||
                count<=0x1ca7 || count>capacity || capacity>=0x200000 ||
                !Read(script+0x1b47,first) || !first ||
                !Read(script+0x1c9f,bonus) || !bonus || first==bonus ||
                !ObjectNamed(base,first,"AK_UI_FirstKill") ||
                !ObjectNamed(base,bonus,"AK_UI_KillBonus")) continue;
            return {outer,script+0x1b47,first};
        }
    }
    return {};
}

// Follow the pointer chain recorded by the live First Blood gate:
// Controller.MyHUD +0x428 -> HUD +0x538 -> main widget +0x468 ->
// top widget +0x2F8 -> perk widget. Check the final UObject class against
// the verified graph Outer before reading its selected-sound field.
std::uintptr_t PerkWidget(std::uintptr_t controller,
                          std::uintptr_t expectedClass) {
    std::uintptr_t hud=0,main=0,top=0,widget=0,cls=0;
    return controller && expectedClass &&
           Read(controller+0x428,hud) && hud &&
           Read(hud+0x538,main) && main &&
           Read(main+0x468,top) && top &&
           Read(top+0x2f8,widget) && widget &&
           Read(widget+32,cls) && cls==expectedClass ? widget : 0;
}

// The reference is an unaligned eight-byte UObject pointer inside loaded
// Blueprint script data. The read/compare/write/readback mirrors the validated
// memory gate. This is not an atomic swap, so another writer changing the same
// script location concurrently would need a different coordination scheme.
bool ReplaceScriptReference(std::uintptr_t address,
                            std::uintptr_t expected,
                            std::uintptr_t replacement) {
    std::uintptr_t current=0;
    if (!Read(address,current) || current!=expected) return false;
    SIZE_T written=0;
    if (!WriteProcessMemory(GetCurrentProcess(),
                            reinterpret_cast<void*>(address),&replacement,
                            sizeof(replacement),&written) ||
        written!=sizeof(replacement)) return false;
    return Read(address,current) && current==replacement;
}

// Restore the original First Kill reference between matches. A second restore
// attempt is harmless if it already contains the original asset pointer.
bool ArmFirstSound(const FirstBloodGraph& graph) {
    std::uintptr_t current=0;
    return Read(graph.reference,current) &&
           (current==graph.firstSound ||
            (current==0 && ReplaceScriptReference(graph.reference,0,
                                                  graph.firstSound)));
}

// Mirror Preservation/tools/first_blood_memory_gate.py. The selected sound at
// perk widget +0x8E8 changes to AK_UI_FirstKill for a bot's personal first
// kill. On the first observed selection, clear only that script reference;
// this silences later requests without changing perk counts or using Kill Bonus.
// A new widget, or two seconds without one, arms the reference for the next
// match. Polling every 25 ms preserves the verified workaround but can miss
// exceptionally close events; an event hook would remove that timing limit.
DWORD WINAPI RunFirstBlood(LPVOID imageBase) {
    const auto base=reinterpret_cast<std::uintptr_t>(imageBase);
    FirstBloodGraph graph{};
    std::uintptr_t widget=0,lastSound=0;
    ULONGLONG missingSince=0;
    bool gated=false;
    Log(L"First Blood fix: waiting for the perk audio assets.\r\n");
    for (;;) {
        if (!graph.reference) {
            graph=FindFirstBloodGraph(base);
            if (!graph.reference) { Sleep(2000); continue; }
            Log(L"First Blood fix: ready; first-kill audio is limited to once per match.\r\n");
        }
        const auto controller=gLocalController.load(std::memory_order_acquire);
        const auto currentWidget=PerkWidget(controller,graph.widgetClass);
        const auto now=GetTickCount64();
        if (!currentWidget) {
            if (!missingSince) missingSince=now;
            if (gated && now-missingSince>=2000) {
                if (ArmFirstSound(graph)) {
                    gated=false;
                    Log(L"First Blood fix: audio rearmed for the next match.\r\n");
                }
            }
            // Retain the old widget during a brief visibility gap. If a new
            // widget appears quickly, its changed identity still resets the
            // gate; a transient gap in the same match leaves it muted.
            if (now-missingSince>=2000) {
                widget=0;
                lastSound=0;
            }
        } else {
            missingSince=0;
            if (currentWidget!=widget) {
                if (gated) {
                    if (!ArmFirstSound(graph)) { Sleep(25); continue; }
                    gated=false;
                    Log(L"First Blood fix: audio rearmed for the new match.\r\n");
                }
                widget=currentWidget;
                lastSound=0;
            }
            std::uintptr_t sound=0;
            if (Read(widget+0x8e8,sound)) {
                if (sound==graph.firstSound && sound!=lastSound && !gated) {
                    if (ReplaceScriptReference(graph.reference,graph.firstSound,0)) {
                        gated=true;
                        Log(L"First Blood fix: first cue detected; subsequent first-kill cues muted.\r\n");
                    }
                }
                lastSound=sound;
            }
        }
        Sleep(25);
    }
}

// Runs on the game thread after the blue zone hook called the original; not
// while threads are suspended, so allocation is fine here.
void ReportBlueZoneRow(std::int32_t selected, std::uint32_t comparison, std::uint32_t number) {
    const auto base=reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr));
    std::string name;
    wchar_t message[192];
    if (!Name(base, comparison, name))
        swprintf_s(message, L"Blue zone: selected index %d\r\n", selected);
    else if (number) // FName instance numbers are the trailing "_N" plus one.
        swprintf_s(message, L"Blue zone: selected index %d (%S_%02u)\r\n",
                   selected, name.c_str(), number - 1);
    else
        swprintf_s(message, L"Blue zone: selected index %d (%S)\r\n", selected, name.c_str());
    Log(message);
}

// Start after DllMain returns. The console appears before hash verification so
// an unsupported build reports why no fix started. Capsules and First Blood
// use their own workers. This thread tracks
// the standalone local controller for class selection and publishes it to
// the First Blood worker.
DWORD WINAPI Run(LPVOID) {
    wchar_t consoleSetting[2]{};
    if (GetEnvironmentVariableW(L"SP_CLIENT_FIXES_CONSOLE",consoleSetting,2)==1 &&
        consoleSetting[0]==L'1' && AllocConsole())
        SetConsoleTitleW(L"SP Client Fixes");
    Log(L"SP Client Fixes DLL loaded. Checking game build...\r\n");
    if (!SupportedBuild()) {
        Log(L"Client fixes disabled: unsupported executable SHA-256.\r\n");
        return 0;
    }
    Log(L"Supported build. Starting client fixes...\r\n");
    const auto base=reinterpret_cast<std::uintptr_t>(GetModuleHandleW(nullptr));
    // Bot-match patches are silent except for the selected blue zone. Each one
    // verifies the exact bytes first and leaves the game unchanged on any mismatch.
    (void)standalone_options::Install(base);
    (void)standalone_options::InstallRate(base);
    (void)standalone_options::InstallAiTimer(base);
    (void)standalone_options::InstallBlueZone(base,ReportBlueZoneRow);
    if (!clientfixes_signing::Install(base,Log))
        Log(L"Client fixes: PAK initialization failed.\r\n");
    HANDLE capsuleThread=CreateThread(nullptr,0,RunCapsules,
                                      reinterpret_cast<LPVOID>(base),0,nullptr);
    if (capsuleThread) CloseHandle(capsuleThread);
    else Log(L"Capsule fix: could not start worker thread.\r\n");
    HANDLE firstBloodThread=CreateThread(nullptr,0,RunFirstBlood,
                                         reinterpret_cast<LPVOID>(base),0,nullptr);
    if (firstBloodThread) CloseHandle(firstBloodThread);
    else Log(L"First Blood fix: could not start worker thread.\r\n");
    LocalPlayer active{};
    LONG original=-1;
    DWORD nextScan=0;
    bool classAnnounced=false;
    Log(L"Class selection: waiting for a standalone local player.\r\n");
    for (;;) {
        LocalPlayer current{};
        if (active.controller && Validate(active.controller, current) &&
            current.world == active.world && current.info == active.info) {
            // The existing local world remains active.
        } else {
            // Restore only if the same old player info is still accessible
            // and unnetworked. Otherwise a stale pointer is left untouched.
            if (original >= 0 && SafeToRestore(active))
                CompareDword(active.info+632, kClassLevel, original);
            active={};
            original=-1;
            current={};
            if (static_cast<LONG>(GetTickCount()-nextScan) >= 0) {
                current=FindLocalPlayer(base);
                nextScan=GetTickCount()+2000;
            }
        }
        if (current.info) {
            active=current;
            gLocalController.store(current.controller,std::memory_order_release);
            LONG level=-1;
            // The live class-selection experiment established that level 5
            // satisfies every current class tile. A legitimate higher level
            // is never lowered, and the original low value is kept for exit.
            const bool levelRead=Read(current.info+632, level);
            if (levelRead && level >= 0 && level < kClassLevel &&
                CompareDword(current.info+632, level, kClassLevel)) {
                if (original<0) original=level;
                if (!classAnnounced) {
                    Log(L"Class selection: local level set to 5; class tiles are available.\r\n");
                    classAnnounced=true;
                }
            } else if (levelRead && level>=kClassLevel && !classAnnounced) {
                Log(L"Class selection: local level already meets the class requirements.\r\n");
                classAnnounced=true;
            }
        } else {
            gLocalController.store(0,std::memory_order_release);
        }
        Sleep(250);
    }
}
} // namespace

// Exported version marker for identifying which DLL was embedded. This number
// advances when a fix is added; the launcher does not currently branch on it.
extern "C" __declspec(dllexport) unsigned int SPClientFixesVersion() { return 17; }

// DllMain runs under the Windows loader lock. Only disable thread callbacks
// and start the bootstrap worker here; do not hash files, scan UObjects, wait
// for a thread, or run game code until Run() begins after this callback.
BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(module);
        HANDLE thread=CreateThread(nullptr, 0, Run, nullptr, 0, nullptr);
        if (thread) CloseHandle(thread);
    }
    return TRUE;
}
