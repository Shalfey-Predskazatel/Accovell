#include "nsis_dui/version.hpp"
#include "nsis_dui/window.hpp"
#include "nsis_dui/progress.hpp"

#include <algorithm>
#include <filesystem>
#include <iterator>
#include <memory>
#include <string>

#include <windows.h>
#include <commctrl.h>

#include "pluginapi.h"

namespace {

HMODULE g_module = nullptr;
std::unique_ptr<nsis_dui::InstallerWindow> g_window;
HWND g_parent = nullptr;
constexpr UINT_PTR kProgressTimer = 0x4F4B;
int g_last_progress = -1;
nsis_dui::ProgressTracker g_progress;
bool g_cancel_forwarded = false;
bool g_restore_native_on_unload = true;
ULONGLONG g_cancel_requested_at = 0;
ULONGLONG g_last_poll_at = 0;

struct NativeHostState {
  HWND window = nullptr;
  bool subclassed = false;
};

NativeHostState g_native_host;
constexpr UINT_PTR kNativeHostSubclass = 0x4F4B4455;

[[nodiscard]] std::wstring Utf8ToWide(const std::string& value) noexcept {
  if (value.empty()) {
    return {};
  }
  const int length = MultiByteToWideChar(
      CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
      static_cast<int>(value.size()), nullptr, 0);
  if (length <= 0) {
    return L"invalid UTF-8 error text";
  }
  std::wstring result(static_cast<std::size_t>(length), L'\0');
  MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
                      static_cast<int>(value.size()), result.data(), length);
  return result;
}

[[nodiscard]] std::string WideToUtf8(const std::wstring& value) {
  if (value.empty()) {
    return {};
  }
  const int length = WideCharToMultiByte(
      CP_UTF8, WC_ERR_INVALID_CHARS, value.data(),
      static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
  if (length <= 0) {
    return {};
  }
  std::string result(static_cast<std::size_t>(length), '\0');
  WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value.data(),
                      static_cast<int>(value.size()), result.data(), length,
                      nullptr, nullptr);
  return result;
}

class Stack final {
 public:
  Stack(int string_size, stack_t** top) noexcept
      : string_size_(string_size > 0 ? string_size : 1), top_(top) {}

  [[nodiscard]] bool Pop(std::wstring& value) noexcept {
    if (top_ == nullptr || *top_ == nullptr) {
      return false;
    }
    stack_t* item = *top_;
    value.assign(item->text);
    *top_ = item->next;
    GlobalFree(item);
    return true;
  }

  void Push(const std::wstring& value) noexcept {
    if (top_ == nullptr) {
      return;
    }
    const std::size_t bytes =
        sizeof(stack_t) + static_cast<std::size_t>(string_size_) * sizeof(wchar_t);
    auto* item = static_cast<stack_t*>(GlobalAlloc(GPTR, bytes));
    if (item == nullptr) {
      return;
    }
    lstrcpynW(item->text, value.c_str(), string_size_);
    item->next = *top_;
    *top_ = item;
  }

  void Ok() noexcept { Push(L"ok"); }

  void Error(const std::string& error) noexcept {
    Push(L"error:" + Utf8ToWide(error));
  }

 private:
  int string_size_;
  stack_t** top_;
};

LRESULT CALLBACK NativeHostSubclassProc(HWND window, UINT message,
                                        WPARAM wparam, LPARAM lparam,
                                        UINT_PTR subclass_id,
                                        DWORD_PTR) noexcept {
  if (message == WM_SHOWWINDOW && wparam != FALSE) {
    ShowWindow(window, SW_HIDE);
    return 0;
  }
  if (message == WM_WINDOWPOSCHANGING) {
    auto* position = reinterpret_cast<WINDOWPOS*>(lparam);
    if (position != nullptr && (position->flags & SWP_SHOWWINDOW) != 0) {
      position->flags &= ~SWP_SHOWWINDOW;
      position->flags |= SWP_HIDEWINDOW;
    }
  } else if (message == WM_WINDOWPOSCHANGED && IsWindowVisible(window)) {
    ShowWindow(window, SW_HIDE);
  } else if (message == WM_NCDESTROY) {
    RemoveWindowSubclass(window, NativeHostSubclassProc, subclass_id);
    if (g_native_host.window == window) {
      g_native_host = {};
    }
  }
  return DefSubclassProc(window, message, wparam, lparam);
}

