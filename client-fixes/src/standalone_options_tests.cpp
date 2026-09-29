#include "standalone_options.hpp"

#include <cstring>
#include <string>
#include <vector>

int main() {
    using namespace standalone_options;
    if (!MatchesOriginal(kTimeOriginal.data(), kAiOriginal.data()) ||
        MatchesOriginal(nullptr, kAiOriginal.data()) ||
        MatchesOriginal(kTimeOriginal.data(), nullptr)) return 1;
    auto malformed = kTimeOriginal;
    malformed[0] = 0x90;
    if (MatchesOriginal(malformed.data(), kAiOriginal.data())) return 2;
    malformed = kAiOriginal;
    malformed[2] = 50;
    if (MatchesOriginal(kTimeOriginal.data(), malformed.data())) return 3;
    // Duplicate or premodified instruction sites cannot be treated as originals.
    if (MatchesOriginal(kTimeConfigured.data(), kAiOriginal.data()) ||
        MatchesOriginal(kTimeOriginal.data(), kAiConfigured.data())) return 4;
    if (kTimeConfigured[2] != 10 || kAiConfigured[2] != 50 ||
        kTimeOriginal[0] != kTimeConfigured[0] || kTimeOriginal[1] != kTimeConfigured[1] ||
        kAiOriginal[0] != kAiConfigured[0] || kAiOriginal[1] != kAiConfigured[1]) return 5;
    std::array<unsigned char, 5> branch{};
    if (!RelativeBranch(0x1ca3063, 0x46b7c10, 0xe8, branch) ||
        branch != std::array<unsigned char, 5>{0xe8, 0xa8, 0x4b, 0xa1, 0x02}) return 6;
    if (!RelativeBranch(0x100000000ULL, 0x100000000ULL - 0x7ffffffbULL, 0xe9, branch) ||
        branch != std::array<unsigned char, 5>{0xe9, 0, 0, 0, 0x80}) return 7;
    if (RelativeBranch(0x100000000ULL, 0x100000000ULL + 5 + 0x80000000ULL, 0xe8, branch) ||
        RelativeBranch(0x100000000ULL, 0x100000000ULL - 0x7ffffffcULL, 0xe8, branch) ||
        RelativeBranch(0x1000, 0x2000, 0x90, branch)) return 8;
    double originalRate = 0, configuredRate = 0;
    std::memcpy(&originalRate, kRateOriginal.data(), sizeof(originalRate));
    std::memcpy(&configuredRate, kRateConfigured.data(), sizeof(configuredRate));
    if (originalRate != 0.800000011920929 || configuredRate != 0.5) return 9;
    float interval = 0;
    std::memcpy(&interval, kAiIntervalConfigured.data(), sizeof(interval));
    if (interval != 0.5f) return 10;

    // Blue zone selection: scripted byte source drives rejection sampling.
    static const unsigned char* script = nullptr;
    static std::size_t scriptLength = 0, scriptPos = 0;
    auto run = [&](const std::vector<unsigned char>& bytes, std::int32_t& out) {
        script = bytes.data(); scriptLength = bytes.size(); scriptPos = 0;
        return PickUniformIndex([](unsigned char* b, std::size_t n) {
            if (n != 1 || scriptPos >= scriptLength) return false;
            *b = script[scriptPos++];
            return true;
        }, kBlueZoneFirst, kBlueZoneLast, out);
    };
    std::int32_t picked = 0;
    // Span 20: bytes 0..239 accepted (240 = 12 * 20), 240..255 rejected.
    if (!run({0}, picked) || picked != 34) return 11;
    if (!run({19}, picked) || picked != 53) return 12;
    if (!run({20}, picked) || picked != 34) return 13;
    if (!run({239}, picked) || picked != 53) return 14;
    if (!run({255, 240, 7}, picked) || picked != 41 || scriptPos != 3) return 15;
    if (run({}, picked)) return 16; // source failure
    if (run(std::vector<unsigned char>(kMaxSelectionDraws, 250), picked)) return 17; // draw bound
    if (PickUniformIndex(nullptr, 34, 53, picked) ||
        PickUniformIndex([](unsigned char*, std::size_t) { return true; }, 5, 4, picked)) return 18;
    for (int value = 0; value != 240; ++value) {
        if (!run({static_cast<unsigned char>(value)}, picked) ||
            picked < kBlueZoneFirst || picked > kBlueZoneLast) return 19;
    }

    // Options detection over UTF-16.
    auto has = [](const std::u16string& s) {
        return OptionsContainAiCount(s.data(), static_cast<std::int32_t>(s.size()));
    };
    if (!has(u"?Game=x?ai_count=50") || !has(u"AI_COUNT=1") || !has(u"?Ai_Count=") ) return 20;
    if (has(u"") || has(u"ai_count") || has(u"ai_coun=1") || has(u"ai-count=1")) return 21;
    if (OptionsContainAiCount(nullptr, 20) || OptionsContainAiCount(u"ai_count=1", 0) ||
        OptionsContainAiCount(u"ai_count=1", 8)) return 22; // num bounds the scan
    std::u16string longText(1500, u'x');                      // party JSON precedes the options
    if (!has(longText + u"?ai_count=50")) return 23;         // past the old 1024-character cap
    if (!has(longText.substr(0, 1500 - 9) + u"ai_count=")) return 24; // exact end
    if (kMaxOptionsChars < 4096 || kOptionsChunkChars <= kAiCountNeedleChars) return 26;
    if (kBlueZoneFirst != 34 || kBlueZoneLast != 53) return 25;
    return 0;
}
