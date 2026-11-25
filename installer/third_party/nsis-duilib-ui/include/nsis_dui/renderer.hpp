#pragma once

#include "nsis_dui/theme.hpp"

#include <optional>
#include <string>

namespace nsis_dui {

struct ViewState {
  std::string version;
  std::string status;
  std::string install_directory;
  std::string per_user_install_directory;
  std::string per_machine_install_directory;
  int progress = 0;
  std::size_t feature_index = 0;
};

struct RenderResult {
  std::optional<std::wstring> xml;
  std::string error;

  [[nodiscard]] explicit operator bool() const noexcept {
    return xml.has_value();
  }
};

[[nodiscard]] RenderResult RenderPage(
    const ThemePackage& package,
    const std::string& page_name,
    const ViewState& state = {}) noexcept;

}  // namespace nsis_dui
