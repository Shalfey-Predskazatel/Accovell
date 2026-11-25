# Accovell 0.1.0 — Windows x64

This release packages the Codex Accounts r85 application under the Accovell product brand.

- Installer: Accovell-0.1.0-Setup-x64.exe.
- Portable folder: Accovell-Portable/, containing the application executable and WebView2Loader.dll.
- The installer was rebuilt from the portable r85 executable and loader in the supplied handoff, so both downloads use the same application binary.
- The installer uses the Microsoft WebView2 bootstrapper when the runtime is missing. The portable build requires WebView2 Runtime to be present.
- No account profiles, OAuth tokens, vault, or personal Codex configuration are included.

Functional testing was not performed for this publication.
