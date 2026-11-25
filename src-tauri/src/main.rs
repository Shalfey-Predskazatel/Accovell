#![windows_subsystem = "windows"]

fn main() {
    if let Some(options) =
        codex_accounts_lib::installer::LaunchOptions::from_args(std::env::args().skip(1))
    {
        if let Err(error) = codex_accounts_lib::run_installer(options) {
            codex_accounts_lib::show_error(&error);
        }
        return;
    }
    if std::env::args().any(|argument| argument == "--prepare-uninstall") {
        if let Err(error) = codex_accounts_lib::prepare_for_uninstall() {
            codex_accounts_lib::show_error(&error);
            std::process::exit(1);
        }
        return;
    }
    match codex_accounts_lib::installer::ensure_webview2_for_app() {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            codex_accounts_lib::show_error(&error);
            return;
        }
    }
    if let Err(error) = codex_accounts_lib::run() {
        codex_accounts_lib::show_error(&error);
    }
}
