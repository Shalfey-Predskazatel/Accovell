#include "nsis_dui/theme.hpp"

#include "nsis_dui/version.hpp"

#include <algorithm>
#include <array>
#include <cctype>
#include <fstream>
#include <stdexcept>
#include <string_view>

#include <nlohmann/json.hpp>

namespace nsis_dui {
namespace {

using Json = nlohmann::json;

struct VersionParts {
  int major = 0;
  int minor = 0;
  int patch = 0;
};

[[nodiscard]] Json ReadJson(const std::filesystem::path& path) {
  std::ifstream input(path, std::ios::binary);
  if (!input) {
    throw std::runtime_error("cannot open " + path.u8string());
  }

  Json value;
  input >> value;
  return value;
}

[[nodiscard]] VersionParts ParseVersion(const std::string& value) {
  VersionParts result;
  std::array<int*, 3> outputs{&result.major, &result.minor, &result.patch};
  std::size_t start = 0;

  for (std::size_t index = 0; index < outputs.size(); ++index) {
    const std::size_t end = value.find('.', start);
    const std::string part = value.substr(start, end - start);
    if (part.empty() ||
        !std::all_of(part.begin(), part.end(), [](unsigned char character) {
          return std::isdigit(character) != 0;
        })) {
      throw std::runtime_error("invalid semantic version: " + value);
    }
    *outputs[index] = std::stoi(part);
    if (index + 1 < outputs.size()) {
      if (end == std::string::npos) {
        throw std::runtime_error("invalid semantic version: " + value);
      }
      start = end + 1;
    } else if (end != std::string::npos) {
      throw std::runtime_error("invalid semantic version: " + value);
    }
  }
  return result;
}

[[nodiscard]] bool IsNewer(const VersionParts& required,
                           const VersionParts& current) noexcept {
  if (required.major != current.major) {
    return required.major > current.major;
  }
  if (required.minor != current.minor) {
    return required.minor > current.minor;
  }
  return required.patch > current.patch;
}

[[nodiscard]] std::filesystem::path ResolveFile(
    const std::filesystem::path& root,
    const std::string& relative_value,
    const std::string& field_name) {
  const std::filesystem::path relative =
      std::filesystem::u8path(relative_value).lexically_normal();
  if (relative.empty() || relative.is_absolute()) {
    throw std::runtime_error(field_name + " must be a relative file path");
  }
  for (const auto& component : relative) {
    if (component == "..") {
      throw std::runtime_error(field_name + " cannot leave the theme directory");
    }
  }

  const std::filesystem::path candidate = root / relative;
  if (!std::filesystem::is_regular_file(candidate)) {
    throw std::runtime_error(field_name + " does not exist: " + relative_value);
  }

  std::error_code error;
  const auto canonical_root = std::filesystem::weakly_canonical(root, error);
  if (error) {
    throw std::runtime_error("cannot resolve theme directory");
  }
  const auto canonical_candidate =
      std::filesystem::weakly_canonical(candidate, error);
  if (error) {
    throw std::runtime_error("cannot resolve " + field_name);
  }
  const auto relative_to_root =
      std::filesystem::relative(canonical_candidate, canonical_root, error);
  if (error || relative_to_root.empty() || relative_to_root.is_absolute() ||
      *relative_to_root.begin() == "..") {
    throw std::runtime_error(field_name + " cannot leave the theme directory");
  }
  return canonical_candidate;
}

[[nodiscard]] bool IsColor(const std::string& value) noexcept {
  if (value.size() != 7 && value.size() != 9) {
    return false;
  }
  if (value.front() != '#') {
    return false;
  }
  return std::all_of(value.begin() + 1, value.end(), [](unsigned char value) {
    return std::isxdigit(value) != 0;
  });
}

void LoadTokens(const std::filesystem::path& path, ThemePackage& package) {
  const Json tokens = ReadJson(path);
  const Json& colors = tokens.at("colors");
  constexpr std::array<std::string_view, 5> required_colors{
      "brand", "background", "surface", "textPrimary", "textSecondary"};

  if (!colors.is_object()) {
    throw std::runtime_error("colors must be an object");
  }
  for (const auto& [color_name, color_value] : colors.items()) {
    const std::string value = color_value.get<std::string>();
    if (!IsColor(value)) {
      throw std::runtime_error("invalid color token: " + color_name);
    }
    package.colors.emplace(color_name, value);
  }
  for (const auto color_name : required_colors) {
    if (package.colors.count(std::string(color_name)) == 0) {
      throw std::runtime_error("missing color token: " + std::string(color_name));
    }
  }

  const Json& typography = tokens.at("typography");
  package.typography.font_family = typography.at("fontFamily").get<std::string>();
  package.typography.title_size = typography.at("titleSize").get<int>();
  package.typography.body_size = typography.at("bodySize").get<int>();
  if (package.typography.font_family.empty() ||
      package.typography.title_size < 16 ||
      package.typography.title_size > 48 ||
      package.typography.body_size < 10 ||
      package.typography.body_size > 24) {
    throw std::runtime_error("invalid typography tokens");
  }
}

[[nodiscard]] std::string NormalizeLocale(std::string locale) {
  std::replace(locale.begin(), locale.end(), '_', '-');
  std::transform(locale.begin(), locale.end(), locale.begin(),
                 [](unsigned char value) {
                   return static_cast<char>(std::tolower(value));
                 });
  return locale;
}

[[nodiscard]] std::string SelectLocale(const Json& locales,
                                       const std::string& preferred_locale) {
  const std::string preferred = NormalizeLocale(preferred_locale);
  for (const auto& [locale, value] : locales.items()) {
    (void)value;
    if (NormalizeLocale(locale) == preferred) {
      return locale;
    }
  }

  const auto has_prefix = [&preferred](std::string_view prefix) {
    return preferred == prefix || preferred.rfind(std::string(prefix) + "-", 0) == 0;
  };
  const auto find = [&locales](std::initializer_list<std::string_view> aliases)
      -> std::string {
    for (const auto alias : aliases) {
      const std::string normalized_alias = NormalizeLocale(std::string(alias));
      for (const auto& [locale, value] : locales.items()) {
        (void)value;
        if (NormalizeLocale(locale) == normalized_alias) {
          return locale;
        }
      }
    }
    return {};
  };

  if (has_prefix("zh")) {
    std::string selected;
    if (preferred.find("hant") != std::string::npos ||
        preferred.find("-tw") != std::string::npos ||
        preferred.find("-mo") != std::string::npos) {
      selected = find({"zh-TW", "zh-HK", "zh-CN"});
    } else if (preferred.find("-hk") != std::string::npos) {
      selected = find({"zh-HK", "zh-TW", "zh-CN"});
    } else {
      selected = find({"zh-CN", "zh-HK", "zh-TW"});
    }
    if (!selected.empty()) {
      return selected;
    }
  }

  const std::size_t separator = preferred.find('-');
  const std::string language = preferred.substr(0, separator);
  for (const auto& [locale, value] : locales.items()) {
    (void)value;
    if (NormalizeLocale(locale) == language) {
      return locale;
    }
  }
  for (const auto& [locale, value] : locales.items()) {
    (void)value;
    const std::string normalized = NormalizeLocale(locale);
    if (normalized.rfind(language + "-", 0) == 0) {
      return locale;
    }
  }

  const std::string english = find({"en-US", "en"});
  return english.empty() ? locales.begin().key() : english;
}

void LoadLocale(const Json& manifest,
                const std::filesystem::path& root,
                const std::string& preferred_locale,
                ThemePackage& package) {
  const Json& locales = manifest.at("locales");
  if (!locales.is_object() || locales.empty()) {
    throw std::runtime_error("theme must define at least one locale");
  }
  const std::string selected = SelectLocale(locales, preferred_locale);
  const auto load_strings = [&locales, &root, &package](
                                const std::string& locale) {
    const auto locale_path = ResolveFile(
        root, locales.at(locale).get<std::string>(), "locales." + locale);
    const Json strings = ReadJson(locale_path);
    for (const auto& [key, value] : strings.items()) {
      if (!value.is_string()) {
        throw std::runtime_error("locale value must be text: " + key);
      }
      package.strings[key] = value.get<std::string>();
    }
  };

  if (locales.contains("en-US") && selected != "en-US") {
    load_strings("en-US");
  }
  load_strings(selected);
  package.locale = selected;
}

void LoadContent(const std::filesystem::path& path,
                 const std::filesystem::path& root,
                 ThemePackage& package) {
  const Json content = ReadJson(path);
  if (content.at("schemaVersion").get<int>() != kThemeSchemaVersion) {
    throw std::runtime_error("unsupported content schemaVersion");
  }

  const Json& items = content.at("items");
  if (!items.is_array() || items.size() > 10) {
    throw std::runtime_error("content items must be an array with at most 10 entries");
  }

  for (const Json& item : items) {
    FeatureCard card;
    card.id = item.at("id").get<std::string>();
    if (item.contains("titleKey")) {
      card.title = package.strings.at(item.at("titleKey").get<std::string>());
    } else {
      card.title = item.at("title").get<std::string>();
    }
    if (item.contains("descriptionKey")) {
      card.description =
          package.strings.at(item.at("descriptionKey").get<std::string>());
    } else {
      card.description = item.at("description").get<std::string>();
    }
    if (card.id.empty() || card.title.empty()) {
      throw std::runtime_error("content id and title cannot be empty");
    }
    if (item.contains("image")) {
      card.image = ResolveFile(
          root, item.at("image").get<std::string>(), "content.image");
    }
    if (item.contains("actionUrl")) {
      const std::string url = item.at("actionUrl").get<std::string>();
      if (url.rfind("https://", 0) != 0) {
        throw std::runtime_error("content actionUrl must use HTTPS");
      }
      card.action_url = url;
    }
    package.features.push_back(std::move(card));
  }
}

}  // namespace

ThemeLoadResult LoadTheme(const std::filesystem::path& theme_root,
                          const std::string& preferred_locale) noexcept {
  try {
    const auto root = std::filesystem::absolute(theme_root).lexically_normal();
    const Json manifest = ReadJson(root / "theme.json");
    if (manifest.at("schemaVersion").get<int>() != kThemeSchemaVersion) {
      throw std::runtime_error("unsupported theme schemaVersion");
    }

    const auto required =
        ParseVersion(manifest.at("minEngineVersion").get<std::string>());
    const auto current = ParseVersion(std::string(Version()));
    if (IsNewer(required, current)) {
      throw std::runtime_error("theme requires a newer engine");
    }

    ThemePackage package;
    package.root = root;
    package.id = manifest.at("id").get<std::string>();
    package.name = manifest.at("name").get<std::string>();
    if (package.id.empty() || package.name.empty()) {
      throw std::runtime_error("theme id and name cannot be empty");
    }

    const Json& window = manifest.at("window");
    package.window.width = window.at("width").get<int>();
    package.window.height = window.at("height").get<int>();
    package.window.radius = window.at("radius").get<int>();
    if (package.window.width < 640 || package.window.width > 1600 ||
        package.window.height < 400 || package.window.height > 1200 ||
        package.window.radius < 0 || package.window.radius > 64) {
      throw std::runtime_error("theme window dimensions are out of range");
    }

    const Json& pages = manifest.at("pages");
    if (!pages.contains("installing")) {
      throw std::runtime_error("theme must define an installing page");
    }
    for (const auto& [page_name, page_value] : pages.items()) {
      package.pages.emplace(
          page_name,
          ResolveFile(root, page_value.get<std::string>(), "pages." + page_name));
    }

    const auto tokens = ResolveFile(
        root, manifest.at("tokens").get<std::string>(), "tokens");
    LoadTokens(tokens, package);
    LoadLocale(manifest, root, preferred_locale, package);

    if (manifest.contains("content")) {
      const auto content = ResolveFile(
          root, manifest.at("content").get<std::string>(), "content");
      LoadContent(content, root, package);
    }

    return ThemeLoadResult{std::move(package), {}};
  } catch (const std::exception& error) {
    return ThemeLoadResult{std::nullopt, error.what()};
  }
}

}  // namespace nsis_dui
