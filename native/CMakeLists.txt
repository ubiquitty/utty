# Copyright (C) 2026 Stanislav Yaglo
# SPDX-License-Identifier: Apache-2.0
cmake_minimum_required(VERSION 3.24)
project(utty-bridge LANGUAGES CXX)
find_package(UbiquittyCore 1.2 REQUIRED)
find_package(Threads REQUIRED)
add_library(tui-core STATIC core.cpp)
set_target_properties(tui-core PROPERTIES POSITION_INDEPENDENT_CODE ON)
target_compile_features(tui-core PRIVATE cxx_std_23)
target_compile_options(tui-core PRIVATE -Wall -Wextra -Wpedantic -Werror -fno-exceptions -fno-rtti)
target_link_libraries(tui-core PRIVATE Ubiquitty::ubiquitty-kit Threads::Threads)
file(GENERATE OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/rust-archives.txt" CONTENT
    "$<TARGET_FILE:tui-core>\n$<TARGET_FILE:Ubiquitty::ubiquitty-kit>\n$<TARGET_FILE:Ubiquitty::ubiquitty-core>\n$<TARGET_FILE:Ubiquitty::ubiquitty-behaviour>\n$<TARGET_FILE:Ubiquitty::ubiquitty-core-model>\n")
