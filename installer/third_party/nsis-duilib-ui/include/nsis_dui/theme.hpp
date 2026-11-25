#pragma once

#include <filesystem>
#include <optional>
#include <string>
#include <unordered_map>
#include <vector>

namespace nsis_dui {

inline constexpr int kThemeSchemaVersion = 1;

struct ThemeWindow {
  int width = 0;
  int height = 0;
  int radius = 0;
};

struct ThemeTypography {
  std::string font_family;
  int title_size = 0;
  int body_size = 0;
};

struct FeatureCard {
  std::string id;
  std::string title;
  std::string description;
  std::optional<std::filesystem::path> image;
  std::optional<std::string> action_url;
};

struct ThemePackage {
  std::filesystem::path root;
  std::string id;
  std::string name;
  std::string locale;
  ThemeWindow window;
  ThemeTypography typography;
  std::unordered_map<std::string, std::filesystem::path> pages;
  std::unordered_map<std::string, std::string> colors;
  std::unordered_map<std::string, std::string> strings;
  std::vector<FeatureCard> features;
};

struct ThemeLoadResult {
  std::optional<ThemePackage> package;
  std::string error;

  [[nodiscard]] explicit operator bool() const noexcept {
    return package.has_value();
  }
};

[[nodiscard]] ThemeLoadResult LoadTheme(
    const std::filesystem::path& theme_root,
    const std::string& preferred_locale = "en-US") noexcept;

}  // namespace nsis_dui

