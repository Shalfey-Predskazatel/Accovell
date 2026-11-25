set(DUILIB_ROOT "${PROJECT_SOURCE_DIR}/third_party/duilib")

file(GLOB_RECURSE DUILIB_SOURCES CONFIGURE_DEPENDS
  "${DUILIB_ROOT}/*.cpp"
)

# The plugin owns the process entry point. The upstream entry point is only
# needed when DuiLib is built as a standalone DLL.
list(FILTER DUILIB_SOURCES EXCLUDE REGEX "/UILib\\.cpp$")
list(FILTER DUILIB_SOURCES EXCLUDE REGEX "/Control/UIFlash\\.cpp$")
list(FILTER DUILIB_SOURCES EXCLUDE REGEX "/Control/UIWebBrowser\\.cpp$")

add_library(duilib STATIC ${DUILIB_SOURCES})

target_include_directories(duilib PUBLIC
  "${DUILIB_ROOT}"
  "${DUILIB_ROOT}/Control"
  "${DUILIB_ROOT}/Core"
  "${DUILIB_ROOT}/Layout"
  "${DUILIB_ROOT}/Utils"
)
target_compile_definitions(duilib PUBLIC
  UILIB_STATIC
  UNICODE
  _UNICODE
  _WIN32_WINNT=0x0A00
  WINVER=0x0A00
)
target_compile_options(duilib PRIVATE
  /W3
  /utf-8
  /wd4100
  /wd4244
  /wd4267
  /wd4456
  /wd4458
  /wd4706
  /wd4828
)
target_link_libraries(duilib PUBLIC
  comctl32
  gdiplus
  imm32
  ole32
  oleaut32
  shlwapi
  uuid
  winmm
  ws2_32
)
set_target_properties(duilib PROPERTIES
  CXX_STANDARD 14
  CXX_STANDARD_REQUIRED ON
  CXX_EXTENSIONS OFF
)
