# Accovell 0.1.0 — Windows x64

This release packages the Codex Accounts r85 application under the Accovell product brand.

- Installer: `Accovell-0.1.0-Setup-x64.exe`.
- Portable archive: `Accovell-0.1.0-Portable.zip`, containing `Accovell-Portable/`, the application executable, and `WebView2Loader.dll`.
- The installer was rebuilt from the portable r85 executable and loader in the supplied handoff. Both downloads use the same application binary.
- If the WebView2 Runtime is missing, the installer uses Microsoft's WebView2 bootstrapper. The portable build requires the runtime to be installed already.
- No account profiles, OAuth tokens, vault, or personal Codex configuration are included.

Functional testing was not performed for this publication.

## Русский

Этот выпуск содержит приложение Codex Accounts r85 под брендом Accovell.

- Установщик: `Accovell-0.1.0-Setup-x64.exe`.
- Portable-архив: `Accovell-0.1.0-Portable.zip`. Внутри находится папка `Accovell-Portable/` с исполняемым файлом программы и `WebView2Loader.dll`.
- Установщик собран из portable-исполняемого файла r85 и загрузчика из переданного комплекта. В обеих загрузках используется один и тот же бинарный файл приложения.
- Если в системе нет WebView2 Runtime, установщик запускает bootstrapper Microsoft для его установки. Для portable-версии WebView2 Runtime должен быть установлен заранее.
- В архивы не входят профили аккаунтов, OAuth-токены, хранилище или личная конфигурация Codex.

Функциональная проверка перед публикацией не проводилась.
