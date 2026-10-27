cmake_minimum_required(VERSION 3.20)
cmake_policy(SET CMP0091 NEW)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded")
project(deepdark_oracle_llama_shim CXX C)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# llama.cpp 本体：只编库，不编 examples/tests/server
set(LLAMA_BUILD_TESTS     OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_EXAMPLES  OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_SERVER    OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TOOLS     OFF CACHE BOOL "" FORCE)
set(LLAMA_CURL            OFF CACHE BOOL "" FORCE)
set(GGML_NATIVE           ON  CACHE BOOL "" FORCE)
set(BUILD_SHARED_LIBS     OFF CACHE BOOL "" FORCE)

# Keep the default build CPU-only and portable.  build.rs sets this option
# when DEEPAISEE_CUDA=1.
option(DEEPAISEE_CUDA "Build llama.cpp with the CUDA backend" OFF)
if (DEEPAISEE_CUDA)
    set(GGML_CUDA ON CACHE BOOL "" FORCE)
else()
    set(GGML_CUDA OFF CACHE BOOL "" FORCE)
endif()

# Vulkan enumerates both discrete and integrated GPUs on Windows. This is
# optional so CPU/CUDA-only builds do not require the Vulkan SDK.
option(DEEPAISEE_VULKAN "Build llama.cpp with the Vulkan backend" OFF)
if (DEEPAISEE_VULKAN)
    set(GGML_VULKAN ON CACHE BOOL "" FORCE)
else()
    set(GGML_VULKAN OFF CACHE BOOL "" FORCE)
endif()

if (MSVC)
    # Keep UTF-8 source support for the C/C++ shim and host code. The
    # language filter prevents CMake from forwarding this MSVC option to nvcc.
    add_compile_options("$<$<COMPILE_LANGUAGE:C,CXX>:/utf-8>")
endif()

add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/llama.cpp
                 ${CMAKE_CURRENT_BINARY_DIR}/llama.cpp)

add_library(llama_shim STATIC llama_shim.cpp)
target_include_directories(llama_shim PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/llama.cpp/include
    ${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/llama.cpp/ggml/include)
target_link_libraries(llama_shim PUBLIC llama)
install(TARGETS llama_shim ARCHIVE DESTINATION lib)
