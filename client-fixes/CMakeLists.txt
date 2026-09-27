cmake_minimum_required(VERSION 3.21)
project(SPClientFixes LANGUAGES CXX)

add_library(SPClientFixes SHARED src/client_fixes.cpp src/custom_pak_signing.cpp)
target_compile_features(SPClientFixes PRIVATE cxx_std_20)
target_compile_definitions(SPClientFixes PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX)
target_link_libraries(SPClientFixes PRIVATE bcrypt)
set_target_properties(SPClientFixes PROPERTIES OUTPUT_NAME SPClientFixes)

if(NOT WIN32 OR NOT CMAKE_SIZEOF_VOID_P EQUAL 8)
    message(FATAL_ERROR "SPClientFixes requires a 64-bit Windows toolchain")
endif()

include(CTest)
if(BUILD_TESTING)
    add_executable(clientfixes_signing_tests src/signing_tests.cpp src/custom_pak_signing.cpp)
    target_compile_features(clientfixes_signing_tests PRIVATE cxx_std_20)
    target_compile_definitions(clientfixes_signing_tests PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX)
    target_link_libraries(clientfixes_signing_tests PRIVATE bcrypt)
    add_test(NAME custom_pak_signatures COMMAND clientfixes_signing_tests)
    add_test(NAME bundled_pak_signature COMMAND clientfixes_signing_tests
        "${CMAKE_CURRENT_SOURCE_DIR}/../src-tauri/resources/client-fixes/BravoHotelGame-ClientFixes_P.pak"
        "${CMAKE_CURRENT_SOURCE_DIR}/../src-tauri/resources/client-fixes/BravoHotelGame-ClientFixes_P.sig")
endif()