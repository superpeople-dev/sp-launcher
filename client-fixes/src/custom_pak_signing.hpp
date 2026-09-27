#pragma once
#include <windows.h>
#include <cstddef>
#include <cstdint>
namespace clientfixes_signing {
using Logger = void (*)(const wchar_t*);
bool Install(std::uintptr_t base, Logger logger);
// Exposed for synthetic offline verification; does not install hooks.
bool VerifyFile(HANDLE file, const unsigned char* signature, std::size_t signatureSize,
                const unsigned char* chunks, std::size_t chunksSize);
}