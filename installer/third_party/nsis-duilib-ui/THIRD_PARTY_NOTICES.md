# Third-party notices

## DuiLib Ultimate

- Project: `qdtroy/DuiLib_Ultimate`
- Upstream commit: `1f1773ade89ed39439bcb938e907d90ae3320119`
- License: MIT
- Source: https://github.com/qdtroy/DuiLib_Ultimate

The vendored source is under `third_party/duilib`. Its original license is
preserved at `third_party/duilib/LICENSE`.

The selected commit predates the upstream WebView2 integration. This project
does not bundle WebView2 or CEF.

Local compatibility changes remove the unused Flash and IE WebBrowser controls,
convert legacy source encodings to UTF-8, and replace ATL-only debug helpers
with standard C++ assertions. This keeps the static renderer independent of
ATL and obsolete browser runtimes.

## JSON for Modern C++

- Project: `nlohmann/json`
- Version: `v3.12.0`
- License: MIT
- Source: https://github.com/nlohmann/json

The pinned single-header distribution is under `third_party/nlohmann`. Its
original license is preserved at `third_party/nlohmann/LICENSE.MIT`.

## NSIS plug-in headers

- Project: `NSIS-Dev/nsis`
- Version: `v3.04` (`25cf71ac318c73016d0706add96f25ef70ccd4f7`)
- License: zlib/libpng
- Source: https://github.com/NSIS-Dev/nsis

The pinned plug-in ABI headers are under `third_party/nsis`. Their original
license is preserved at `third_party/nsis/LICENSE`.
