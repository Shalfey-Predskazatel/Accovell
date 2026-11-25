#include "nsis_dui/renderer.hpp"
#include "nsis_dui/theme.hpp"

#include <cassert>
#include <filesystem>
#include <string>

int main() {
  const std::filesystem::path root =
      std::filesystem::path(NSIS_DUI_SOURCE_DIR) / "themes" / "onekey-modern";
  const auto theme = nsis_dui::LoadTheme(root, "zh-CN");
  assert(theme);

  nsis_dui::ViewState state;
  state.version = "7.0 & preview";
  state.status = "正在写入 <核心文件>…";
  state.progress = 42;
  state.feature_index = 1;
  state.install_directory = "D:\\Apps\\OneKey";
  state.per_user_install_directory = "D:\\Apps\\OneKey";
  state.per_machine_install_directory = "C:\\Program Files\\OneKey";
  const auto rendered = nsis_dui::RenderPage(*theme.package, "installing", state);
  assert(rendered);
  assert(rendered.xml->find(L"{{") == std::wstring::npos);
  assert(rendered.xml->find(L"7.0 &amp; preview") != std::wstring::npos);
  assert(rendered.xml->find(L"正在写入 &lt;核心文件&gt;…") != std::wstring::npos);
  assert(rendered.xml->find(L"value=\"42\"") != std::wstring::npos);

  const auto confirm_exit =
      nsis_dui::RenderPage(*theme.package, "confirmExit", state);
  assert(confirm_exit);
  assert(confirm_exit.xml->find(L"name=\"exit-stay\"") !=
         std::wstring::npos);
  assert(confirm_exit.xml->find(L"name=\"exit-confirm\"") !=
         std::wstring::npos);

  const auto welcome = nsis_dui::RenderPage(*theme.package, "welcome", state);
  assert(welcome);
  assert(welcome.xml->find(L"name=\"install-directory\"") !=
         std::wstring::npos);
  assert(welcome.xml->find(
             L"height=\"44\" bkcolor=\"#B8FFFFFF\" bordercolor="
             L"\"#D9E1E8\" bordersize=\"1\" borderround=\"10,10\" "
             L"inset=\"15,0,10,0\" childvalign=\"vcenter\"") !=
         std::wstring::npos);
  assert(welcome.xml->find(L"name=\"more-options\"") !=
         std::wstring::npos);
  assert(welcome.xml->find(L"name=\"scope-current\"") !=
         std::wstring::npos);
  assert(welcome.xml->find(L"name=\"scope-all\"") !=
         std::wstring::npos);
  assert(welcome.xml->find(L"font=\"0\" />") !=
         welcome.xml->rfind(L"font=\"0\" />"));
  assert(welcome.xml->find(L"name=\"legal-links\"") !=
         std::wstring::npos);
  assert(welcome.xml->find(L"<LegalNotice") != std::wstring::npos);
  assert(welcome.xml->find(L"showhtml=\"true\"") == std::wstring::npos);
  assert(welcome.xml->find(L"{a terms}") != std::wstring::npos);
  assert(welcome.xml->find(L"{a privacy}") != std::wstring::npos);
  assert(welcome.xml->find(L"Recommended") != std::wstring::npos);
  assert(welcome.xml->find(L"No administrator privileges required") ==
         std::wstring::npos);
  assert(welcome.xml->find(L"D:\\Apps\\OneKey") != std::wstring::npos);

  const auto options = nsis_dui::RenderPage(*theme.package, "options", state);
  assert(options);
  assert(options.xml->find(
             L"height=\"46\" bkcolor=\"#B8FFFFFF\" bordercolor="
             L"\"#D9E1E8\" bordersize=\"1\" borderround=\"10,10\" "
             L"inset=\"15,0,14,0\" childvalign=\"vcenter\"") !=
         std::wstring::npos);

  const auto uninstall_welcome =
      nsis_dui::RenderPage(*theme.package, "uninstallWelcome", state);
  assert(uninstall_welcome);
  assert(uninstall_welcome.xml->find(L"name=\"primary\"") !=
         std::wstring::npos);
  assert(uninstall_welcome.xml->find(L"name=\"cancel\"") !=
         std::wstring::npos);

  for (const auto& [page_name, page_path] : theme.package->pages) {
    (void)page_path;
    const auto page = nsis_dui::RenderPage(*theme.package, page_name, state);
    assert(page);
    assert(page.xml->find(L"{{") == std::wstring::npos);
  }

  const auto missing = nsis_dui::RenderPage(*theme.package, "missing", state);
  assert(!missing);
  assert(missing.error.find("does not define page") != std::string::npos);
  return 0;
}