void RestoreNativeHost() noexcept {
  if (g_native_host.subclassed && g_native_host.window != nullptr &&
      IsWindow(g_native_host.window)) {
    RemoveWindowSubclass(g_native_host.window, NativeHostSubclassProc,
                         kNativeHostSubclass);
  }
  g_native_host = {};
}

void ConcealNativeHost(HWND parent) noexcept {
  RestoreNativeHost();
  if (parent == nullptr || !IsWindow(parent)) {
    return;
  }

  g_native_host.window = parent;
  g_native_host.subclassed =
      SetWindowSubclass(parent, NativeHostSubclassProc,
                        kNativeHostSubclass, 0) != FALSE;
  ShowWindow(parent, SW_HIDE);
}

void StopProgressTimer() noexcept {
  if (g_parent != nullptr && IsWindow(g_parent)) {
    KillTimer(g_parent, kProgressTimer);
  }
  g_parent = nullptr;
  g_last_progress = -1;
  (void)g_progress.ResetForNative();
  g_cancel_forwarded = false;
  g_cancel_requested_at = 0;
  g_last_poll_at = 0;
}

struct ProgressSearch {
  HWND window = nullptr;
  int width = -1;
};

BOOL CALLBACK FindProgressChild(HWND child, LPARAM parameter) {
  wchar_t class_name[64] = {};
  GetClassNameW(child, class_name, static_cast<int>(std::size(class_name)));
  if (_wcsicmp(class_name, PROGRESS_CLASSW) == 0) {
    RECT bounds = {};
    GetWindowRect(child, &bounds);
    const int width = bounds.right - bounds.left;
    auto* result = reinterpret_cast<ProgressSearch*>(parameter);
    if (width > result->width) {
      result->window = child;
      result->width = width;
    }
  }
  return TRUE;
}

[[nodiscard]] HWND FindProgressWindow(HWND parent) noexcept {
  ProgressSearch result;
  EnumChildWindows(parent, FindProgressChild,
                   reinterpret_cast<LPARAM>(&result));
  return result.window;
}

void CALLBACK ProgressTimerProc(HWND, UINT, UINT_PTR, DWORD) {
  if (!g_window || g_parent == nullptr || !IsWindow(g_parent)) {
    return;
  }

  const nsis_dui::WindowEvent event = g_window->event();
  ShowWindow(g_parent, SW_HIDE);
  if ((event == nsis_dui::WindowEvent::cancel ||
       event == nsis_dui::WindowEvent::closed) &&
      !g_cancel_forwarded) {
    const ULONGLONG now = GetTickCount64();
    if (g_last_poll_at != 0 && now - g_last_poll_at <= 250) {
      return;
    }
    if (g_cancel_requested_at == 0) {
      g_cancel_requested_at = now;
      return;
    }
    if (now - g_cancel_requested_at < 160) {
      return;
    }
    g_cancel_forwarded = true;
    PostMessageW(g_parent, WM_COMMAND, IDCANCEL, 0);
    return;
  }
  if (!g_cancel_forwarded) {
    g_cancel_requested_at = 0;
  }

  if (!g_progress.AllowsNativeUpdates()) {
    return;
  }

  const HWND progress_window = FindProgressWindow(g_parent);
  if (progress_window == nullptr) {
    return;
  }
  const LRESULT minimum = SendMessageW(progress_window, PBM_GETRANGE, TRUE, 0);
  const LRESULT maximum = SendMessageW(progress_window, PBM_GETRANGE, FALSE, 0);
  const LRESULT position = SendMessageW(progress_window, PBM_GETPOS, 0, 0);
  if (maximum <= minimum) {
    return;
  }
  const int progress = static_cast<int>(
      std::clamp((position - minimum) * 100 / (maximum - minimum),
                 static_cast<LRESULT>(0), static_cast<LRESULT>(100)));
  const int monotonic_progress = g_progress.Update(progress);
  if (monotonic_progress != g_last_progress) {
    g_last_progress = monotonic_progress;
    g_window->SetProgress(monotonic_progress);
    g_window->Refresh();
  }
}

