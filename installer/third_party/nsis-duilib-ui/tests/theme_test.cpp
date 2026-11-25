#include "nsis_dui/theme.hpp"

#include <array>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <regex>

namespace {

int Fail(const std::string& message) {
  std::cerr << message << '\n';
  return EXIT_FAILURE;
}

}  // namespace

int main() {
  const std::filesystem::path source_root = NSIS_DUI_SOURCE_DIR;
  const auto loaded =
      nsis_dui::LoadTheme(source_root / "themes" / "onekey-modern", "zh-CN");
  if (!loaded) {
    return Fail("valid theme failed: " + loaded.error);
  }
  if (loaded.package->id != "onekey-modern" ||
      loaded.package->locale != "zh-CN" ||
      loaded.package->pages.count("installing") != 1 ||
      loaded.package->pages.count("confirmExit") != 1 ||
      loaded.package->colors.at("brand") != "#44D62C" ||
      loaded.package->features.size() != 3 ||
      !loaded.package->features.front().action_url ||
      loaded.package->features.front().action_url->rfind("https://", 0) != 0) {
    return Fail("valid theme values were not loaded correctly");
  }

  const std::regex button_pattern(R"(<Button\b[^>]*>)");
  for (const auto& [page_name, page_path] : loaded.package->pages) {
    std::ifstream input(page_path, std::ios::binary);
    if (!input) {
      return Fail("could not read page while checking hover states: " +
                  page_name);
    }
    const std::string markup((std::istreambuf_iterator<char>(input)),
                             std::istreambuf_iterator<char>());
    for (std::sregex_iterator button(markup.begin(), markup.end(),
                                     button_pattern),
         end;
         button != end; ++button) {
      const std::string tag = button->str();
      if (tag.find("hotbkcolor=") == std::string::npos &&
          tag.find("hottextcolor=") == std::string::npos) {
        return Fail("button without a hover state in page: " + page_name);
      }
    }
  }

  const auto underscored = nsis_dui::LoadTheme(
      source_root / "themes" / "onekey-modern", "ZH_cn");
  if (!underscored || underscored.package->locale != "zh-CN") {
    return Fail("locale normalization did not select zh-CN");
  }

  const auto regional = nsis_dui::LoadTheme(
      source_root / "themes" / "onekey-modern", "zh-Hant-MO");
  if (!regional || regional.package->locale != "zh-TW") {
    return Fail("regional locale fallback did not select zh-TW");
  }

  constexpr std::array<const char*, 20> supported_locales{
      "bn",    "de",    "en",    "en-US", "es",    "fr-FR", "hi-IN",
      "id",    "it-IT", "ja-JP", "ko-KR", "pt",    "pt-BR", "ru",
      "th-TH", "uk-UA", "vi",    "zh-CN", "zh-HK", "zh-TW"};
  for (const char* locale : supported_locales) {
    const auto localized = nsis_dui::LoadTheme(
        source_root / "themes" / "onekey-modern", locale);
    if (!localized || localized.package->locale != locale ||
        localized.package->strings.at("welcome.start").empty() ||
        localized.package->strings.at("welcome.install").empty() ||
        localized.package->strings.at("welcome.scopeLabel").empty() ||
        localized.package->strings.at("welcome.pathLabel").empty() ||
        localized.package->strings.at("scope.recommended").empty() ||
        localized.package->strings.at("upgrade.install").empty()) {
      return Fail(std::string("supported locale failed: ") + locale);
    }
  }

  const auto escaped = nsis_dui::LoadTheme(
      source_root / "tests" / "fixtures" / "path-traversal");
  if (escaped || escaped.error.find("cannot leave") == std::string::npos) {
    return Fail("path traversal fixture was not rejected");
  }
  return EXIT_SUCCESS;
}
