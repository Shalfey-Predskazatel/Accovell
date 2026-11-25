#pragma once

#include "nsis_dui\renderer.hpp"
#include "nsis_dui\theme.hpp"

#include <memory>
#include <string>

#include <windows.h>

namespace nsis_dui {

enum class WindowEvent {
  none,
  primary,
  primary_current,
  primary_all,
  language,
  theme,
  sound,
  browse,
  browse_current,
  browse_all,
  scope_current,
  scope_all,
  cancel,
  closed,
};

class InstallerWindow final {
 public:
  InstallerWindow();
  ~InstallerWindow();

  InstallerWindow(const InstallerWindow&) = delete;
  InstallerWindow& operator=(const InstallerWindow&) = delete;

  [[nodiscard]] bool Create(HWND owner,
                            const ThemePackage& package,
                            const std::string& page_name,
                            const ViewState& state) noexcept;
  void Show(bool activate = true) noexcept;
  void Hide() noexcept;
  void Refresh() noexcept;
  void Close() noexcept;
  [[nodiscard]] int RunMessageLoop() noexcept;
  [[nodiscard]] WindowEvent WaitForEvent() noexcept;
  void PumpMessages() noexcept;

  [[nodiscard]] bool SetPage(const std::string& page_name) noexcept;
  void SetProgress(int progress) noexcept;
  void ResetProgress(int progress = 0) noexcept;
  void SetStatus(const std::string& status) noexcept;
  void SetControlText(const std::string& name,
                      const std::string& text) noexcept;
  void SetVersion(const std::string& version) noexcept;
  void SetInstallDirectory(const std::string& install_directory) noexcept;
  void SetPerUserInstallDirectory(const std::string& install_directory) noexcept;
  void SetPerMachineInstallDirectory(const std::string& install_directory) noexcept;
  void ResetEvent() noexcept;
  void NextFeature(int direction = 1) noexcept;

  [[nodiscard]] HWND hwnd() const noexcept;
  [[nodiscard]] WindowEvent event() const noexcept;
  [[nodiscard]] const std::string& error() const noexcept;

 private:
  class Impl;
  std::unique_ptr<Impl> impl_;
};

}  // namespace nsis_dui