void StartProgressTimer(HWND parent) noexcept {
  StopProgressTimer();
  if (parent != nullptr && IsWindow(parent)) {
    g_parent = parent;
    SetTimer(parent, kProgressTimer, 80, ProgressTimerProc);
  }
}

void DestroyInstallerWindow(bool restore_native_host = true) noexcept {
  StopProgressTimer();
  if (g_window) {
    g_window->Close();
    g_window->PumpMessages();
    g_window.reset();
  }
  if (restore_native_host) {
    RestoreNativeHost();
  }
}

UINT_PTR PluginCallback(NSPIM message) {
  if (message == NSPIM_UNLOAD || message == NSPIM_GUIUNLOAD) {
    DestroyInstallerWindow(g_restore_native_on_unload);
  }
  return 0;
}

void RegisterUnloadCallback(extra_parameters* extra) noexcept {
  if (extra != nullptr && extra->RegisterPluginCallback != nullptr &&
      g_module != nullptr) {
    extra->RegisterPluginCallback(g_module, PluginCallback);
  }
}

#define NSIS_ENTRY(name)                                                      \
  extern "C" void name(                                                     \
      HWND hwnd_parent, int string_size, LPTSTR, stack_t** stack_top,         \
      extra_parameters* extra)

}  // namespace

NSIS_ENTRY(Init) {
  Stack stack(string_size, stack_top);
  try {
    std::wstring theme_root;
    std::wstring locale;
    std::wstring version;
    if (!stack.Pop(theme_root) || !stack.Pop(locale) || !stack.Pop(version)) {
      stack.Error("Init expects themeRoot, locale, and version");
      return;
    }

    DestroyInstallerWindow();
    g_restore_native_on_unload = true;
    const auto loaded =
        nsis_dui::LoadTheme(std::filesystem::path(theme_root), WideToUtf8(locale));
    if (!loaded) {
      stack.Error(loaded.error);
      return;
    }

    nsis_dui::ViewState state;
    state.version = WideToUtf8(version);
    const auto status = loaded.package->strings.find("install.status");
    if (status != loaded.package->strings.end()) {
      state.status = status->second;
    }

    auto window = std::make_unique<nsis_dui::InstallerWindow>();
    if (!window->Create(hwnd_parent, *loaded.package, "installing", state)) {
      stack.Error(window->error());
      return;
    }
    g_window = std::move(window);
    ConcealNativeHost(hwnd_parent);
    StartProgressTimer(hwnd_parent);
    RegisterUnloadCallback(extra);
    stack.Ok();
  } catch (const std::exception& error) {
    DestroyInstallerWindow();
    stack.Error(error.what());
  } catch (...) {
    DestroyInstallerWindow();
    stack.Error("unexpected initialization failure");
  }
}

NSIS_ENTRY(Show) {
  Stack stack(string_size, stack_top);
  if (!g_window || g_window->hwnd() == nullptr) {
    stack.Error("plugin is not initialized");
    return;
  }
  g_window->Show(true);
  g_restore_native_on_unload = true;
  g_window->PumpMessages();
  stack.Ok();
}

NSIS_ENTRY(Hide) {
  Stack stack(string_size, stack_top);
  if (!g_window || g_window->hwnd() == nullptr) {
    stack.Error("plugin is not initialized");
    return;
  }
  g_window->Hide();
  g_window->PumpMessages();
  g_restore_native_on_unload = false;
  stack.Ok();
}

NSIS_ENTRY(ConcealHost) {
  Stack stack(string_size, stack_top);
  ConcealNativeHost(hwnd_parent);
  if (g_parent != hwnd_parent) {
    StartProgressTimer(hwnd_parent);
  }
  stack.Ok();
}

NSIS_ENTRY(SetPage) {
  Stack stack(string_size, stack_top);
  std::wstring page;
  if (!g_window || !stack.Pop(page)) {
    stack.Error("SetPage expects an initialized plugin and page name");
    return;
  }
  if (!g_window->SetPage(WideToUtf8(page))) {
    stack.Error(g_window->error());
    return;
  }
  g_window->PumpMessages();
  stack.Ok();
}

