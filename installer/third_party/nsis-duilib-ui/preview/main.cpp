#include "nsis_dui/theme.hpp"
#include "nsis_dui/window.hpp"

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>

#include <gdiplus.h>
#include <shellapi.h>
#include <windows.h>

namespace {

struct Options {
  std::filesystem::path theme =
      std::filesystem::path(NSIS_DUI_SOURCE_DIR) / "themes" / "onekey-modern";
  std::string locale = "zh-CN";
  std::string page = "installing";
  std::string version = "6.5.2";
  int progress = 42;
  std::filesystem::path screenshot;
  std::filesystem::path event_result;
};

[[nodiscard]] const char* EventName(nsis_dui::WindowEvent event) noexcept {
  switch (event) {
    case nsis_dui::WindowEvent::primary:
      return "primary";
    case nsis_dui::WindowEvent::primary_current:
      return "primary-current";
    case nsis_dui::WindowEvent::primary_all:
      return "primary-all";
    case nsis_dui::WindowEvent::browse:
      return "browse";
    case nsis_dui::WindowEvent::browse_current:
      return "browse-current";
    case nsis_dui::WindowEvent::browse_all:
      return "browse-all";
    case nsis_dui::WindowEvent::scope_current:
      return "scope-current";
    case nsis_dui::WindowEvent::scope_all:
      return "scope-all";
    case nsis_dui::WindowEvent::cancel:
      return "cancel";
    case nsis_dui::WindowEvent::closed:
      return "closed";
    default:
      return "none";
  }
}

[[nodiscard]] std::string WideToUtf8(const std::wstring& value) {
  if (value.empty()) {
    return {};
  }
  const int length = WideCharToMultiByte(
      CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0,
      nullptr, nullptr);
  std::string result(static_cast<std::size_t>(length), '\0');
  WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()),
                      result.data(), length, nullptr, nullptr);
  return result;
}

[[nodiscard]] Options ParseOptions() {
  Options options;
  int count = 0;
  LPWSTR* arguments = CommandLineToArgvW(GetCommandLineW(), &count);
  if (arguments == nullptr) {
    return options;
  }
  for (int index = 1; index + 1 < count; index += 2) {
    const std::wstring name = arguments[index];
    const std::wstring value = arguments[index + 1];
    if (name == L"--theme") {
      options.theme = value;
    } else if (name == L"--locale") {
      options.locale = WideToUtf8(value);
    } else if (name == L"--page") {
      options.page = WideToUtf8(value);
    } else if (name == L"--version") {
      options.version = WideToUtf8(value);
    } else if (name == L"--progress") {
      wchar_t* end = nullptr;
      const long parsed = wcstol(value.c_str(), &end, 10);
      if (end != value.c_str() && *end == L'\0') {
        options.progress = static_cast<int>(std::clamp(parsed, 0L, 100L));
      }
    } else if (name == L"--screenshot") {
      options.screenshot = value;
    } else if (name == L"--event-result") {
      options.event_result = value;
    }
  }
  LocalFree(arguments);
  return options;
}

[[nodiscard]] int PngEncoderClsid(CLSID& clsid) {
  UINT count = 0;
  UINT bytes = 0;
  Gdiplus::GetImageEncodersSize(&count, &bytes);
  if (bytes == 0) {
    return -1;
  }
  std::vector<unsigned char> storage(bytes);
  auto* encoders =
      reinterpret_cast<Gdiplus::ImageCodecInfo*>(storage.data());
  Gdiplus::GetImageEncoders(count, bytes, encoders);
  for (UINT index = 0; index < count; ++index) {
    if (wcscmp(encoders[index].MimeType, L"image/png") == 0) {
      clsid = encoders[index].Clsid;
      return static_cast<int>(index);
    }
  }
  return -1;
}

[[nodiscard]] bool CaptureWindow(HWND window,
                                 const std::filesystem::path& output) {
  RECT bounds = {};
  if (!GetWindowRect(window, &bounds)) {
    return false;
  }
  const int width = bounds.right - bounds.left;
  const int height = bounds.bottom - bounds.top;
  HDC source = GetWindowDC(window);
  HDC target = CreateCompatibleDC(source);
  HBITMAP bitmap = CreateCompatibleBitmap(source, width, height);
  const HGDIOBJ previous = SelectObject(target, bitmap);
  const BOOL copied = BitBlt(target, 0, 0, width, height, source, 0, 0,
                             SRCCOPY | CAPTUREBLT);
  SelectObject(target, previous);
  DeleteDC(target);
  ReleaseDC(window, source);

  bool saved = false;
  if (copied) {
    CLSID png = {};
    if (PngEncoderClsid(png) >= 0) {
      Gdiplus::Bitmap image(bitmap, nullptr);
      saved = image.Save(output.c_str(), &png, nullptr) == Gdiplus::Ok;
    }
  }
  DeleteObject(bitmap);
  return saved;
}

void ShowError(const std::string& error) {
  const int length = MultiByteToWideChar(
      CP_UTF8, 0, error.data(), static_cast<int>(error.size()), nullptr, 0);
  std::wstring message(static_cast<std::size_t>(length), L'\0');
  MultiByteToWideChar(CP_UTF8, 0, error.data(), static_cast<int>(error.size()),
                      message.data(), length);
  MessageBoxW(nullptr, message.c_str(), L"Theme Preview", MB_OK | MB_ICONERROR);
}

}  // namespace

int WINAPI wWinMain(HINSTANCE, HINSTANCE, PWSTR, int) {
  const Options options = ParseOptions();
  const auto loaded = nsis_dui::LoadTheme(options.theme, options.locale);
  if (!loaded) {
    ShowError(loaded.error);
    return 1;
  }

  nsis_dui::ViewState state;
  state.version = options.version;
  state.progress = options.progress;
  state.install_directory = "C:\\Users\\You\\AppData\\Local\\Programs\\OneKey";
  state.per_user_install_directory = state.install_directory;
  state.per_machine_install_directory = "C:\\Program Files\\OneKey";
  const auto status = loaded.package->strings.find("install.status");
  if (status != loaded.package->strings.end()) {
    state.status = status->second;
  }

  nsis_dui::InstallerWindow window;
  if (!window.Create(nullptr, *loaded.package, options.page, state)) {
    ShowError(window.error());
    return 2;
  }
  window.Show(true);

  if (!options.screenshot.empty()) {
    for (int frame = 0; frame < 10; ++frame) {
      window.PumpMessages();
      RedrawWindow(window.hwnd(), nullptr, nullptr,
                   RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN);
      Sleep(16);
    }
    const bool captured = CaptureWindow(window.hwnd(), options.screenshot);
    window.Close();
    window.PumpMessages();
    return captured ? 0 : 3;
  }

  if (!options.event_result.empty()) {
    const nsis_dui::WindowEvent event = window.WaitForEvent();
    std::ofstream output(options.event_result, std::ios::binary);
    if (!output) {
      return 4;
    }
    output << EventName(event) << '\n';
    return output ? 0 : 4;
  }

  return window.RunMessageLoop();
}
