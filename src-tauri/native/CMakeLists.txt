cmake_minimum_required(VERSION 3.20)
project(deepaisee_llama_shim CXX C)

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

if (MSVC)
    add_compile_options(/utf-8)
endif()

add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/llama.cpp
                 ${CMAKE_CURRENT_BINARY_DIR}/llama.cpp)

add_library(llama_shim STATIC llama_shim.cpp)
target_include_directories(llama_shim PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/llama.cpp/include
    ${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/llama.cpp/ggml/include)
target_link_libraries(llama_shim PUBLIC llama)