NSIS_ENTRY(SetProgress) {
  Stack stack(string_size, stack_top);
  std::wstring progress;
  if (!g_window || !stack.Pop(progress)) {
    stack.Error("SetProgress expects an initialized plugin and value");
    return;
  }
  wchar_t* end = nullptr;
  const long value = wcstol(progress.c_str(), &end, 10);
  if (end == progress.c_str() || *end != L'\0') {
    stack.Error("progress must be an integer");
    return;
  }
  const int progress_value = g_progress.UpdateManual(
      static_cast<int>(std::clamp(value, 0L, 100L)));
  g_window->SetProgress(progress_value);
  g_window->PumpMessages();
  stack.Ok();
}

NSIS_ENTRY(ResetProgress) {
  Stack stack(string_size, stack_top);
  std::wstring progress;
  if (!g_window || !stack.Pop(progress)) {
    stack.Error("ResetProgress expects an initialized plugin and value");
    return;
  }
  wchar_t* end = nullptr;
  const long value = wcstol(progress.c_str(), &end, 10);
  if (end == progress.c_str() || *end != L'\0') {
    stack.Error("progress must be an integer");
    return;
  }
  const int progress_value = g_progress.ResetForNative(
      static_cast<int>(std::clamp(value, 0L, 100L)));
  g_last_progress = progress_value;
  g_window->ResetProgress(progress_value);
  g_window->PumpMessages();
  stack.Ok();
}

NSIS_ENTRY(SetInstallDirectory) {
  Stack stack(string_size, stack_top);
  std::wstring install_directory;
  if (!g_window || !stack.Pop(install_directory)) {
    stack.Error("SetInstallDirectory expects an initialized plugin and path");
    return;
  }
  g_window->SetInstallDirectory(WideToUtf8(install_directory));
  g_window->PumpMessages();
  stack.Ok();
}

NSIS_ENTRY(SetPerUserInstallDirectory) {
  Stack stack(string_size, stack_top);
  std::wstring install_directory;
  if (!g_window || !stack.Pop(install_directory)) {
    stack.Error(
        "SetPerUserInstallDirectory expects an initialized plugin and path");
    return;
  }
  g_window->SetPerUserInstallDirectory(WideToUtf8(install_directory));
  g_window->PumpMessages();
  stack.Ok();
}

NSIS_ENTRY(SetPerMachineInstallDirectory) {
  Stack stack(string_size, stack_top);
  std::wstring install_directory;
  if (!g_window || !stack.Pop(install_directory)) {
    stack.Error(
        "SetPerMachineInstallDirectory expects an initialized plugin and path");
    return;
  }
  g_window->SetPerMachineInstallDirectory(WideToUtf8(install_directory));
  g_window->PumpMessages();
  stack.Ok();
}

NSIS_ENTRY(SetStatus) {
  Stack stack(string_size, stack_top);
  std::wstring status;
  if (!g_window || !stack.Pop(status)) {
    stack.Error("SetStatus expects an initialized plugin and text");
    return;
  }
  g_window->SetStatus(WideToUtf8(status));
  g_window->PumpMessages();
  stack.Ok();
}

NSIS_ENTRY(SetControlText) {
  Stack stack(string_size, stack_top);
  std::wstring name;
  std::wstring text;
  if (!g_window || !stack.Pop(name) || !stack.Pop(text)) {
    stack.Error("SetControlText expects an initialized plugin, control name, and text");
    return;
  }
  g_window->SetControlText(WideToUtf8(name), WideToUtf8(text));
  g_window->PumpMessages();
  stack.Ok();
}

NSIS_ENTRY(SetVersion) {
  Stack stack(string_size, stack_top);
  std::wstring version;
  if (!g_window || !stack.Pop(version)) {
    stack.Error("SetVersion expects an initialized plugin and value");
    return;
  }
  g_window->SetVersion(WideToUtf8(version));
  g_window->PumpMessages();
  stack.Ok();
}

