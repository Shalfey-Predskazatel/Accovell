cmake_minimum_required(VERSION 3.24)

project(nsis_duilib_ui VERSION 0.1.0 LANGUAGES CXX)

if(NOT WIN32)
  message(FATAL_ERROR "nsis-duilib-ui only supports Windows")
endif()

if(NOT MSVC)
  message(FATAL_ERROR "The supported toolchain is MSVC")
endif()

if(NOT CMAKE_SIZEOF_VOID_P EQUAL 4)
  message(FATAL_ERROR "The NSIS plugin must be built for Win32/x86")
endif()

option(NSIS_DUILIB_UI_BUILD_TESTS "Build focused native tests" ON)
option(NSIS_DUILIB_UI_BUILD_PREVIEW "Build the interactive theme preview" ON)
option(NSIS_DUILIB_UI_BUILD_PLUGIN "Build the NSIS Unicode plugin" ON)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")

include(cmake/DuiLib.cmake)

add_library(nsis_dui_core STATIC
  src/renderer.cpp
  src/theme.cpp
  src/version.cpp
)

target_include_directories(nsis_dui_core PUBLIC include)
target_include_directories(nsis_dui_core SYSTEM PRIVATE third_party)
target_compile_definitions(nsis_dui_core PUBLIC
  UNICODE
  _UNICODE
  _WIN32_WINNT=0x0A00
  WINVER=0x0A00
)
target_compile_options(nsis_dui_core PRIVATE /W4 /permissive- /utf-8)

add_library(nsis_dui_ui STATIC src/window.cpp)
target_include_directories(nsis_dui_ui PUBLIC include)
target_link_libraries(nsis_dui_ui PUBLIC nsis_dui_core PRIVATE duilib shell32)
target_compile_definitions(nsis_dui_ui PUBLIC
  UNICODE
  _UNICODE
  _WIN32_WINNT=0x0A00
  WINVER=0x0A00
)
target_compile_options(nsis_dui_ui PRIVATE /W4 /permissive- /utf-8)

if(NSIS_DUILIB_UI_BUILD_PREVIEW)
  add_executable(ThemePreview WIN32
    preview/main.cpp
    preview/ThemePreview.manifest
  )
  target_link_libraries(ThemePreview PRIVATE nsis_dui_ui gdiplus shell32)
  target_compile_definitions(ThemePreview PRIVATE
    NSIS_DUI_SOURCE_DIR=L"${PROJECT_SOURCE_DIR}"
  )
  target_compile_options(ThemePreview PRIVATE /W4 /permissive- /utf-8)
endif()

if(NSIS_DUILIB_UI_BUILD_PLUGIN)
  add_library(nsis-duilib-ui SHARED
    plugin/nsis_plugin.cpp
    plugin/nsis-duilib-ui.def
  )
  target_include_directories(nsis-duilib-ui PRIVATE third_party/nsis)
  target_link_libraries(nsis-duilib-ui PRIVATE nsis_dui_ui comctl32)
  target_compile_options(nsis-duilib-ui PRIVATE /W4 /permissive- /utf-8)
  set_target_properties(nsis-duilib-ui PROPERTIES PREFIX "")
endif()

if(NSIS_DUILIB_UI_BUILD_TESTS)
  enable_testing()
  add_executable(nsis_dui_version_tests tests/version_test.cpp)
  target_link_libraries(nsis_dui_version_tests PRIVATE nsis_dui_core)
  target_compile_options(nsis_dui_version_tests PRIVATE /W4 /permissive- /utf-8)
  add_test(NAME version COMMAND nsis_dui_version_tests)

  add_executable(nsis_dui_theme_tests tests/theme_test.cpp)
  target_link_libraries(nsis_dui_theme_tests PRIVATE nsis_dui_core)
  target_compile_definitions(nsis_dui_theme_tests PRIVATE
    NSIS_DUI_SOURCE_DIR=L"${PROJECT_SOURCE_DIR}"
  )
  target_compile_options(nsis_dui_theme_tests PRIVATE /W4 /permissive- /utf-8)
  add_test(NAME theme COMMAND nsis_dui_theme_tests)

  add_executable(nsis_dui_renderer_tests tests/renderer_test.cpp)
  target_link_libraries(nsis_dui_renderer_tests PRIVATE nsis_dui_core)
  target_compile_definitions(nsis_dui_renderer_tests PRIVATE
    NSIS_DUI_SOURCE_DIR=L"${PROJECT_SOURCE_DIR}"
  )
  target_compile_options(nsis_dui_renderer_tests PRIVATE /W4 /permissive- /utf-8)
  add_test(NAME renderer COMMAND nsis_dui_renderer_tests)

  add_executable(nsis_dui_progress_tests tests/progress_test.cpp)
  target_link_libraries(nsis_dui_progress_tests PRIVATE nsis_dui_core)
  target_compile_options(nsis_dui_progress_tests PRIVATE /W4 /permissive- /utf-8)
  add_test(NAME progress COMMAND nsis_dui_progress_tests)
endif()
