# rn-anydoc: the Nitro glue plus the prebuilt anydoc Rust core.
project(RNAnydoc)
cmake_minimum_required(VERSION 3.9.0)

set(PACKAGE_NAME RNAnydoc)
set(CMAKE_VERBOSE_MAKEFILE ON)
set(CMAKE_CXX_STANDARD 20)

# The module's own sources.
add_library(
  ${PACKAGE_NAME}
  SHARED
  ../cpp/HybridDocumentConverter.cpp
)
include_directories(../cpp)

# Everything nitrogen generated (spec, autolinking, RN/Nitro linkage).
include(${CMAKE_SOURCE_DIR}/../nitrogen/generated/android/RNAnydoc+autolinking.cmake)

# The Rust core, linked statically INTO libRNAnydoc.so - one shipped library,
# and the linker drops everything the FFI surface doesn't reach.
# Built by scripts/build-rust.sh; see the README.
add_library(anydoc_core STATIC IMPORTED)
set_target_properties(anydoc_core PROPERTIES
  IMPORTED_LOCATION ${CMAKE_SOURCE_DIR}/libs/${ANDROID_ABI}/libanydoc_mobile.a
)
target_link_libraries(${PACKAGE_NAME} anydoc_core)