NSIS_ENTRY(PollEvent) {
  Stack stack(string_size, stack_top);
  if (!g_window) {
    stack.Push(L"closed");
    return;
  }
  g_last_poll_at = GetTickCount64();
  g_window->PumpMessages();
  switch (g_window->event()) {
    case nsis_dui::WindowEvent::primary:
      stack.Push(L"primary");
      break;
    case nsis_dui::WindowEvent::primary_current:
      stack.Push(L"primary-current");
      break;
    case nsis_dui::WindowEvent::primary_all:
      stack.Push(L"primary-all");
      break;
    case nsis_dui::WindowEvent::language:
      g_window->ResetEvent();
      stack.Push(L"language");
      break;
    case nsis_dui::WindowEvent::theme:
      g_window->ResetEvent();
      stack.Push(L"theme");
      break;
    case nsis_dui::WindowEvent::sound:
      g_window->ResetEvent();
      stack.Push(L"sound");
      break;
    case nsis_dui::WindowEvent::browse:
      g_window->ResetEvent();
      stack.Push(L"browse");
      break;
    case nsis_dui::WindowEvent::browse_current:
      g_window->ResetEvent();
      stack.Push(L"browse-current");
      break;
    case nsis_dui::WindowEvent::browse_all:
      g_window->ResetEvent();
      stack.Push(L"browse-all");
      break;
    case nsis_dui::WindowEvent::scope_current:
      g_window->ResetEvent();
      stack.Push(L"scope-current");
      break;
    case nsis_dui::WindowEvent::scope_all:
      g_window->ResetEvent();
      stack.Push(L"scope-all");
      break;
    case nsis_dui::WindowEvent::cancel:
      stack.Push(L"cancel");
      break;
    case nsis_dui::WindowEvent::closed:
      stack.Push(L"closed");
      break;
    default:
      stack.Push(L"none");
      break;
  }
}

NSIS_ENTRY(WaitForEvent) {
  Stack stack(string_size, stack_top);
  if (!g_window) {
    stack.Push(L"closed");
    return;
  }
  switch (g_window->WaitForEvent()) {
    case nsis_dui::WindowEvent::primary:
      stack.Push(L"primary");
      break;
    case nsis_dui::WindowEvent::primary_current:
      stack.Push(L"primary-current");
      break;
    case nsis_dui::WindowEvent::primary_all:
      stack.Push(L"primary-all");
      break;
    case nsis_dui::WindowEvent::language:
      g_window->ResetEvent();
      stack.Push(L"language");
      break;
    case nsis_dui::WindowEvent::theme:
      g_window->ResetEvent();
      stack.Push(L"theme");
      break;
    case nsis_dui::WindowEvent::sound:
      g_window->ResetEvent();
      stack.Push(L"sound");
      break;
    case nsis_dui::WindowEvent::browse:
      g_window->ResetEvent();
      stack.Push(L"browse");
      break;
    case nsis_dui::WindowEvent::browse_current:
      g_window->ResetEvent();
      stack.Push(L"browse-current");
      break;
    case nsis_dui::WindowEvent::browse_all:
      g_window->ResetEvent();
      stack.Push(L"browse-all");
      break;
    case nsis_dui::WindowEvent::scope_current:
      g_window->ResetEvent();
      stack.Push(L"scope-current");
      break;
    case nsis_dui::WindowEvent::scope_all:
      g_window->ResetEvent();
      stack.Push(L"scope-all");
      break;
    case nsis_dui::WindowEvent::cancel:
      stack.Push(L"cancel");
      break;
    default:
      stack.Push(L"closed");
      break;
  }
}

NSIS_ENTRY(Shutdown) {
  Stack stack(string_size, stack_top);
  DestroyInstallerWindow();
  g_restore_native_on_unload = true;
  stack.Ok();
}

NSIS_ENTRY(ShutdownHidden) {
  Stack stack(string_size, stack_top);
  DestroyInstallerWindow(false);
  g_restore_native_on_unload = false;
  stack.Ok();
}

NSIS_ENTRY(NsisDuiGetVersion) {
  Stack stack(string_size, stack_top);
  stack.Push(Utf8ToWide(std::string(nsis_dui::Version())));
}

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID) {
  if (reason == DLL_PROCESS_ATTACH) {
    g_module = instance;
    DisableThreadLibraryCalls(instance);
  }
  return TRUE;
}
