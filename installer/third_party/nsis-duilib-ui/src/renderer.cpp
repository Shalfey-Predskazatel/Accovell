#include "nsis_dui/renderer.hpp"

#include <algorithm>
#include <fstream>
#include <stdexcept>
#include <unordered_map>

#include <windows.h>

namespace nsis_dui {
namespace {

[[nodiscard]] std::string ReadText(const std::filesystem::path& path) {
  std::ifstream input(path, std::ios::binary);
  if (!input) {
    throw std::runtime_error("cannot open " + path.u8string());
  }
  return {std::istreambuf_iterator<char>(input),
          std::istreambuf_iterator<char>()};
}

[[nodiscard]] std::wstring Utf8ToWide(const std::string& value) {
  if (value.empty()) {
    return {};
  }
  const int length = MultiByteToWideChar(
      CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
      static_cast<int>(value.size()), nullptr, 0);
  if (length <= 0) {
    throw std::runtime_error("theme text is not valid UTF-8");
  }
  std::wstring result(static_cast<std::size_t>(length), L'\0');
  if (MultiByteToWideChar(
          CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
          static_cast<int>(value.size()), result.data(), length) != length) {
    throw std::runtime_error("cannot convert theme text to UTF-16");
  }
  return result;
}

[[nodiscard]] std::string XmlEscape(const std::string& value) {
  std::string result;
  result.reserve(value.size());
  for (const char character : value) {
    switch (character) {
      case '&':
        result += "&amp;";
        break;
      case '<':
        result += "&lt;";
        break;
      case '>':
        result += "&gt;";
        break;
      case '\"':
        result += "&quot;";
        break;
      case '\'':
        result += "&apos;";
        break;
      default:
        result += character;
        break;
    }
  }
  return result;
}

[[nodiscard]] std::string DuiColor(const std::string& value) {
  return value.size() == 7 ? "#FF" + value.substr(1) : value;
}

[[nodiscard]] std::string FirstFontFamily(const std::string& value) {
  std::string result = value.substr(0, value.find(','));
  const auto first = result.find_first_not_of(" \t");
  const auto last = result.find_last_not_of(" \t");
  if (first == std::string::npos) {
    throw std::runtime_error("font family cannot be empty");
  }
  return result.substr(first, last - first + 1);
}

void ReplaceAll(std::string& value,
                const std::string& needle,
                const std::string& replacement) {
  std::size_t position = 0;
  while ((position = value.find(needle, position)) != std::string::npos) {
    value.replace(position, needle.size(), replacement);
    position += replacement.size();
  }
}

[[nodiscard]] std::unordered_map<std::string, std::string> BuildValues(
    const ThemePackage& package,
    const ViewState& state) {
  std::unordered_map<std::string, std::string> values;
  for (const auto& [name, color] : package.colors) {
    values.emplace("color." + name, DuiColor(color));
  }
  for (const auto& [name, text] : package.strings) {
    std::string resolved = text;
    ReplaceAll(resolved, "{version}", state.version);
    values.emplace("string." + name, std::move(resolved));
  }

  values.emplace("font.family", FirstFontFamily(package.typography.font_family));
  values.emplace("font.titleSize", std::to_string(package.typography.title_size));
  values.emplace("font.bodySize", std::to_string(package.typography.body_size));
  values.emplace("window.width", std::to_string(package.window.width));
  values.emplace("window.height", std::to_string(package.window.height));
  values.emplace("window.radius", std::to_string(package.window.radius));
  values.emplace("runtime.version", state.version);
  values.emplace("runtime.status", state.status);
  values.emplace("runtime.installDirectory", state.install_directory);
  values.emplace("runtime.perUserInstallDirectory",
                 state.per_user_install_directory);
  values.emplace("runtime.perMachineInstallDirectory",
                 state.per_machine_install_directory);
  values.emplace("runtime.progress", std::to_string(std::clamp(state.progress, 0, 100)));

  if (!package.features.empty()) {
    const FeatureCard& feature =
        package.features[state.feature_index % package.features.size()];
    values.emplace("feature.title", feature.title);
    values.emplace("feature.description", feature.description);
    values.emplace("feature.position",
                   std::to_string((state.feature_index % package.features.size()) + 1));
    values.emplace("feature.count", std::to_string(package.features.size()));
  } else {
    values.emplace("feature.title", "");
    values.emplace("feature.description", "");
    values.emplace("feature.position", "0");
    values.emplace("feature.count", "0");
  }
  return values;
}

[[nodiscard]] std::string Expand(
    std::string source,
    const std::unordered_map<std::string, std::string>& values) {
  std::size_t position = 0;
  while ((position = source.find("{{", position)) != std::string::npos) {
    const std::size_t end = source.find("}}", position + 2);
    if (end == std::string::npos) {
      throw std::runtime_error("unterminated theme placeholder");
    }
    const std::string key = source.substr(position + 2, end - position - 2);
    const auto value = values.find(key);
    if (value == values.end()) {
      throw std::runtime_error("unknown theme placeholder: " + key);
    }
    const std::string escaped = XmlEscape(value->second);
    source.replace(position, end + 2 - position, escaped);
    position += escaped.size();
  }
  return source;
}

}  // namespace

RenderResult RenderPage(const ThemePackage& package,
                        const std::string& page_name,
                        const ViewState& state) noexcept {
  try {
    const auto page = package.pages.find(page_name);
    if (page == package.pages.end()) {
      throw std::runtime_error("theme does not define page: " + page_name);
    }
    return RenderResult{
        Utf8ToWide(Expand(ReadText(page->second), BuildValues(package, state))),
        {}};
  } catch (const std::exception& error) {
    return RenderResult{std::nullopt, error.what()};
  }
}

}  // namespace nsis_dui
