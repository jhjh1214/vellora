# Vellora desktop shell (M0 task 22b).
#
# The shell never parses PDF data: it talks to `vellora-engine` through the cxx bridge of the Rust
# crate `vellora-engine-client` (ADR-0003, ADR-0004). Corrosion builds the crate as a static
# library and the engine as an executable that is copied next to the shell, together with PDFium.
cmake_minimum_required(VERSION 3.28)
project(vellora-app VERSION 0.0.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_AUTOMOC ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# MSVC: Rust links the release C runtime (`/MD`) and builds the cxx glue with it, and a mix of
# runtimes (or `_ITERATOR_DEBUG_LEVEL`s) does not link. So every configuration of the shell uses
# the release runtime and the release Qt libraries; Debug still has no optimisation and has
# debug information.
if(MSVC)
  set(CMAKE_MSVC_RUNTIME_LIBRARY MultiThreadedDLL)
endif()

# Qt6::GuiPrivate: QRhi (the GPU abstraction QRhiWidget draws with) is only available through Qt's
# private headers. ShaderTools provides `qsb`, which turns the GLSL in src/canvas/shaders into
# shaders for every graphics API.
find_package(Qt6 6.8 REQUIRED COMPONENTS Widgets Test ShaderTools)
if(MSVC)
  # Only Qt's targets: Corrosion's imported targets exist for the active configuration only.
  set_target_properties(Qt6::Core Qt6::Gui Qt6::Widgets Qt6::Test
                        PROPERTIES MAP_IMPORTED_CONFIG_DEBUG Release)
endif()

include(FetchContent)
FetchContent_Declare(
  Corrosion
  GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
  GIT_TAG v0.6.1)
FetchContent_MakeAvailable(Corrosion)

get_filename_component(VELLORA_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/.." ABSOLUTE)

# `--locked`: the app builds exactly the dependencies in Cargo.lock, like CI's cargo jobs.
corrosion_import_crate(
  MANIFEST_PATH "${VELLORA_ROOT}/Cargo.toml"
  CRATES vellora-engine-client vellora-engine
  LOCKED)

# build.rs of `vellora-engine-client` copies the generated header and `rust/cxx.h` here, because
# cargo's own output directory contains a hash.
set(VELLORA_CXXBRIDGE_DIR "${CMAKE_BINARY_DIR}/cxxbridge")
corrosion_set_env_vars(vellora_engine_client "VELLORA_CXXBRIDGE_DIR=${VELLORA_CXXBRIDGE_DIR}")

# The shell's own code, as a library so the tests link exactly what the executable links.
add_library(vellora_shell STATIC
  src/MainWindow.cpp
  src/MainWindow.h
  src/canvas/CanvasController.cpp
  src/canvas/CanvasController.h
  src/canvas/CanvasView.cpp
  src/canvas/CanvasView.h
  src/canvas/CanvasWidget.cpp
  src/canvas/CanvasWidget.h
  src/canvas/PageLayout.cpp
  src/canvas/PageLayout.h
  src/bridge/EngineSession.cpp
  src/bridge/EngineSession.h)
target_include_directories(vellora_shell PUBLIC src "${VELLORA_CXXBRIDGE_DIR}/include")
target_link_libraries(vellora_shell PUBLIC Qt6::Widgets Qt6::GuiPrivate vellora_engine_client)
qt_add_shaders(vellora_shell "vellora_shaders"
  PREFIX "/shaders"
  BASE src/canvas/shaders
  GLSL "100es,120,150"
  HLSL 50
  MSL 12
  FILES src/canvas/shaders/quad.vert src/canvas/shaders/quad.frag)
target_compile_features(vellora_shell PUBLIC cxx_std_20)
if(MSVC)
  target_compile_options(vellora_shell PUBLIC /W4 /permissive- /utf-8)
else()
  target_compile_options(vellora_shell PUBLIC -Wall -Wextra -Wpedantic)
endif()

qt_add_executable(vellora src/main.cpp)
target_link_libraries(vellora PRIVATE vellora_shell)
set_target_properties(vellora PROPERTIES WIN32_EXECUTABLE ON MACOSX_BUNDLE OFF)

# The engine and PDFium live next to the shell (`engine_executable()` looks beside the running
# executable, and the engine looks beside itself for PDFium). PDFium comes from
# `cargo xtask pdfium fetch`; without it the shell still builds, but cannot render.
file(GLOB VELLORA_PDFIUM_LIBRARY
     "${VELLORA_ROOT}/third_party/pdfium/*/bin/pdfium.dll"
     "${VELLORA_ROOT}/third_party/pdfium/*/lib/libpdfium.so"
     "${VELLORA_ROOT}/third_party/pdfium/*/lib/libpdfium.dylib")
list(GET VELLORA_PDFIUM_LIBRARY 0 VELLORA_PDFIUM_LIBRARY)
if(NOT VELLORA_PDFIUM_LIBRARY)
  message(WARNING "PDFium not found: run `cargo xtask pdfium fetch` (the app cannot render without it)")
endif()

function(vellora_stage_runtime target)
  add_dependencies(${target} vellora-engine)
  add_custom_command(TARGET ${target} POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different $<TARGET_FILE:vellora-engine>
            $<TARGET_FILE_DIR:${target}>
    COMMENT "Staging vellora-engine next to ${target}")
  if(VELLORA_PDFIUM_LIBRARY)
    add_custom_command(TARGET ${target} POST_BUILD
      COMMAND ${CMAKE_COMMAND} -E copy_if_different "${VELLORA_PDFIUM_LIBRARY}"
              $<TARGET_FILE_DIR:${target}>
      COMMENT "Staging PDFium next to ${target}")
  endif()
endfunction()
vellora_stage_runtime(vellora)

enable_testing()
add_subdirectory(tests)
