#include "custom_pak_signing.hpp"
#include "signing_fixture.hpp"
#include <array>
#include <cstdio>
#include <vector>
#include <algorithm>
#include <iterator>
#include <fstream>
#include <cstring>

int main(int argc, char** argv) {
    if (argc==3) {
        std::ifstream input(argv[2],std::ios::binary);
        std::vector<unsigned char> sig((std::istreambuf_iterator<char>(input)),std::istreambuf_iterator<char>());
        if (sig.size()<532) return 4;
        auto u32=[&](std::size_t offset) { unsigned int value=0; std::memcpy(&value,sig.data()+offset,4); return value; };
        if (u32(0)!=0x73832daa || u32(4)!=1 || u32(8)!=512 ||
            sig.size()!=528ull+4ull*u32(524)) return 5;
        HANDLE pak=CreateFileA(argv[1],GENERIC_READ,FILE_SHARE_READ,nullptr,OPEN_EXISTING,FILE_ATTRIBUTE_NORMAL,nullptr);
        if (pak==INVALID_HANDLE_VALUE) return 6;
        const bool valid=clientfixes_signing::VerifyFile(pak,sig.data()+12,512,sig.data()+528,sig.size()-528);
        CloseHandle(pak);
        std::puts(valid ? "Bundled PAK signature verified." : "Bundled PAK signature FAILED.");
        return valid ? 0 : 7;
    }
    std::array<wchar_t,MAX_PATH> directory{},path{};
    if (!GetTempPathW(static_cast<DWORD>(directory.size()),directory.data()) ||
        !GetTempFileNameW(directory.data(),L"spf",0,path.data())) return 1;
    HANDLE file=CreateFileW(path.data(),GENERIC_READ|GENERIC_WRITE,0,nullptr,
                            CREATE_ALWAYS,FILE_ATTRIBUTE_TEMPORARY|FILE_FLAG_DELETE_ON_CLOSE,nullptr);
    if (file==INVALID_HANDLE_VALUE) { DeleteFileW(path.data()); return 2; }
    DWORD written=0;
    bool okay=WriteFile(file,kSigningFixture,sizeof(kSigningFixture),&written,nullptr) && written==sizeof(kSigningFixture);
    auto verify=[&](const unsigned char* signature,std::size_t size,const unsigned char* chunks,std::size_t chunksSize) {
        return clientfixes_signing::VerifyFile(file,signature,size,chunks,chunksSize);
    };
    okay=okay && verify(kSigningFixtureSignature,sizeof(kSigningFixtureSignature),kSigningFixtureChunks,sizeof(kSigningFixtureChunks));
    std::vector<unsigned char> signature(std::begin(kSigningFixtureSignature),std::end(kSigningFixtureSignature));
    signature[7]^=1;
    okay=okay && !verify(signature.data(),signature.size(),kSigningFixtureChunks,sizeof(kSigningFixtureChunks));
    okay=okay && !verify(kSigningFixtureSignature,511,kSigningFixtureChunks,sizeof(kSigningFixtureChunks));
    std::array<unsigned char,4> chunks{};
    std::copy(std::begin(kSigningFixtureChunks),std::end(kSigningFixtureChunks),chunks.begin());
    chunks[0]^=1;
    okay=okay && !verify(kSigningFixtureSignature,sizeof(kSigningFixtureSignature),chunks.data(),chunks.size());
    okay=okay && !verify(kSigningFixtureSignature,sizeof(kSigningFixtureSignature),chunks.data(),0);
    LARGE_INTEGER zero{};
    okay=okay && SetFilePointerEx(file,zero,nullptr,FILE_BEGIN);
    unsigned char changed=0;
    okay=okay && WriteFile(file,&changed,1,&written,nullptr) && written==1;
    okay=okay && !verify(kSigningFixtureSignature,sizeof(kSigningFixtureSignature),kSigningFixtureChunks,sizeof(kSigningFixtureChunks));
    CloseHandle(file);
    std::puts(okay ? "Signing verification passed: valid fixture accepted; modified PAK, CRC table, signature and invalid lengths rejected."
                   : "Signing verification FAILED.");
    return okay ? 0 : 3;
}