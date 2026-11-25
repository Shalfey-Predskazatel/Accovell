#include "nsis_dui/window.hpp"

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <iterator>
#include <string_view>
#include <utility>

#include <shellapi.h>
#include <windowsx.h>

#include "UILib.h"

namespace nsis_dui {
namespace {

[[nodiscard]] std::wstring Utf8ToWide(const std::string& value) {
  if (value.empty()) {
    return {};
  }
  const int length = MultiByteToWideChar(
      CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
      static_cast<int>(value.size()), nullptr, 0);
  if (length <= 0) {
    return {};
  }
  std::wstring result(static_cast<std::size_t>(length), L'\0');
  if (MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
                          static_cast<int>(value.size()), result.data(),
                          length) != length) {
    return {};
  }
  return result;
}

[[nodiscard]] HINSTANCE CurrentModule() noexcept {
  HMODULE module = nullptr;
  const auto address = reinterpret_cast<LPCWSTR>(&CurrentModule);
  if (GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                             GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                         address, &module) == FALSE) {
    return GetModuleHandleW(nullptr);
  }
  return module;
}

class ThreadDpiScope final {
 public:
  ThreadDpiScope() noexcept {
    const HMODULE user32 = GetModuleHandleW(L"user32.dll");
    set_context_ = reinterpret_cast<SetContext>(
        GetProcAddress(user32, "SetThreadDpiAwarenessContext"));
    if (set_context_ != nullptr) {
      previous_ = set_context_(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
  }

  ~ThreadDpiScope() {
    if (set_context_ != nullptr && previous_ != nullptr) {
      set_context_(previous_);
    }
  }

 private:
  using SetContext = DPI_AWARENESS_CONTEXT(WINAPI*)(DPI_AWARENESS_CONTEXT);
  SetContext set_context_ = nullptr;
  DPI_AWARENESS_CONTEXT previous_ = nullptr;
};

void ApplyModernCorners(HWND window) noexcept {
  const HMODULE dwmapi = LoadLibraryW(L"dwmapi.dll");
  if (dwmapi == nullptr) {
    return;
  }
  using SetWindowAttribute = HRESULT(WINAPI*)(HWND, DWORD, LPCVOID, DWORD);
  const auto set_attribute = reinterpret_cast<SetWindowAttribute>(
      GetProcAddress(dwmapi, "DwmSetWindowAttribute"));
  if (set_attribute != nullptr) {
    constexpr DWORD kWindowCornerPreference = 33;
    constexpr DWORD kRound = 2;
    set_attribute(window, kWindowCornerPreference, &kRound, sizeof(kRound));
  }
  FreeLibrary(dwmapi);
}

void ApplyWindowRegion(HWND window,
                       int radius,
                       int physical_width = 0,
                       int physical_height = 0) noexcept {
  if (window == nullptr || !IsWindow(window)) {
    return;
  }

  // NSIS hosts are commonly system-DPI aware.  When this plugin is called
  // after the per-monitor bootstrap scope has ended, GetWindowRect can be
  // DPI-virtualized (for example returning 900x560 for a physical 1350x840
  // window).  SetWindowRgn consumes physical window coordinates, so creating
  // the region from that virtualized rectangle clips the right and bottom of
  // the installer.  Keep the measurement in the same per-monitor coordinate
  // space as the HWND and the DIB surfaces.
  const ThreadDpiScope dpi_scope;

  // Prefer dimensions supplied by the caller.  They come from the same
  // physical values used by SetWindowPos and therefore cannot be converted
  // back to a virtualized 96-DPI rectangle by an NSIS host thread.
  int width = physical_width;
  int height = physical_height;
  if (width <= 0 || height <= 0) {
    RECT bounds = {};
    if (!GetWindowRect(window, &bounds)) {
      return;
    }
    width = bounds.right - bounds.left;
    height = bounds.bottom - bounds.top;
  }
  if (width <= 0 || height <= 0) {
    return;
  }

  const int corner = (std::max)(0, (std::min)(radius, (std::min)(width, height)));
  HRGN region = CreateRoundRectRgn(0, 0, width + 1, height + 1, corner,
                                   corner);
  if (region == nullptr) {
    return;
  }
  if (SetWindowRgn(window, region, TRUE) == 0) {
    DeleteObject(region);
  }
}

// The NSIS host window is hidden while the custom installer is displayed.
// DuiLib's CenterWindow() prefers that owner when it is present, so its
// stale/zero-sized rectangle can place the installer at the work area's edge.
// Center against the monitor work area directly and keep the window fully
// visible on high-DPI and multi-monitor setups.
void CenterOnWorkArea(HWND window) noexcept {
  if (window == nullptr || !IsWindow(window)) {
    return;
  }

  RECT window_rect = {};
  if (!GetWindowRect(window, &window_rect)) {
    return;
  }

  const HMONITOR monitor =
      MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
  MONITORINFO monitor_info = {};
  monitor_info.cbSize = sizeof(monitor_info);
  if (monitor == nullptr || !GetMonitorInfoW(monitor, &monitor_info)) {
    return;
  }

  const int width = window_rect.right - window_rect.left;
  const int height = window_rect.bottom - window_rect.top;
  const RECT work = monitor_info.rcWork;
  const int work_width = work.right - work.left;
  const int work_height = work.bottom - work.top;
  const int left = work.left +
                   (width >= work_width ? 0 : (work_width - width) / 2);
  const int top = work.top +
                  (height >= work_height ? 0 : (work_height - height) / 2);

  SetWindowPos(window, nullptr, left, top, 0, 0,
               SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
}

[[nodiscard]] UINT EffectiveDpiForMonitor(HMONITOR monitor,
                                          UINT requested_dpi,
                                          int logical_width,
                                          int logical_height,
                                          RECT* work_area = nullptr) noexcept {
  MONITORINFO info = {};
  info.cbSize = sizeof(info);
  if (monitor == nullptr || !GetMonitorInfoW(monitor, &info) ||
      logical_width <= 0 || logical_height <= 0) {
    return requested_dpi;
  }
  if (work_area != nullptr) {
    *work_area = info.rcWork;
  }
  const int width = info.rcWork.right - info.rcWork.left;
  const int height = info.rcWork.bottom - info.rcWork.top;
  const UINT width_limit = static_cast<UINT>(
      (std::max)(72, MulDiv(width, 96, logical_width)));
  const UINT height_limit = static_cast<UINT>(
      (std::max)(72, MulDiv(height, 96, logical_height)));
  return (std::min)(requested_dpi, (std::min)(width_limit, height_limit));
}

[[nodiscard]] UINT MonitorEffectiveDpi(HMONITOR monitor,
                                       UINT fallback_dpi) noexcept {
  if (monitor == nullptr) {
    return fallback_dpi;
  }

  const HMODULE shcore = LoadLibraryW(L"shcore.dll");
  if (shcore == nullptr) {
    return fallback_dpi;
  }
  using GetDpiForMonitor = HRESULT(WINAPI *)(HMONITOR, int, UINT *, UINT *);
  const auto get_dpi = reinterpret_cast<GetDpiForMonitor>(
      GetProcAddress(shcore, "GetDpiForMonitor"));
  UINT dpi_x = 0;
  UINT dpi_y = 0;
  const HRESULT result = get_dpi == nullptr
                            ? E_FAIL
                            : get_dpi(monitor, 0, &dpi_x, &dpi_y);
  FreeLibrary(shcore);
  if (SUCCEEDED(result) && dpi_x != 0) {
    return dpi_x;
  }
  return fallback_dpi;
}

[[nodiscard]] DWORD ParseArgb(LPCTSTR value, DWORD fallback) noexcept;

class BrandMarkControl final : public DuiLib::CControlUI {
 public:
  [[nodiscard]] LPCTSTR GetClass() const override {
    return L"BrandMarkUI";
  }

  void SetAttribute(LPCTSTR name, LPCTSTR value) override {
    if (_wcsicmp(name, L"brandcolor") == 0) {
      brand_ = ParseArgb(value, brand_);
    } else if (_wcsicmp(name, L"outercolor") == 0) {
      outer_ = ParseArgb(value, outer_);
    } else if (_wcsicmp(name, L"outlinecolor") == 0) {
      outline_ = ParseArgb(value, outline_);
    } else if (_wcsicmp(name, L"raycolor") == 0) {
      ray_ = ParseArgb(value, ray_);
    } else {
      CControlUI::SetAttribute(name, value);
    }
  }

  void Init() override {
    CControlUI::Init();
    last_tick_ = std::chrono::steady_clock::now();
    SetTimer(kAnimationTimer, 8);
  }

  void DoEvent(DuiLib::TEventUI& event) override {
    if (event.Type == DuiLib::UIEVENT_TIMER &&
        event.wParam == kAnimationTimer) {
      const auto now = std::chrono::steady_clock::now();
      const float elapsed = std::clamp(
          std::chrono::duration<float>(now - last_tick_).count(), 0.0F, 0.05F);
      last_tick_ = now;
      if (GetManager() != nullptr &&
          IsWindowVisible(GetManager()->GetPaintWindow())) {
        phase_ = std::fmod(phase_ + elapsed * 1.05F, 6.2831853F);
        Invalidate();
      }
      return;
    }
    CControlUI::DoEvent(event);
  }

  bool DoPaint(HDC dc,
               const RECT& paint,
               DuiLib::CControlUI* stop_control) override {
    if (!CControlUI::DoPaint(dc, paint, stop_control)) {
      return false;
    }

    const RECT bounds = GetPos();
    const Gdiplus::REAL width = static_cast<Gdiplus::REAL>(
        bounds.right - bounds.left);
    const Gdiplus::REAL height = static_cast<Gdiplus::REAL>(
        bounds.bottom - bounds.top);
    const Gdiplus::REAL unit = (std::min)(width, height);
    const Gdiplus::REAL scale = unit / 100.0F;
    const Gdiplus::REAL left =
        static_cast<Gdiplus::REAL>(bounds.left) + (width - unit) * 0.5F;
    const Gdiplus::REAL top =
        static_cast<Gdiplus::REAL>(bounds.top) + (height - unit) * 0.5F;
    const auto point = [left, top, scale](Gdiplus::REAL x,
                                         Gdiplus::REAL y) {
      return Gdiplus::PointF(left + x * scale, top + y * scale);
    };

    Gdiplus::Graphics graphics(dc);
    graphics.SetSmoothingMode(Gdiplus::SmoothingModeAntiAlias);
    graphics.SetCompositingQuality(Gdiplus::CompositingQualityHighQuality);

    const auto rounded_rect = [](Gdiplus::GraphicsPath& path,
                                 const Gdiplus::RectF& rect,
                                 Gdiplus::REAL radius) {
      const Gdiplus::REAL diameter = radius * 2.0F;
      path.AddArc(rect.X, rect.Y, diameter, diameter, 180.0F, 90.0F);
      path.AddArc(rect.GetRight() - diameter, rect.Y, diameter, diameter,
                  270.0F, 90.0F);
      path.AddArc(rect.GetRight() - diameter, rect.GetBottom() - diameter,
                  diameter, diameter, 0.0F, 90.0F);
      path.AddArc(rect.X, rect.GetBottom() - diameter, diameter, diameter,
                  90.0F, 90.0F);
      path.CloseFigure();
    };
    const Gdiplus::RectF outer(left, top, unit, unit);
    Gdiplus::GraphicsPath outer_shape;
    rounded_rect(outer_shape, outer, unit * 0.29F);
    Gdiplus::SolidBrush outer_fill{Gdiplus::Color(outer_)};
    graphics.FillPath(&outer_fill, &outer_shape);

    const Gdiplus::RectF inner(left + unit * 0.055F, top + unit * 0.055F,
                               unit * 0.89F, unit * 0.89F);
    Gdiplus::GraphicsPath inner_shape;
    rounded_rect(inner_shape, inner, unit * 0.245F);
    Gdiplus::LinearGradientBrush violet(
        inner, Gdiplus::Color(0xFFE38BF0), Gdiplus::Color(brand_),
        Gdiplus::LinearGradientModeForwardDiagonal);
    graphics.FillPath(&violet, &inner_shape);
    Gdiplus::Pen outline{Gdiplus::Color(outline_), (std::max)(0.55F, scale)};
    graphics.DrawPath(&outline, &inner_shape);

    const auto ray_state = graphics.Save();
    const Gdiplus::REAL center_x = left + unit * 0.5F;
    const Gdiplus::REAL center_y = top + unit * 0.5F;
    graphics.TranslateTransform(center_x, center_y);
    graphics.RotateTransform(3.5F * std::sin(phase_));
    graphics.TranslateTransform(-center_x, -center_y);
    Gdiplus::Pen rays{Gdiplus::Color(ray_), (std::max)(0.55F, 1.4F * scale)};
    rays.SetStartCap(Gdiplus::LineCapRound);
    rays.SetEndCap(Gdiplus::LineCapRound);
    graphics.DrawLine(&rays, point(50.0F, 24.0F), point(50.0F, 76.0F));
    graphics.DrawLine(&rays, point(24.0F, 50.0F), point(76.0F, 50.0F));
    graphics.DrawLine(&rays, point(32.0F, 32.0F), point(68.0F, 68.0F));
    graphics.DrawLine(&rays, point(68.0F, 32.0F), point(32.0F, 68.0F));
    graphics.Restore(ray_state);
    const Gdiplus::REAL sparkle_size =
        scale * (10.0F + 1.4F * (0.5F + 0.5F * std::sin(phase_ * 1.7F)));
    const Gdiplus::PointF sparkle_point = point(64.0F, 64.0F);
    Gdiplus::SolidBrush sparkle{Gdiplus::Color(0xFFFFFFFF)};
    graphics.FillEllipse(&sparkle, sparkle_point.X, sparkle_point.Y,
                         sparkle_size, sparkle_size);
    return true;
  }

 private:
  static constexpr UINT kAnimationTimer = 0x4F4D;
  DWORD brand_ = 0xFFD799FF;
  DWORD outer_ = 0xFF21122F;
  DWORD outline_ = 0x70FFB9ED;
  DWORD ray_ = 0xFFF8EFFC;
  std::chrono::steady_clock::time_point last_tick_{};
  float phase_ = 0.0F;
};

class CaptionButtonControl final : public DuiLib::CButtonUI {
 public:
  [[nodiscard]] LPCTSTR GetClass() const override {
    return L"CaptionButtonUI";
  }

  void SetAttribute(LPCTSTR name, LPCTSTR value) override {
    if (_wcsicmp(name, L"kind") == 0) {
      kind_ = value == nullptr ? L"" : value;
    } else if (_wcsicmp(name, L"iconcolor") == 0) {
      icon_ = ParseArgb(value, icon_);
    } else if (_wcsicmp(name, L"hoticoncolor") == 0) {
      hot_icon_ = ParseArgb(value, hot_icon_);
    } else {
      CButtonUI::SetAttribute(name, value);
    }
  }

  bool DoPaint(HDC dc,
               const RECT& paint,
               DuiLib::CControlUI* stop_control) override {
    if (!CButtonUI::DoPaint(dc, paint, stop_control)) {
      return false;
    }

    const RECT bounds = GetPos();
    const Gdiplus::REAL width =
        static_cast<Gdiplus::REAL>(bounds.right - bounds.left);
    const Gdiplus::REAL height =
        static_cast<Gdiplus::REAL>(bounds.bottom - bounds.top);
    const DWORD color = (m_uButtonState & UISTATE_HOT) != 0
                            ? hot_icon_
                            : icon_;
    Gdiplus::Graphics graphics(dc);
    graphics.SetSmoothingMode(Gdiplus::SmoothingModeAntiAlias);
    Gdiplus::Pen pen(Gdiplus::Color(color),
                     (std::max)(1.5F, width * 0.05F));
    pen.SetStartCap(Gdiplus::LineCapRound);
    pen.SetEndCap(Gdiplus::LineCapRound);

    const Gdiplus::REAL center_x = bounds.left + width * 0.5F;
    const Gdiplus::REAL center_y = bounds.top + height * 0.5F;
    if (_wcsicmp(kind_.c_str(), L"close") == 0) {
      const Gdiplus::REAL radius = (std::min)(width, height) * 0.17F;
      graphics.DrawLine(&pen, center_x - radius, center_y - radius,
                        center_x + radius, center_y + radius);
      graphics.DrawLine(&pen, center_x + radius, center_y - radius,
                        center_x - radius, center_y + radius);
    } else {
      const Gdiplus::REAL half_width = width * 0.19F;
      graphics.DrawLine(&pen, center_x - half_width, center_y + 3.0F,
                        center_x + half_width, center_y + 3.0F);
    }
    return true;
  }

 private:
  std::wstring kind_ = L"minimize";
  DWORD icon_ = 0xFF75807B;
  DWORD hot_icon_ = 0xFF151918;
};

class LegalNoticeControl final : public DuiLib::CLabelUI {
 public:
  [[nodiscard]] LPCTSTR GetClass() const override {
    return L"LegalNoticeUI";
  }

  UINT GetControlFlags() const override {
    return IsEnabled() ? UIFLAG_SETCURSOR : 0;
  }

  void SetAttribute(LPCTSTR name, LPCTSTR value) override {
    if (_wcsicmp(name, L"hotlinkcolor") == 0) {
      hot_link_color_ = ParseArgb(value, hot_link_color_);
    } else {
      CLabelUI::SetAttribute(name, value);
    }
  }

  void SetText(LPCTSTR text) override {
    CLabelUI::SetText(text);
    ParseText(text == nullptr ? std::wstring_view{} : std::wstring_view{text});
  }

  void DoEvent(DuiLib::TEventUI& event) override {
    if (event.Type == DuiLib::UIEVENT_SETCURSOR) {
      const int link = LinkAt(event.ptMouse);
      ::SetCursor(
          ::LoadCursorW(nullptr, link >= 0 ? IDC_HAND : IDC_ARROW));
      return;
    }
    if (event.Type == DuiLib::UIEVENT_MOUSEENTER ||
        event.Type == DuiLib::UIEVENT_MOUSEMOVE) {
      SetHoveredLink(LinkAt(event.ptMouse));
    } else if (event.Type == DuiLib::UIEVENT_MOUSELEAVE) {
      pressed_link_ = -1;
      SetHoveredLink(-1);
    } else if (event.Type == DuiLib::UIEVENT_BUTTONDOWN) {
      pressed_link_ = LinkAt(event.ptMouse);
    } else if (event.Type == DuiLib::UIEVENT_BUTTONUP) {
      const int released_link = LinkAt(event.ptMouse);
      if (released_link >= 0 && released_link == pressed_link_ &&
          m_pManager != nullptr) {
        m_pManager->SendNotify(this, DUI_MSGTYPE_LINK, released_link);
      }
      pressed_link_ = -1;
    }
    CLabelUI::DoEvent(event);
  }

  void PaintText(HDC dc) override {
    if (plain_text_.empty() || m_pManager == nullptr) {
      return;
    }

    const RECT bounds = GetPos();
    Gdiplus::RectF layout(
        static_cast<Gdiplus::REAL>(bounds.left),
        static_cast<Gdiplus::REAL>(bounds.top),
        static_cast<Gdiplus::REAL>(bounds.right - bounds.left),
        static_cast<Gdiplus::REAL>(bounds.bottom - bounds.top));
    Gdiplus::Graphics graphics(dc);
    graphics.SetTextRenderingHint(Gdiplus::TextRenderingHintClearTypeGridFit);
    Gdiplus::Font font(dc, m_pManager->GetFont(m_iFont));
    Gdiplus::StringFormat format(Gdiplus::StringFormat::GenericTypographic());
    format.SetAlignment(Gdiplus::StringAlignmentCenter);
    format.SetLineAlignment(Gdiplus::StringAlignmentCenter);
    format.SetFormatFlags(Gdiplus::StringFormatFlagsNoWrap |
                          Gdiplus::StringFormatFlagsMeasureTrailingSpaces);

    const DWORD text_color =
        m_dwTextColor == 0 ? m_pManager->GetDefaultFontColor() : m_dwTextColor;
    Gdiplus::SolidBrush text_brush{Gdiplus::Color(text_color)};
    graphics.DrawString(plain_text_.c_str(),
                        static_cast<INT>(plain_text_.size()), &font, layout,
                        &format, &text_brush);

    std::array<Gdiplus::CharacterRange, 2> ranges = {
        Gdiplus::CharacterRange(link_starts_[0], link_lengths_[0]),
        Gdiplus::CharacterRange(link_starts_[1], link_lengths_[1])};
    format.SetMeasurableCharacterRanges(static_cast<INT>(ranges.size()),
                                        ranges.data());
    std::array<Gdiplus::Region, 2> regions;
    if (graphics.MeasureCharacterRanges(
            plain_text_.c_str(), static_cast<INT>(plain_text_.size()), &font,
            layout, &format, static_cast<INT>(regions.size()),
            regions.data()) != Gdiplus::Ok) {
      return;
    }

    for (std::size_t index = 0; index < regions.size(); ++index) {
      Gdiplus::RectF link_bounds;
      regions[index].GetBounds(&link_bounds, &graphics);
      link_rects_[index] = {
          static_cast<LONG>(std::floor(link_bounds.X)),
          static_cast<LONG>(std::floor(link_bounds.Y)),
          static_cast<LONG>(std::ceil(link_bounds.GetRight())),
          static_cast<LONG>(std::ceil(link_bounds.GetBottom()))};
      const DWORD underline_color =
          hovered_link_ == static_cast<int>(index) ? hot_link_color_ : text_color;
      Gdiplus::Pen underline(Gdiplus::Color(underline_color),
                             hovered_link_ == static_cast<int>(index) ? 1.5F
                                                                      : 1.0F);
      const Gdiplus::REAL underline_y = link_bounds.GetBottom() - 1.5F;
      graphics.DrawLine(&underline, link_bounds.X, underline_y,
                        link_bounds.GetRight(), underline_y);
    }
  }

  [[nodiscard]] DuiLib::CDuiString LinkTarget(int index) const {
    return index == 0 ? L"terms" : index == 1 ? L"privacy" : L"";
  }

 private:
  void ParseText(std::wstring_view raw) {
    constexpr std::wstring_view kTermsOpen = L"{a terms}";
    constexpr std::wstring_view kPrivacyOpen = L"{a privacy}";
    constexpr std::wstring_view kClose = L"{/a}";

    plain_text_.clear();
    link_starts_.fill(0);
    link_lengths_.fill(0);
    link_rects_.fill(RECT{});

    const std::size_t terms_open = raw.find(kTermsOpen);
    const std::size_t terms_close =
        terms_open == std::wstring_view::npos
            ? std::wstring_view::npos
            : raw.find(kClose, terms_open + kTermsOpen.size());
    const std::size_t privacy_open =
        terms_close == std::wstring_view::npos
            ? std::wstring_view::npos
            : raw.find(kPrivacyOpen, terms_close + kClose.size());
    const std::size_t privacy_close =
        privacy_open == std::wstring_view::npos
            ? std::wstring_view::npos
            : raw.find(kClose, privacy_open + kPrivacyOpen.size());
    if (terms_open == std::wstring_view::npos ||
        terms_close == std::wstring_view::npos ||
        privacy_open == std::wstring_view::npos ||
        privacy_close == std::wstring_view::npos) {
      plain_text_.assign(raw);
      return;
    }

    plain_text_.append(raw.substr(0, terms_open));
    link_starts_[0] = static_cast<INT>(plain_text_.size());
    plain_text_.append(raw.substr(terms_open + kTermsOpen.size(),
                                  terms_close - terms_open -
                                      kTermsOpen.size()));
    link_lengths_[0] =
        static_cast<INT>(plain_text_.size()) - link_starts_[0];

    std::size_t between_start = terms_close + kClose.size();
    if (between_start < raw.size() && raw[between_start] == L' ') {
      ++between_start;
    }
    plain_text_.append(
        raw.substr(between_start, privacy_open - between_start));
    link_starts_[1] = static_cast<INT>(plain_text_.size());
    plain_text_.append(raw.substr(privacy_open + kPrivacyOpen.size(),
                                  privacy_close - privacy_open -
                                      kPrivacyOpen.size()));
    link_lengths_[1] =
        static_cast<INT>(plain_text_.size()) - link_starts_[1];

    std::size_t suffix_start = privacy_close + kClose.size();
    if (suffix_start < raw.size() && raw[suffix_start] == L' ') {
      ++suffix_start;
    }
    plain_text_.append(raw.substr(suffix_start));
  }

  [[nodiscard]] int LinkAt(POINT point) const noexcept {
    for (std::size_t index = 0; index < link_rects_.size(); ++index) {
      if (PtInRect(&link_rects_[index], point)) {
        return static_cast<int>(index);
      }
    }
    return -1;
  }

  void SetHoveredLink(int index) {
    if (hovered_link_ != index) {
      hovered_link_ = index;
      Invalidate();
    }
  }

  std::wstring plain_text_;
  std::array<INT, 2> link_starts_{};
  std::array<INT, 2> link_lengths_{};
  std::array<RECT, 2> link_rects_{};
  int hovered_link_ = -1;
  int pressed_link_ = -1;
  DWORD hot_link_color_ = 0xFF151918;
};

[[nodiscard]] DWORD ParseArgb(LPCTSTR value, DWORD fallback) noexcept {
  if (value == nullptr) {
    return fallback;
  }
  while (*value != L'\0' && *value <= L' ') {
    value = CharNext(value);
  }
  if (*value == L'#') {
    value = CharNext(value);
  }
  wchar_t* end = nullptr;
  const unsigned long parsed = wcstoul(value, &end, 16);
  if (end == value || *end != L'\0') {
    return fallback;
  }
  return wcslen(value) == 6 ? static_cast<DWORD>(parsed | 0xFF000000UL)
                            : static_cast<DWORD>(parsed);
}

[[nodiscard]] Gdiplus::Color AlphaColor(DWORD argb, BYTE alpha) noexcept {
  const Gdiplus::Color color(argb);
  return Gdiplus::Color(alpha, color.GetR(), color.GetG(), color.GetB());
}

void AddRoundedRectangle(Gdiplus::GraphicsPath& path,
                         const Gdiplus::RectF& bounds,
                         Gdiplus::REAL radius) {
  const Gdiplus::REAL diameter = radius * 2.0F;
  path.AddArc(bounds.X, bounds.Y, diameter, diameter, 180.0F, 90.0F);
  path.AddArc(bounds.GetRight() - diameter, bounds.Y, diameter, diameter,
              270.0F, 90.0F);
  path.AddArc(bounds.GetRight() - diameter, bounds.GetBottom() - diameter,
              diameter, diameter, 0.0F, 90.0F);
  path.AddArc(bounds.X, bounds.GetBottom() - diameter, diameter, diameter,
              90.0F, 90.0F);
  path.CloseFigure();
}

class SoftButtonControl final : public DuiLib::CButtonUI {
 public:
  [[nodiscard]] LPCTSTR GetClass() const override {
    return L"SoftButtonUI";
  }

  void SetAttribute(LPCTSTR name, LPCTSTR value) override {
    if (_wcsicmp(name, L"ripplecolor") == 0) {
      ripple_color_ = ParseArgb(value, ripple_color_);
    } else if (_wcsicmp(name, L"gradientstart") == 0) {
      gradient_start_ = ParseArgb(value, gradient_start_);
      gradient_enabled_ = true;
    } else if (_wcsicmp(name, L"gradientend") == 0) {
      gradient_end_ = ParseArgb(value, gradient_end_);
      gradient_enabled_ = true;
    } else {
      CButtonUI::SetAttribute(name, value);
    }
  }

  void DoEvent(DuiLib::TEventUI& event) override {
    if (event.Type == DuiLib::UIEVENT_TIMER &&
        event.wParam == kRippleTimer) {
      if (GetManager() == nullptr ||
          !IsWindowVisible(GetManager()->GetPaintWindow())) {
        KillTimer(kRippleTimer);
        ripple_active_ = false;
        return;
      }
      const auto now = std::chrono::steady_clock::now();
      ripple_elapsed_ = std::clamp(
          ripple_elapsed_ + std::chrono::duration<float>(now - last_tick_).count(),
          0.0F, 0.5F);
      last_tick_ = now;
      if (ripple_elapsed_ >= kRippleDuration) {
        ripple_active_ = false;
        KillTimer(kRippleTimer);
      }
      Invalidate();
      return;
    }

    if (event.Type == DuiLib::UIEVENT_BUTTONDOWN) {
      const RECT bounds = GetPos();
      const float width = static_cast<float>(bounds.right - bounds.left);
      const float height = static_cast<float>(bounds.bottom - bounds.top);
      ripple_center_.X = std::clamp(
          static_cast<float>(event.ptMouse.x - bounds.left), 0.0F, width);
      ripple_center_.Y = std::clamp(
          static_cast<float>(event.ptMouse.y - bounds.top), 0.0F, height);
      ripple_elapsed_ = 0.0F;
      ripple_active_ = true;
      last_tick_ = std::chrono::steady_clock::now();
      SetTimer(kRippleTimer, 8);
    } else if (event.Type == DuiLib::UIEVENT_MOUSEMOVE) {
      const RECT bounds = GetPos();
      const float width = static_cast<float>(bounds.right - bounds.left);
      const float height = static_cast<float>(bounds.bottom - bounds.top);
      hover_center_.X = std::clamp(
          static_cast<float>(event.ptMouse.x - bounds.left), 0.0F, width);
      hover_center_.Y = std::clamp(
          static_cast<float>(event.ptMouse.y - bounds.top), 0.0F, height);
    }
    CButtonUI::DoEvent(event);
  }

  bool DoPaint(HDC dc,
               const RECT& paint,
               DuiLib::CControlUI* stop_control) override {
    if (!CButtonUI::DoPaint(dc, paint, stop_control)) {
      return false;
    }

    const RECT bounds = GetPos();
    const Gdiplus::REAL width =
        static_cast<Gdiplus::REAL>(bounds.right - bounds.left);
    const Gdiplus::REAL height =
        static_cast<Gdiplus::REAL>(bounds.bottom - bounds.top);
    if (width <= 0.0F || height <= 0.0F) {
      return true;
    }

    const bool hovered = (m_uButtonState & UISTATE_HOT) != 0;
    if (!gradient_enabled_ && !hovered && !ripple_active_) {
      return true;
    }

    Gdiplus::Graphics graphics(dc);
    graphics.SetSmoothingMode(Gdiplus::SmoothingModeAntiAlias);
    const Gdiplus::RectF rect(static_cast<Gdiplus::REAL>(bounds.left),
                              static_cast<Gdiplus::REAL>(bounds.top), width,
                              height);
    Gdiplus::GraphicsPath button_shape;
    AddRoundedRectangle(button_shape, rect,
                        (std::min)(height * 0.28F, width * 0.12F));
    const auto saved = graphics.Save();
    graphics.SetClip(&button_shape);

    if (gradient_enabled_) {
      Gdiplus::LinearGradientBrush gradient(
          rect, AlphaColor(gradient_start_, 94),
          AlphaColor(gradient_end_, 94), Gdiplus::LinearGradientModeForwardDiagonal);
      graphics.FillPath(&gradient, &button_shape);
    }

    if (hovered) {
      const Gdiplus::REAL radius = (std::max)(width, height) * 0.68F;
      Gdiplus::GraphicsPath glow_path;
      glow_path.AddEllipse(hover_center_.X - radius,
                           hover_center_.Y - radius,
                           radius * 2.0F, radius * 2.0F);
      Gdiplus::PathGradientBrush glow(&glow_path);
      glow.SetCenterPoint(hover_center_);
      glow.SetCenterColor(AlphaColor(ripple_color_, 24));
      Gdiplus::Color clear = AlphaColor(ripple_color_, 0);
      INT color_count = 1;
      glow.SetSurroundColors(&clear, &color_count);
      graphics.FillPath(&glow, &glow_path);
    }

    if (ripple_active_) {
      const float progress = std::clamp(ripple_elapsed_ / kRippleDuration,
                                        0.0F, 1.0F);
      const float eased = 1.0F - std::pow(1.0F - progress, 3.0F);
      const Gdiplus::REAL radius =
          (std::max)(width, height) * (0.08F + 0.82F * eased);
      Gdiplus::GraphicsPath ripple_path;
      ripple_path.AddEllipse(ripple_center_.X - radius,
                             ripple_center_.Y - radius,
                             radius * 2.0F, radius * 2.0F);
      Gdiplus::PathGradientBrush ripple(&ripple_path);
      ripple.SetCenterPoint(ripple_center_);
      const BYTE alpha = static_cast<BYTE>(
          48.0F * std::sin(progress * 3.14159265F));
      ripple.SetCenterColor(AlphaColor(ripple_color_, alpha));
      Gdiplus::Color clear = AlphaColor(ripple_color_, 0);
      INT color_count = 1;
      ripple.SetSurroundColors(&clear, &color_count);
      graphics.FillPath(&ripple, &ripple_path);
    }

    graphics.Restore(saved);
    if (gradient_enabled_) {
      PaintText(dc);
    }
    return true;
  }

 private:
  static constexpr UINT kRippleTimer = 0x4F4E;
  static constexpr float kRippleDuration = 0.38F;
  DWORD ripple_color_ = 0xFFFFB9ED;
  DWORD gradient_start_ = 0;
  DWORD gradient_end_ = 0;
  Gdiplus::PointF ripple_center_{0.0F, 0.0F};
  Gdiplus::PointF hover_center_{0.0F, 0.0F};
  std::chrono::steady_clock::time_point last_tick_{};
  float ripple_elapsed_ = 0.0F;
  bool gradient_enabled_ = false;
  bool ripple_active_ = false;
};

class OrbitIndicatorControl final : public DuiLib::CControlUI {
 public:
  [[nodiscard]] LPCTSTR GetClass() const override {
    return L"OrbitIndicatorUI";
  }

  void SetAttribute(LPCTSTR name, LPCTSTR value) override {
    if (_wcsicmp(name, L"brandcolor") == 0) {
      brand_ = ParseArgb(value, brand_);
    } else if (_wcsicmp(name, L"secondarycolor") == 0) {
      secondary_ = ParseArgb(value, secondary_);
    } else if (_wcsicmp(name, L"tertiarycolor") == 0) {
      tertiary_ = ParseArgb(value, tertiary_);
    } else {
      CControlUI::SetAttribute(name, value);
    }
  }

  void Init() override {
    CControlUI::Init();
    last_tick_ = std::chrono::steady_clock::now();
    SetTimer(kOrbitTimer, 8);
  }

  void DoEvent(DuiLib::TEventUI& event) override {
    if (event.Type == DuiLib::UIEVENT_TIMER && event.wParam == kOrbitTimer) {
      const auto now = std::chrono::steady_clock::now();
      const float elapsed = std::clamp(
          std::chrono::duration<float>(now - last_tick_).count(), 0.0F, 0.05F);
      last_tick_ = now;
      if (GetManager() != nullptr &&
          IsWindowVisible(GetManager()->GetPaintWindow())) {
        phase_ = std::fmod(phase_ + elapsed * 0.72F, 6.2831853F);
        Invalidate();
      }
      return;
    }
    CControlUI::DoEvent(event);
  }

  bool DoPaint(HDC dc,
               const RECT& paint,
               DuiLib::CControlUI* stop_control) override {
    if (!CControlUI::DoPaint(dc, paint, stop_control)) {
      return false;
    }
    const RECT bounds = GetPos();
    const Gdiplus::REAL width = static_cast<Gdiplus::REAL>(bounds.right - bounds.left);
    const Gdiplus::REAL height = static_cast<Gdiplus::REAL>(bounds.bottom - bounds.top);
    const Gdiplus::REAL unit = (std::min)(width, height);
    const Gdiplus::REAL cx = bounds.left + width * 0.5F;
    const Gdiplus::REAL cy = bounds.top + height * 0.5F;
    const Gdiplus::REAL radius = unit * 0.40F;
    if (unit <= 0.0F) {
      return true;
    }

    Gdiplus::Graphics graphics(dc);
    graphics.SetSmoothingMode(Gdiplus::SmoothingModeAntiAlias);
    const Gdiplus::REAL line = (std::max)(0.7F, unit * 0.018F);
    Gdiplus::Pen circular(AlphaColor(brand_, 56), line);
    Gdiplus::Pen orbit(AlphaColor(secondary_, 70), line);
    graphics.DrawEllipse(&circular, cx - radius, cy - radius,
                         radius * 2.0F, radius * 2.0F);

    const auto orbit_state = graphics.Save();
    graphics.TranslateTransform(cx, cy);
    graphics.RotateTransform(33.0F);
    graphics.DrawEllipse(&orbit, -radius, -radius * 0.43F,
                         radius * 2.0F, radius * 0.86F);
    graphics.Restore(orbit_state);

    const Gdiplus::REAL core_size = unit * 0.27F;
    const Gdiplus::RectF core(cx - core_size * 0.5F,
                              cy - core_size * 0.5F,
                              core_size, core_size);
    Gdiplus::GraphicsPath core_path;
    core_path.AddEllipse(core);
    Gdiplus::PathGradientBrush core_brush(&core_path);
    core_brush.SetCenterPoint(Gdiplus::PointF(cx - core_size * 0.12F,
                                               cy - core_size * 0.18F));
    core_brush.SetCenterColor(Gdiplus::Color(0xFFFFF5FF));
    Gdiplus::Color core_edge = Gdiplus::Color(brand_);
    INT color_count = 1;
    core_brush.SetSurroundColors(&core_edge, &color_count);
    graphics.FillPath(&core_brush, &core_path);

    const Gdiplus::REAL satellite_size = (std::max)(2.0F, unit * 0.085F);
    const Gdiplus::REAL satellite_x = cx + radius * std::cos(phase_);
    const Gdiplus::REAL satellite_y = cy + radius * std::sin(phase_);
    Gdiplus::SolidBrush satellite{Gdiplus::Color(brand_)};
    graphics.FillEllipse(&satellite, satellite_x - satellite_size * 0.5F,
                         satellite_y - satellite_size * 0.5F,
                         satellite_size, satellite_size);
    const Gdiplus::REAL second_phase = -phase_ * 0.64F + 1.2F;
    const Gdiplus::REAL second_size = satellite_size * 0.62F;
    Gdiplus::SolidBrush second{Gdiplus::Color(secondary_)};
    graphics.FillEllipse(&second,
                         cx + radius * 0.74F * std::cos(second_phase) - second_size * 0.5F,
                         cy + radius * 0.74F * std::sin(second_phase) - second_size * 0.5F,
                         second_size, second_size);

    const Gdiplus::REAL third_phase = phase_ * 1.18F - 2.0F;
    const Gdiplus::REAL third_size = satellite_size * 0.78F;
    Gdiplus::SolidBrush third{Gdiplus::Color(tertiary_)};
    graphics.FillEllipse(&third,
                         cx + radius * 0.88F * std::cos(third_phase) - third_size * 0.5F,
                         cy + radius * 0.88F * std::sin(third_phase) - third_size * 0.5F,
                         third_size, third_size);
    return true;
  }

 private:
  static constexpr UINT kOrbitTimer = 0x4F4F;
  DWORD brand_ = 0xFFD799FF;
  DWORD secondary_ = 0xFFFFB9ED;
  DWORD tertiary_ = 0xFF54E3FF;
  std::chrono::steady_clock::time_point last_tick_{};
  float phase_ = 0.0F;
};

class LightBackdropControl final : public DuiLib::CControlUI {
 public:
  [[nodiscard]] LPCTSTR GetClass() const override {
    return L"LightBackdropUI";
  }

  void SetAttribute(LPCTSTR name, LPCTSTR value) override {
    if (_wcsicmp(name, L"canvascolor") == 0) {
      canvas_ = ParseArgb(value, canvas_);
    } else if (_wcsicmp(name, L"bluecolor") == 0) {
      blue_ = ParseArgb(value, blue_);
    } else if (_wcsicmp(name, L"pinkcolor") == 0 ||
               _wcsicmp(name, L"greencolor") == 0) {
      pink_ = ParseArgb(value, pink_);
    } else {
      CControlUI::SetAttribute(name, value);
    }
  }

  bool DoPaint(HDC dc,
               const RECT& paint,
               DuiLib::CControlUI* stop_control) override {
    if (!CControlUI::DoPaint(dc, paint, stop_control)) {
      return false;
    }

    const RECT raw = GetPos();
    const Gdiplus::RectF bounds(
        static_cast<Gdiplus::REAL>(raw.left),
        static_cast<Gdiplus::REAL>(raw.top),
        static_cast<Gdiplus::REAL>(raw.right - raw.left),
        static_cast<Gdiplus::REAL>(raw.bottom - raw.top));
    Gdiplus::Graphics graphics(dc);
    graphics.SetSmoothingMode(Gdiplus::SmoothingModeAntiAlias);
    graphics.SetCompositingQuality(Gdiplus::CompositingQualityHighQuality);

    Gdiplus::SolidBrush canvas{Gdiplus::Color(canvas_)};
    graphics.FillRectangle(&canvas, bounds);

    // Keep the backdrop atmospheric without repainting a busy texture. A
    // single low-contrast bloom plus two broad ribbons matches the glass
    // background used by the dashboard and remains inexpensive on Windows 10.
    Gdiplus::GraphicsPath bloom_path;
    const Gdiplus::RectF bloom(bounds.X + bounds.Width * 0.18F,
                               bounds.Y - bounds.Height * 0.56F,
                               bounds.Width * 0.64F,
                               bounds.Height * 0.96F);
    bloom_path.AddEllipse(bloom);
    Gdiplus::PathGradientBrush bloom_brush(&bloom_path);
    bloom_brush.SetCenterPoint(Gdiplus::PointF(
        bloom.X + bloom.Width * 0.5F, bloom.Y + bloom.Height * 0.5F));
    bloom_brush.SetCenterColor(AlphaColor(blue_, 42));
    Gdiplus::Color transparent_blue = AlphaColor(blue_, 0);
    INT blue_count = 1;
    bloom_brush.SetSurroundColors(&transparent_blue, &blue_count);
    graphics.FillPath(&bloom_brush, &bloom_path);

    const Gdiplus::REAL ribbon_width = bounds.Height * 0.075F;
    const auto draw_ribbon = [&](Gdiplus::REAL x,
                                 Gdiplus::REAL y,
                                 Gdiplus::REAL rotation,
                                 Gdiplus::REAL ellipse_width,
                                 Gdiplus::REAL ellipse_height,
                                 DWORD color,
                                 BYTE alpha) {
      const auto state = graphics.Save();
      graphics.TranslateTransform(x, y);
      graphics.RotateTransform(rotation);
      Gdiplus::Pen ribbon(AlphaColor(color, alpha), ribbon_width);
      graphics.DrawEllipse(&ribbon, -ellipse_width * 0.50F,
                           -ellipse_height * 0.16F, ellipse_width,
                           ellipse_height);
      graphics.Restore(state);
    };

    draw_ribbon(bounds.X + bounds.Width * 0.22F,
                bounds.Y + bounds.Height * 0.26F,
                -16.0F,
                bounds.Width * 0.78F,
                bounds.Height * 0.36F,
                blue_,
                22);
    draw_ribbon(bounds.X + bounds.Width * 0.78F,
                bounds.Y + bounds.Height * 0.26F,
                17.0F,
                bounds.Width * 0.76F,
                bounds.Height * 0.35F,
                pink_,
                24);
    return true;
  }

 private:
  DWORD canvas_ = 0xFF100A18;
  DWORD blue_ = 0xFFD799FF;
  DWORD pink_ = 0xFFFFB9ED;
};

class AnimatedProgressControl final : public DuiLib::CControlUI {
 public:
  [[nodiscard]] LPCTSTR GetClass() const override {
    return L"AnimatedProgressUI";
  }

  void SetAttribute(LPCTSTR name, LPCTSTR value) override {
    if (_wcsicmp(name, L"value") == 0) {
      wchar_t* end = nullptr;
      const long parsed = wcstol(value, &end, 10);
      if (end != value && *end == L'\0') {
        target_ = static_cast<float>(std::clamp(parsed, 0L, 100L));
      }
    } else if (_wcsicmp(name, L"trackcolor") == 0) {
      track_ = ParseArgb(value, track_);
    } else if (_wcsicmp(name, L"startcolor") == 0) {
      start_ = ParseArgb(value, start_);
    } else if (_wcsicmp(name, L"endcolor") == 0) {
      end_ = ParseArgb(value, end_);
    } else {
      CControlUI::SetAttribute(name, value);
    }
  }

  void Init() override {
    CControlUI::Init();
    displayed_ = target_;
    last_tick_ = std::chrono::steady_clock::now();
    SetTimer(kAnimationTimer, 8);
  }

  void SetValue(int value) noexcept {
    const float next = static_cast<float>(std::clamp(value, 0, 100));
    if (next != target_) {
      flash_ = 1.0F;
    }
    target_ = next;
    Invalidate();
  }

  void ResetValue(int value) noexcept {
    target_ = static_cast<float>(std::clamp(value, 0, 100));
    displayed_ = target_;
    flash_ = 0.0F;
    Invalidate();
  }

  void DoEvent(DuiLib::TEventUI& event) override {
    if (event.Type == DuiLib::UIEVENT_TIMER &&
        event.wParam == kAnimationTimer) {
      const auto now = std::chrono::steady_clock::now();
      const float elapsed = std::clamp(
          std::chrono::duration<float>(now - last_tick_).count(), 0.0F, 0.05F);
      last_tick_ = now;
      if (GetManager() == nullptr ||
          !IsWindowVisible(GetManager()->GetPaintWindow())) {
        return;
      }
      const float difference = target_ - displayed_;
      const float blend = 1.0F - std::exp(-elapsed * 7.2F);
      displayed_ = std::abs(difference) < 0.035F
                       ? target_
                       : displayed_ + difference * blend;
      shimmer_ += elapsed * 0.30F;
      flash_ = (std::max)(0.0F, flash_ - elapsed * 1.8F);
      if (shimmer_ > 1.35F) {
        shimmer_ = -0.35F;
      }
      Invalidate();
      return;
    }
    CControlUI::DoEvent(event);
  }

  bool DoPaint(HDC dc,
               const RECT& paint,
               DuiLib::CControlUI* stop_control) override {
    if (!CControlUI::DoPaint(dc, paint, stop_control)) {
      return false;
    }

    const RECT raw = GetPos();
    const Gdiplus::REAL width = static_cast<Gdiplus::REAL>(raw.right - raw.left);
    const Gdiplus::REAL height = static_cast<Gdiplus::REAL>(raw.bottom - raw.top);
    const Gdiplus::RectF track(static_cast<Gdiplus::REAL>(raw.left) + 10.0F,
                               static_cast<Gdiplus::REAL>(raw.top) +
                                   (height - 8.0F) * 0.5F,
                               (std::max)(0.0F, width - 20.0F), 8.0F);
    Gdiplus::Graphics graphics(dc);
    graphics.SetSmoothingMode(Gdiplus::SmoothingModeAntiAlias);
    graphics.SetCompositingQuality(Gdiplus::CompositingQualityHighQuality);

    Gdiplus::GraphicsPath track_path;
    AddRoundedRectangle(track_path, track, 4.0F);
    Gdiplus::SolidBrush track_brush{Gdiplus::Color(track_)};
    graphics.FillPath(&track_brush, &track_path);

    const Gdiplus::REAL fill_width =
        track.Width * std::clamp(displayed_, 0.0F, 100.0F) / 100.0F;
    if (fill_width <= 0.5F) {
      return true;
    }
    const Gdiplus::RectF fill(track.X, track.Y, fill_width, track.Height);
    Gdiplus::GraphicsPath fill_path;
    AddRoundedRectangle(fill_path, fill,
                        (std::min)(4.0F, fill_width * 0.5F));
    Gdiplus::LinearGradientBrush fill_brush(
        fill, Gdiplus::Color(start_), Gdiplus::Color(end_),
        Gdiplus::LinearGradientModeHorizontal);
    graphics.FillPath(&fill_brush, &fill_path);

    const auto clip_state = graphics.Save();
    graphics.SetClip(&fill_path);
    const Gdiplus::REAL shimmer_x = fill.X + fill.Width * shimmer_ - 16.0F;
    const Gdiplus::RectF shimmer_rect(shimmer_x, fill.Y - 2.0F,
                                     32.0F, fill.Height + 4.0F);
    Gdiplus::LinearGradientBrush shimmer_brush(
        shimmer_rect, Gdiplus::Color(0, 255, 255, 255),
        Gdiplus::Color(0, 255, 255, 255),
        Gdiplus::LinearGradientModeHorizontal);
    Gdiplus::Color shimmer_colors[] = {
        Gdiplus::Color(0, 255, 255, 255),
        Gdiplus::Color(48, 255, 255, 255),
        Gdiplus::Color(0, 255, 255, 255)};
    Gdiplus::REAL shimmer_positions[] = {0.0F, 0.5F, 1.0F};
    shimmer_brush.SetInterpolationColors(shimmer_colors,
                                         shimmer_positions, 3);
    graphics.FillRectangle(&shimmer_brush, shimmer_rect);
    graphics.Restore(clip_state);

    if (flash_ > 0.02F) {
      const Gdiplus::REAL endpoint = fill.GetRight();
      const Gdiplus::REAL center_y = track.Y + track.Height * 0.5F;
      const Gdiplus::REAL glow_size = 7.0F + 5.0F * flash_;
      const BYTE alpha = static_cast<BYTE>(45.0F * flash_);
      Gdiplus::SolidBrush halo(AlphaColor(end_, alpha));
      graphics.FillEllipse(&halo, endpoint - glow_size * 0.5F,
                           center_y - glow_size * 0.5F,
                           glow_size, glow_size);
    }
    return true;
  }

 private:
  static constexpr UINT kAnimationTimer = 0x4F4B;
  std::chrono::steady_clock::time_point last_tick_{};
  float target_ = 0.0F;
  float displayed_ = 0.0F;
  float shimmer_ = -0.35F;
  float flash_ = 0.0F;
  DWORD track_ = 0xFFE8EEEB;
  DWORD start_ = 0xFFD799FF;
  DWORD end_ = 0xFFFFB9ED;
};

class GlassPanelControl final : public DuiLib::CVerticalLayoutUI {
 public:
  [[nodiscard]] LPCTSTR GetClass() const override {
    return L"GlassPanelUI";
  }

  bool DoPaint(HDC dc,
               const RECT& paint,
               DuiLib::CControlUI* stop_control) override {
    if (!DuiLib::CVerticalLayoutUI::DoPaint(dc, paint, stop_control)) {
      return false;
    }

    const RECT bounds = GetPos();
    const Gdiplus::REAL width =
        static_cast<Gdiplus::REAL>(bounds.right - bounds.left);
    const Gdiplus::REAL height =
        static_cast<Gdiplus::REAL>(bounds.bottom - bounds.top);
    if (width <= 0.0F || height <= 0.0F) {
      return true;
    }
    Gdiplus::Graphics graphics(dc);
    graphics.SetSmoothingMode(Gdiplus::SmoothingModeAntiAlias);
    Gdiplus::Pen rim(Gdiplus::Color(32, 255, 255, 255), 1.0F);
    const Gdiplus::REAL radius = (std::min)(16.0F, height * 0.16F);
    graphics.DrawLine(&rim, bounds.left + radius, bounds.top + 1.0F,
                      bounds.right - radius, bounds.top + 1.0F);
    return true;
  }
};

class SuccessMarkControl final : public DuiLib::CControlUI {
 public:
  [[nodiscard]] LPCTSTR GetClass() const override {
    return L"SuccessMarkUI";
  }

  void SetAttribute(LPCTSTR name, LPCTSTR value) override {
    if (_wcsicmp(name, L"accentcolor") == 0) {
      accent_ = ParseArgb(value, accent_);
    } else if (_wcsicmp(name, L"inkcolor") == 0) {
      ink_ = ParseArgb(value, ink_);
    } else {
      CControlUI::SetAttribute(name, value);
    }
  }

  void Init() override {
    CControlUI::Init();
    last_tick_ = std::chrono::steady_clock::now();
    SetTimer(kSuccessTimer, 8);
  }

  void DoEvent(DuiLib::TEventUI& event) override {
    if (event.Type == DuiLib::UIEVENT_TIMER &&
        event.wParam == kSuccessTimer) {
      const auto now = std::chrono::steady_clock::now();
      const float elapsed = std::clamp(
          std::chrono::duration<float>(now - last_tick_).count(), 0.0F, 0.05F);
      last_tick_ = now;
      if (GetManager() != nullptr &&
          IsWindowVisible(GetManager()->GetPaintWindow())) {
        progress_ = (std::min)(1.0F, progress_ + elapsed * 2.25F);
        Invalidate();
      }
      if (progress_ >= 1.0F) {
        KillTimer(kSuccessTimer);
      }
      return;
    }
    CControlUI::DoEvent(event);
  }

  bool DoPaint(HDC dc,
               const RECT& paint,
               DuiLib::CControlUI* stop_control) override {
    if (!CControlUI::DoPaint(dc, paint, stop_control)) {
      return false;
    }
    const RECT raw = GetPos();
    const Gdiplus::REAL width = static_cast<Gdiplus::REAL>(raw.right - raw.left);
    const Gdiplus::REAL height = static_cast<Gdiplus::REAL>(raw.bottom - raw.top);
    const Gdiplus::REAL unit = (std::min)(width, height);
    const Gdiplus::REAL left = raw.left + (width - unit) * 0.5F;
    const Gdiplus::REAL top = raw.top + (height - unit) * 0.5F;
    Gdiplus::Graphics graphics(dc);
    graphics.SetSmoothingMode(Gdiplus::SmoothingModeAntiAlias);
    const BYTE halo_alpha = static_cast<BYTE>(
        16.0F + 14.0F * std::sin((std::min)(progress_, 1.0F) * 3.14159265F));
    Gdiplus::SolidBrush halo(AlphaColor(accent_, halo_alpha));
    graphics.FillEllipse(&halo, left, top, unit, unit);
    const Gdiplus::REAL inset = unit * 0.13F;
    Gdiplus::SolidBrush accent{Gdiplus::Color(accent_)};
    graphics.FillEllipse(&accent, left + inset, top + inset,
                         unit - inset * 2.0F, unit - inset * 2.0F);
    Gdiplus::Pen check(Gdiplus::Color(ink_), unit * 0.055F);
    check.SetStartCap(Gdiplus::LineCapRound);
    check.SetEndCap(Gdiplus::LineCapRound);
    const Gdiplus::PointF points[] = {
        {left + unit * 0.35F, top + unit * 0.51F},
        {left + unit * 0.47F, top + unit * 0.63F},
        {left + unit * 0.68F, top + unit * 0.38F}};
    const float check_progress = std::clamp(progress_, 0.0F, 1.0F);
    constexpr float first_segment_end = 0.42F;
    if (check_progress > 0.0F) {
      const float first_progress =
          (std::min)(1.0F, check_progress / first_segment_end);
      const Gdiplus::PointF first_end(
          points[0].X + (points[1].X - points[0].X) * first_progress,
          points[0].Y + (points[1].Y - points[0].Y) * first_progress);
      graphics.DrawLine(&check, points[0], first_end);
      if (check_progress > first_segment_end) {
        const float second_progress = (check_progress - first_segment_end) /
                                      (1.0F - first_segment_end);
        const Gdiplus::PointF second_end(
            points[1].X + (points[2].X - points[1].X) * second_progress,
            points[1].Y + (points[2].Y - points[1].Y) * second_progress);
        graphics.DrawLine(&check, points[1], second_end);
      }
    }
    return true;
  }

 private:
  static constexpr UINT kSuccessTimer = 0x4F51;
  DWORD accent_ = 0xFFD799FF;
  DWORD ink_ = 0xFF341643;
  std::chrono::steady_clock::time_point last_tick_{};
  float progress_ = 0.0F;
};

}  // namespace

class InstallerWindow::Impl final : public DuiLib::WindowImplBase {
 public:
  Impl(const ThemePackage& package,
       std::string page_name,
       ViewState state) noexcept
      : package_(package), page_name_(std::move(page_name)), state_(std::move(state)) {}

  ~Impl() override {
    if (IsWindow(m_hWnd)) {
      DestroyWindow(m_hWnd);
    }
  }

  [[nodiscard]] bool OpenHost(HWND owner) noexcept {
    try {
      DuiLib::CPaintManagerUI::SetInstance(CurrentModule());
      // Page XML imports the bundled IBM Plex Sans faces relative to the
      // selected theme directory.  DuiLib otherwise resolves imports against
      // the process working directory, which is usually the NSIS temp host
      // and makes the font silently fall back to a low quality system face.
      std::wstring resource_path = package_.root.wstring();
      // AddFontArray appends the import path directly to the resource root;
      // keep the separator so bundled faces resolve on both Win32 path forms.
      if (!resource_path.empty() && resource_path.back() != L'\\' &&
          resource_path.back() != L'/') {
        resource_path.push_back(L'\\');
      }
      DuiLib::CPaintManagerUI::SetResourcePath(resource_path.c_str());
      const ThreadDpiScope dpi_scope;
      // DuiLib lays out the theme in logical pixels.  Create the native
      // window at the same physical size that WM_DPICHANGED uses later;
      // otherwise a 150% display gets a 900x560 window containing a
      // 1350x840 layout and the right side is clipped on first show.
      const HMONITOR monitor =
          MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST);
      const UINT requested_dpi = MonitorEffectiveDpi(
          monitor, owner != nullptr ? GetDpiForWindow(owner)
                                    : GetDpiForSystem());
      const UINT effective_dpi = EffectiveDpiForMonitor(
          monitor, requested_dpi, package_.window.width,
          package_.window.height);
      const int window_width =
          MulDiv(package_.window.width, effective_dpi, 96);
      const int window_height =
          MulDiv(package_.window.height, effective_dpi, 96);
      const HWND created = Create(
          owner, L"Codex Accounts Setup",
          WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
          WS_EX_APPWINDOW, CW_USEDEFAULT, CW_USEDEFAULT,
           window_width, window_height);
      if (created == nullptr || !ready_) {
        if (error_.empty()) {
          error_ = "cannot create installer window";
        }
        return false;
      }
      HICON icon = reinterpret_cast<HICON>(
          ::SendMessageW(owner, WM_GETICON, ICON_BIG, 0));
      if (icon == nullptr) {
        icon = reinterpret_cast<HICON>(
            ::GetClassLongPtrW(owner, GCLP_HICON));
      }
      if (icon != nullptr) {
        ::SendMessageW(created, WM_SETICON, ICON_BIG,
                       reinterpret_cast<LPARAM>(icon));
        ::SendMessageW(created, WM_SETICON, ICON_SMALL,
                       reinterpret_cast<LPARAM>(icon));
      }

      // Once the HWND exists, Windows knows the real monitor DPI even when
      // the hidden NSIS owner reported 96 DPI during Create().  Reconcile
      // the logical theme scale and the physical window before the first
      // Show() so high-DPI displays never expose a clipped right half.
      const HMONITOR actual_monitor =
          MonitorFromWindow(created, MONITOR_DEFAULTTONEAREST);
      const UINT actual_dpi = GetDpiForWindow(created);
      const UINT actual_effective_dpi = EffectiveDpiForMonitor(
          actual_monitor, actual_dpi, package_.window.width,
          package_.window.height);
      m_pm.GetDPIObj()->SetScale(actual_effective_dpi);
      m_pm.ResetDPIAssets();
      const int actual_width =
          MulDiv(package_.window.width, actual_effective_dpi, 96);
      const int actual_height =
          MulDiv(package_.window.height, actual_effective_dpi, 96);
      SetWindowPos(created, nullptr, 0, 0, actual_width, actual_height,
                   SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
      ApplyWindowRegion(
          created,
          MulDiv(static_cast<int>(package_.window.radius), actual_effective_dpi,
                 96),
          actual_width, actual_height);
      if (m_pm.GetRoot() != nullptr) {
        m_pm.GetRoot()->NeedUpdate();
      }
      CenterOnWorkArea(created);
      ApplyModernCorners(created);
      return true;
    } catch (const std::exception& error) {
      error_ = error.what();
      return false;
    } catch (...) {
      error_ = "unexpected window creation failure";
      return false;
    }
  }

  void ShowInstaller(bool activate) noexcept {
    if (IsWindow(m_hWnd)) {
      if (!IsWindowVisible(m_hWnd)) {
        // A hidden HWND may still report the owner's 96 DPI.  Give Windows
        // one non-activating show pass so GetDpiForWindow() reflects the
        // actual monitor before the visible fade starts.
        ::ShowWindow(m_hWnd, SW_SHOWNOACTIVATE);
        ApplyCurrentDpi();
        ::ShowWindow(m_hWnd, SW_HIDE);
        const DWORD flags = AW_BLEND | (activate ? AW_ACTIVATE : 0);
        if (::AnimateWindow(m_hWnd, 190, flags) == FALSE) {
          ShowWindow(true, activate);
        }
      } else {
        ShowWindow(true, activate);
      }
      UpdateWindow(m_hWnd);
    }
  }

  void ApplyCurrentDpi() noexcept {
    if (!IsWindow(m_hWnd)) {
      return;
    }
    const HMONITOR monitor =
        MonitorFromWindow(m_hWnd, MONITOR_DEFAULTTONEAREST);
    const UINT effective_dpi = EffectiveDpiForMonitor(
        monitor, GetDpiForWindow(m_hWnd), package_.window.width,
        package_.window.height);
    m_pm.GetDPIObj()->SetScale(effective_dpi);
    m_pm.ResetDPIAssets();
    // Changing CPaintManager's scale does not rebuild an already attached
    // DuiLib tree. Recreate the current page so every control uses the new
    // logical-to-physical scale; otherwise the window grows to (for example)
    // 1350x840 while the old 900x560 tree remains in its top-left corner.
    if (rendered_dpi_ != 0 && rendered_dpi_ != effective_dpi) {
      if (AttachPage()) {
        rendered_dpi_ = effective_dpi;
      }
    }
    const int width = MulDiv(package_.window.width, effective_dpi, 96);
    const int height = MulDiv(package_.window.height, effective_dpi, 96);
    SetWindowPos(m_hWnd, nullptr, 0, 0, width, height,
                 SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    ApplyWindowRegion(
        m_hWnd,
        MulDiv(static_cast<int>(package_.window.radius), effective_dpi, 96),
        width, height);
    if (m_pm.GetRoot() != nullptr) {
      m_pm.GetRoot()->NeedUpdate();
    }
    CenterOnWorkArea(m_hWnd);
  }

  void HideInstaller() noexcept {
    if (IsWindow(m_hWnd)) {
      if (IsWindowVisible(m_hWnd) &&
          ::AnimateWindow(m_hWnd, 125, AW_BLEND | AW_HIDE) != FALSE) {
        return;
      }
      ShowWindow(false, false);
    }
  }

  void RefreshInstaller() noexcept {
    if (IsWindow(m_hWnd)) {
      UpdateWindow(m_hWnd);
    }
  }

  void CloseInstaller() noexcept {
    if (IsWindow(m_hWnd)) {
      Close(IDCANCEL);
    }
  }

  [[nodiscard]] bool SetPageName(const std::string& page_name) noexcept {
    suspended_page_name_.clear();
    event_ = WindowEvent::none;
    return TransitionToPage(page_name);
  }

  void SetProgressValue(int progress) noexcept {
    state_.progress = (std::max)(state_.progress, std::clamp(progress, 0, 100));
    if (DuiLib::CControlUI* control = m_pm.FindControl(L"progress");
        control != nullptr) {
      if (auto* animated = dynamic_cast<AnimatedProgressControl*>(control);
          animated != nullptr) {
        animated->SetValue(state_.progress);
      } else if (auto* native = dynamic_cast<DuiLib::CProgressUI*>(control);
                 native != nullptr) {
        native->SetValue(state_.progress);
      }
    }
    SetControlText(L"progress-value", std::to_string(state_.progress) + "%");
  }

  void ResetProgressValue(int progress) noexcept {
    state_.progress = std::clamp(progress, 0, 100);
    if (DuiLib::CControlUI* control = m_pm.FindControl(L"progress");
        control != nullptr) {
      if (auto* animated = dynamic_cast<AnimatedProgressControl*>(control);
          animated != nullptr) {
        animated->ResetValue(state_.progress);
      } else if (auto* native = dynamic_cast<DuiLib::CProgressUI*>(control);
                 native != nullptr) {
        native->SetValue(state_.progress);
      }
    }
    SetControlText(L"progress-value", std::to_string(state_.progress) + "%");
  }

  void SetStatusText(const std::string& status) noexcept {
    state_.status = status;
    SetControlText(L"status", status);
  }

  void SetNamedControlText(const std::string& name,
                           const std::string& value) noexcept {
    const std::wstring wide_name = Utf8ToWide(name);
    SetControlText(wide_name.c_str(), value);
  }

  void SetVersionText(const std::string& version) noexcept {
    state_.version = version;
    std::string label = package_.strings.count("install.version") != 0
                            ? package_.strings.at("install.version")
                            : version;
    const std::size_t placeholder = label.find("{version}");
    if (placeholder != std::string::npos) {
      label.replace(placeholder, 9, version);
    }
    SetControlText(L"version", label);
  }

  void SetInstallDirectoryText(const std::string& install_directory) noexcept {
    state_.install_directory = install_directory;
    SetControlText(L"install-directory", install_directory);
  }

  void SetPerUserInstallDirectoryText(
      const std::string& install_directory) noexcept {
    state_.per_user_install_directory = install_directory;
    SetControlText(L"per-user-install-directory", install_directory);
    if (page_name_ == "welcome" && selected_scope_ == InstallScope::current) {
      SetInstallDirectoryText(install_directory);
    }
  }

  void SetPerMachineInstallDirectoryText(
      const std::string& install_directory) noexcept {
    state_.per_machine_install_directory = install_directory;
    SetControlText(L"per-machine-install-directory", install_directory);
    if (page_name_ == "welcome" && selected_scope_ == InstallScope::all) {
      SetInstallDirectoryText(install_directory);
    }
  }

  void ResetWindowEvent() noexcept {
    event_ = WindowEvent::none;
  }

  void ChangeFeature(int direction) noexcept {
    if (package_.features.empty()) {
      return;
    }
    const auto count = static_cast<int>(package_.features.size());
    int index = static_cast<int>(state_.feature_index % package_.features.size());
    index = (index + direction) % count;
    if (index < 0) {
      index += count;
    }
    state_.feature_index = static_cast<std::size_t>(index);
    const auto& feature = package_.features[state_.feature_index];
    SetControlText(L"feature-title", feature.title);
    SetControlText(L"feature-description", feature.description);
    SetControlText(L"feature-counter",
                   std::to_string(index + 1) + " / " + std::to_string(count));
    UpdateFeatureActionVisibility();
  }

  void OpenFeatureAction() noexcept {
    if (package_.features.empty()) {
      return;
    }
    const auto& feature =
        package_.features[state_.feature_index % package_.features.size()];
    if (!feature.action_url || feature.action_url->rfind("https://", 0) != 0) {
      return;
    }
    const std::wstring url = Utf8ToWide(*feature.action_url);
    if (!url.empty()) {
      ShellExecuteW(m_hWnd, L"open", url.c_str(), nullptr, nullptr,
                    SW_SHOWNORMAL);
    }
  }

  void OpenLegalLink(const DuiLib::CDuiString& target) noexcept {
    const wchar_t* url = nullptr;
    if (_wcsicmp(target.GetData(), L"terms") == 0) {
      url = L"https://onekey.so/terms";
    } else if (_wcsicmp(target.GetData(), L"privacy") == 0) {
      url = L"https://onekey.so/privacy";
    }
    if (url != nullptr) {
      ShellExecuteW(m_hWnd, L"open", url, nullptr, nullptr, SW_SHOWNORMAL);
    }
  }

  [[nodiscard]] WindowEvent event() const noexcept { return event_; }
  [[nodiscard]] const std::string& error() const noexcept { return error_; }

 protected:
  [[nodiscard]] LPCTSTR GetWindowClassName() const override {
    return L"NsisDuiLibInstallerWindow";
  }

  [[nodiscard]] DuiLib::CDuiString GetSkinFile() override {
    return rendered_xml_.c_str();
  }

  DuiLib::CControlUI* CreateControl(LPCTSTR class_name) override {
    if (_wcsicmp(class_name, L"SoftButton") == 0) {
      return new SoftButtonControl();
    }
    if (_wcsicmp(class_name, L"OrbitIndicator") == 0) {
      return new OrbitIndicatorControl();
    }
    if (_wcsicmp(class_name, L"BrandMark") == 0) {
      return new BrandMarkControl();
    }
    if (_wcsicmp(class_name, L"CaptionButton") == 0) {
      return new CaptionButtonControl();
    }
    if (_wcsicmp(class_name, L"LegalNotice") == 0) {
      return new LegalNoticeControl();
    }
    if (_wcsicmp(class_name, L"LightBackdrop") == 0) {
      return new LightBackdropControl();
    }
    if (_wcsicmp(class_name, L"AnimatedProgress") == 0) {
      return new AnimatedProgressControl();
    }
    if (_wcsicmp(class_name, L"GlassPanel") == 0) {
      return new GlassPanelControl();
    }
    if (_wcsicmp(class_name, L"SuccessMark") == 0) {
      return new SuccessMarkControl();
    }
    return nullptr;
  }

  LRESULT OnCreate(UINT,
                   WPARAM,
                   LPARAM,
                   BOOL& handled) override {
    handled = TRUE;
    LONG style = GetWindowLongW(m_hWnd, GWL_STYLE);
    SetWindowLongW(m_hWnd, GWL_STYLE,
                   (style & ~WS_CAPTION) | WS_CLIPSIBLINGS | WS_CLIPCHILDREN);

    m_pm.Init(m_hWnd);
    const HMONITOR monitor =
        MonitorFromWindow(m_hWnd, MONITOR_DEFAULTTONEAREST);
    const UINT requested_dpi =
        MonitorEffectiveDpi(monitor, GetDpiForWindow(m_hWnd));
    const UINT effective_dpi = EffectiveDpiForMonitor(
        monitor, requested_dpi, package_.window.width,
        package_.window.height);
    // The NSIS owner can be system-DPI aware while this window is
    // per-monitor aware.  In that case OpenHost may have created the window
    // at the logical size.  Correct it here, after Windows has assigned the
    // actual monitor DPI, before the first page is attached.
    const int scaled_width =
        MulDiv(package_.window.width, effective_dpi, 96);
    const int scaled_height =
        MulDiv(package_.window.height, effective_dpi, 96);
    SetWindowPos(m_hWnd, nullptr, 0, 0, scaled_width, scaled_height,
                 SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    ApplyWindowRegion(
        m_hWnd,
        MulDiv(static_cast<int>(package_.window.radius), effective_dpi, 96),
        scaled_width, scaled_height);
    m_pm.GetDPIObj()->SetScale(effective_dpi);
    m_pm.AddPreMessageFilter(this);
    m_pm.AddNotifier(this);
    if (!AttachPage()) {
      m_pm.RemoveNotifier(this);
      m_pm.RemovePreMessageFilter(this);
      return -1;
    }
    // AttachPage lays out controls using the scale that was active when the
    // page was created. Keep that value so a hidden NSIS owner that reports
    // system DPI (96) can be reconciled after the real monitor DPI becomes
    // available in OpenHost/WM_SHOWWINDOW.
    rendered_dpi_ = effective_dpi;
    ready_ = true;
    return 0;
  }

  LRESULT OnClose(UINT,
                  WPARAM,
                  LPARAM,
                  BOOL& handled) override {
    if (ShouldConfirmExit()) {
      QueuePageTransition(kShowExitConfirmation);
      handled = TRUE;
      return 0;
    }
    if (IsExitConfirmationPage()) {
      QueuePageTransition(kRestoreSuspendedPage);
      handled = TRUE;
      return 0;
    }
    if (event_ == WindowEvent::none) {
      event_ = WindowEvent::closed;
    }
    if (IsOperationPage()) {
      handled = TRUE;
      return 0;
    }
    handled = FALSE;
    return 0;
  }

  LRESULT OnDestroy(UINT,
                    WPARAM,
                    LPARAM,
                    BOOL& handled) override {
    ready_ = false;
    handled = FALSE;
    return 0;
  }

  LRESULT HandleMessage(UINT message,
                        WPARAM wparam,
                        LPARAM lparam) override {
    if (message == kShowExitConfirmation) {
      page_transition_pending_ = false;
      ShowExitConfirmation();
      return 0;
    }
    if (message == kRestoreSuspendedPage) {
      page_transition_pending_ = false;
      RestoreSuspendedPage();
      return 0;
    }
    if (message == WM_SHOWWINDOW && wparam != FALSE) {
      // The real per-monitor DPI becomes available when this HWND is shown;
      // correct the initial logical-size window before its first paint.
      ApplyCurrentDpi();
    }
    if (message == WM_DPICHANGED) {
      const auto suggested = reinterpret_cast<const RECT*>(lparam);
      const HMONITOR monitor =
          MonitorFromRect(suggested, MONITOR_DEFAULTTONEAREST);
      RECT work_area = *suggested;
      const UINT effective_dpi = EffectiveDpiForMonitor(
          monitor, HIWORD(wparam), package_.window.width,
          package_.window.height, &work_area);
      m_pm.GetDPIObj()->SetScale(effective_dpi);
      m_pm.ResetDPIAssets();
      if (rendered_dpi_ != 0 && rendered_dpi_ != effective_dpi) {
        if (AttachPage()) {
          rendered_dpi_ = effective_dpi;
        }
      }
      const int width = MulDiv(package_.window.width, effective_dpi, 96);
      const int height = MulDiv(package_.window.height, effective_dpi, 96);
      const int left = work_area.left +
                       ((work_area.right - work_area.left) - width) / 2;
      const int top = work_area.top +
                      ((work_area.bottom - work_area.top) - height) / 2;
      SetWindowPos(m_hWnd, nullptr, left, top, width, height,
                   SWP_NOZORDER | SWP_NOACTIVATE);
      ApplyWindowRegion(
          m_hWnd,
          MulDiv(static_cast<int>(package_.window.radius), effective_dpi, 96),
          width, height);
      if (m_pm.GetRoot() != nullptr) {
        m_pm.GetRoot()->NeedUpdate();
      }
      return 0;
    }
    const LRESULT result =
        WindowImplBase::HandleMessage(message, wparam, lparam);
    if (message == WM_SIZE && IsWindow(m_hWnd)) {
      // WinImplBase has its own legacy region update.  Re-apply the region
      // after it has processed WM_SIZE so the final mask uses the physical
      // client size even when the NSIS thread is system-DPI aware.
      const UINT dpi = GetDpiForWindow(m_hWnd);
      ApplyWindowRegion(
          m_hWnd,
          MulDiv(static_cast<int>(package_.window.radius), dpi, 96),
          // WM_SIZE is expressed in the window's active coordinate space.
          // The installer is fixed to its theme dimensions, so use the
          // physical size that was applied during the DPI pass.
          MulDiv(package_.window.width, dpi, 96),
          MulDiv(package_.window.height, dpi, 96));
    }
    return result;
  }

  void Notify(DuiLib::TNotifyUI& message) override {
    if (message.pSender == nullptr) {
      return;
    }
    if (message.sType == DUI_MSGTYPE_LINK) {
      auto* legal_notice =
          dynamic_cast<LegalNoticeControl*>(message.pSender);
      if (legal_notice != nullptr) {
        OpenLegalLink(
            legal_notice->LinkTarget(static_cast<int>(message.wParam)));
        return;
      }
      auto* text = dynamic_cast<DuiLib::CTextUI*>(message.pSender);
      if (text != nullptr) {
        const auto* target =
            text->GetLinkContent(static_cast<int>(message.wParam));
        if (target != nullptr) {
          OpenLegalLink(*target);
        }
      }
      return;
    }
    if (message.sType != DUI_MSGTYPE_CLICK) {
      return;
    }
    const DuiLib::CDuiString name = message.pSender->GetName();
    if (name == L"closebtn") {
      if (ShouldConfirmExit()) {
        QueuePageTransition(kShowExitConfirmation);
      } else if (IsExitConfirmationPage()) {
        QueuePageTransition(kRestoreSuspendedPage);
      } else {
        event_ = WindowEvent::closed;
      }
      if (!IsOperationPage() && !IsExitConfirmationPage()) {
        Close();
      }
    } else if (name == L"minbtn") {
      SendMessage(WM_SYSCOMMAND, SC_MINIMIZE);
    } else if (name == L"cancel") {
      if (ShouldConfirmExit()) {
        QueuePageTransition(kShowExitConfirmation);
      } else {
        event_ = WindowEvent::cancel;
      }
      if (!IsOperationPage() && !IsExitConfirmationPage()) {
        Close(IDCANCEL);
      }
    } else if (name == L"setup-language") {
      event_ = WindowEvent::language;
    } else if (name == L"setup-theme") {
      event_ = WindowEvent::theme;
    } else if (name == L"setup-sound") {
      event_ = WindowEvent::sound;
    } else if (name == L"exit-stay") {
      QueuePageTransition(kRestoreSuspendedPage);
    } else if (name == L"exit-confirm") {
      event_ = WindowEvent::cancel;
    } else if (name == L"primary") {
      if (page_name_ == "welcome") {
        event_ = selected_scope_ == InstallScope::all
                     ? WindowEvent::primary_all
                     : WindowEvent::primary_current;
      } else {
        event_ = WindowEvent::primary;
      }
      if (page_name_ == "finish" || page_name_ == "uninstallFinish") {
        Close(IDOK);
      }
    } else if (name == L"browse") {
      if (page_name_ == "welcome") {
        event_ = selected_scope_ == InstallScope::all
                     ? WindowEvent::browse_all
                     : WindowEvent::browse_current;
      } else {
        event_ = WindowEvent::browse;
      }
    } else if (name == L"more-options") {
      advanced_options_visible_ = !advanced_options_visible_;
      if (advanced_options_visible_) {
        advanced_options_ever_opened_ = true;
      }
      UpdateWelcomeControls();
    } else if (name == L"scope-current") {
      if (page_name_ == "welcome") {
        SelectInstallScope(InstallScope::current);
      } else {
        event_ = WindowEvent::scope_current;
      }
    } else if (name == L"scope-all") {
      if (page_name_ == "welcome") {
        SelectInstallScope(InstallScope::all);
      } else {
        event_ = WindowEvent::scope_all;
      }
    } else if (name == L"feature-prev") {
      ChangeFeature(-1);
    } else if (name == L"feature-next") {
      ChangeFeature(1);
    } else if (name == L"feature-action") {
      OpenFeatureAction();
    }
  }

  void OnFinalMessage(HWND window) override {
    ready_ = false;
    WindowImplBase::OnFinalMessage(window);
  }

 private:
  enum class InstallScope { current, all };

  static constexpr UINT kShowExitConfirmation = WM_APP + 0x4F4B;
  static constexpr UINT kRestoreSuspendedPage = WM_APP + 0x4F4C;

  [[nodiscard]] bool IsOperationPage() const noexcept {
    return page_name_ == "installing" || page_name_ == "uninstalling";
  }

  [[nodiscard]] bool ShouldConfirmExit() const noexcept {
    return page_name_ == "installing";
  }

  [[nodiscard]] bool IsExitConfirmationPage() const noexcept {
    return page_name_ == "confirmExit";
  }

  void QueuePageTransition(UINT message) noexcept {
    if (!page_transition_pending_ && IsWindow(m_hWnd)) {
      page_transition_pending_ = true;
      ::PostMessageW(m_hWnd, message, 0, 0);
    }
  }

  void ShowExitConfirmation() noexcept {
    suspended_page_name_ = page_name_;
    event_ = WindowEvent::none;
    if (!TransitionToPage("confirmExit")) {
      const std::string previous = std::exchange(suspended_page_name_, {});
      (void)TransitionToPage(previous);
    }
  }

  void RestoreSuspendedPage() noexcept {
    if (suspended_page_name_.empty()) {
      return;
    }
    const std::string previous = std::exchange(suspended_page_name_, {});
    event_ = WindowEvent::none;
    (void)TransitionToPage(previous);
  }

  [[nodiscard]] bool TransitionToPage(const std::string& next_page) noexcept {
    const bool page_changes = page_name_ != next_page;
    const bool animate = page_changes && IsWindow(m_hWnd) &&
                         IsWindowVisible(m_hWnd);
    const bool restore_focus = animate && GetForegroundWindow() == m_hWnd;
    if (animate &&
        ::AnimateWindow(m_hWnd, 82, AW_BLEND | AW_HIDE) == FALSE) {
      // Continue with a direct page update if Windows declines the fade.
    }

    const std::string previous = page_name_;
    page_name_ = next_page;
    if (!AttachPage()) {
      page_name_ = previous;
      (void)AttachPage();
      if (animate) {
        const DWORD flags = AW_BLEND | (restore_focus ? AW_ACTIVATE : 0);
        if (::AnimateWindow(m_hWnd, 145, flags) == FALSE) {
          ShowWindow(true, restore_focus);
        }
      }
      return false;
    }

    if (animate) {
      const DWORD flags = AW_BLEND | (restore_focus ? AW_ACTIVATE : 0);
      if (::AnimateWindow(m_hWnd, 175, flags) == FALSE) {
        ShowWindow(true, restore_focus);
      }
    }
    return true;
  }

  [[nodiscard]] bool AttachPage() noexcept {
    const RenderResult rendered = RenderPage(package_, page_name_, state_);
    if (!rendered) {
      error_ = rendered.error;
      return false;
    }
    rendered_xml_ = *rendered.xml;

    DuiLib::CDialogBuilder builder;
    DuiLib::CControlUI* root =
        builder.Create(rendered_xml_.c_str(), 0, this, &m_pm);
    if (root == nullptr) {
      wchar_t message[512] = {};
      builder.GetLastErrorMessage(message, std::size(message));
      error_ = "DuiLib could not parse the rendered page";
      if (message[0] != L'\0') {
        const int length = WideCharToMultiByte(CP_UTF8, 0, message, -1, nullptr,
                                                0, nullptr, nullptr);
        if (length > 1) {
          std::string detail(static_cast<std::size_t>(length), '\0');
          WideCharToMultiByte(CP_UTF8, 0, message, -1, detail.data(), length,
                              nullptr, nullptr);
          detail.resize(static_cast<std::size_t>(length - 1));
          error_ += ": " + detail;
        }
      }
      return false;
    }
    if (!m_pm.AttachDialog(root)) {
      delete root;
      error_ = "DuiLib could not attach the rendered page";
      return false;
    }
    UpdateFeatureActionVisibility();
    UpdateWelcomeControls();
    error_.clear();
    return true;
  }

  void SelectInstallScope(InstallScope scope) noexcept {
    selected_scope_ = scope;
    const std::string& directory =
        scope == InstallScope::all ? state_.per_machine_install_directory
                                   : state_.per_user_install_directory;
    SetInstallDirectoryText(directory);
    UpdateWelcomeControls();
  }

  void UpdateScopeButton(const wchar_t* name, bool selected) noexcept {
    auto* button = dynamic_cast<DuiLib::CButtonUI*>(m_pm.FindControl(name));
    if (button == nullptr) {
      return;
    }
    const DWORD brand = ThemeColor("brand", 0xFF44D62C);
    const DWORD brand_soft = ThemeColor("brandSoft", 0xFFDFFBD9);
    const DWORD surface = 0xCCFFFFFF;
    const DWORD border = ThemeColor("border", 0x141A2821);
    const DWORD text = ThemeColor("textPrimary", 0xFF151918);
    button->SetBkColor(selected ? brand_soft : surface);
    button->SetBorderColor(selected ? brand : border);
    button->SetTextColor(text);
    button->SetFont(selected ? 3 : 0);
    button->SetHotBkColor(selected ? brand_soft : brand_soft);
    button->SetPushedBkColor(selected ? brand_soft : 0xFFE6ECE9);
  }

  [[nodiscard]] DWORD ThemeColor(const char* name,
                                 DWORD fallback) const noexcept {
    const auto color = package_.colors.find(name);
    if (color == package_.colors.end()) {
      return fallback;
    }
    const std::wstring value = Utf8ToWide(color->second);
    return ParseArgb(value.c_str(), fallback);
  }

  void UpdateWelcomeControls() noexcept {
    if (page_name_ != "welcome") {
      return;
    }
    if (DuiLib::CControlUI* options =
            m_pm.FindControl(L"advanced-options");
        options != nullptr) {
      options->SetVisible(advanced_options_visible_);
    }
    const auto key = advanced_options_visible_ ? "welcome.lessOptions"
                                                : "welcome.moreOptions";
    const auto label = package_.strings.find(key);
    if (label != package_.strings.end()) {
      SetControlText(L"more-options", label->second);
    }
    const auto primary_key = advanced_options_ever_opened_ ? "welcome.install"
                                                            : "welcome.start";
    const auto primary_label = package_.strings.find(primary_key);
    if (primary_label != package_.strings.end()) {
      SetControlText(L"primary", primary_label->second);
    }
    UpdateScopeButton(L"scope-current",
                      selected_scope_ == InstallScope::current);
    UpdateScopeButton(L"scope-all", selected_scope_ == InstallScope::all);
    if (m_pm.GetRoot() != nullptr) {
      m_pm.GetRoot()->NeedUpdate();
    }
    if (IsWindow(m_hWnd)) {
      InvalidateRect(m_hWnd, nullptr, FALSE);
    }
  }

  void UpdateFeatureActionVisibility() noexcept {
    DuiLib::CControlUI* action = m_pm.FindControl(L"feature-action");
    if (action == nullptr) {
      return;
    }
    const bool visible = !package_.features.empty() &&
                         package_.features[state_.feature_index %
                                           package_.features.size()]
                             .action_url.has_value();
    action->SetVisible(visible);
  }

  void SetControlText(const wchar_t* name, const std::string& value) noexcept {
    if (DuiLib::CControlUI* control = m_pm.FindControl(name); control != nullptr) {
      const std::wstring wide = Utf8ToWide(value);
      control->SetText(wide.c_str());
      if (IsWindow(m_hWnd)) {
        InvalidateRect(m_hWnd, nullptr, FALSE);
      }
    }
  }

  ThemePackage package_;
  std::string page_name_;
  std::string suspended_page_name_;
  ViewState state_;
  std::wstring rendered_xml_;
  std::string error_;
  WindowEvent event_ = WindowEvent::none;
  InstallScope selected_scope_ = InstallScope::current;
  bool advanced_options_visible_ = false;
  bool advanced_options_ever_opened_ = false;
  bool page_transition_pending_ = false;
  UINT rendered_dpi_ = 0;
  bool ready_ = false;
};

InstallerWindow::InstallerWindow() = default;

InstallerWindow::~InstallerWindow() = default;

bool InstallerWindow::Create(HWND owner,
                             const ThemePackage& package,
                             const std::string& page_name,
                             const ViewState& state) noexcept {
  impl_ = std::make_unique<Impl>(package, page_name, state);
  return impl_->OpenHost(owner);
}

void InstallerWindow::Show(bool activate) noexcept {
  if (impl_) {
    impl_->ShowInstaller(activate);
  }
}

void InstallerWindow::Hide() noexcept {
  if (impl_) {
    impl_->HideInstaller();
  }
}

void InstallerWindow::Refresh() noexcept {
  if (impl_) {
    impl_->RefreshInstaller();
  }
}

void InstallerWindow::Close() noexcept {
  if (impl_) {
    impl_->CloseInstaller();
  }
}

int InstallerWindow::RunMessageLoop() noexcept {
  if (!impl_ || !IsWindow(impl_->GetHWND())) {
    return 1;
  }
  MSG message = {};
  while (IsWindow(impl_->GetHWND())) {
    const BOOL result = GetMessageW(&message, nullptr, 0, 0);
    if (result <= 0) {
      return result == 0 ? static_cast<int>(message.wParam) : 1;
    }
    if (!DuiLib::CPaintManagerUI::TranslateMessage(&message)) {
      TranslateMessage(&message);
      DispatchMessageW(&message);
    }
  }
  return 0;
}

WindowEvent InstallerWindow::WaitForEvent() noexcept {
  if (!impl_) {
    return WindowEvent::closed;
  }
  MSG message = {};
  while (impl_->event() == WindowEvent::none &&
         IsWindow(impl_->GetHWND())) {
    const BOOL result = GetMessageW(&message, nullptr, 0, 0);
    if (result <= 0) {
      break;
    }
    if (!DuiLib::CPaintManagerUI::TranslateMessage(&message)) {
      TranslateMessage(&message);
      DispatchMessageW(&message);
    }
  }
  return impl_->event();
}

void InstallerWindow::PumpMessages() noexcept {
  MSG message = {};
  while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
    if (message.message == WM_QUIT) {
      PostQuitMessage(static_cast<int>(message.wParam));
      break;
    }
    if (!DuiLib::CPaintManagerUI::TranslateMessage(&message)) {
      TranslateMessage(&message);
      DispatchMessageW(&message);
    }
  }
}

bool InstallerWindow::SetPage(const std::string& page_name) noexcept {
  return impl_ && impl_->SetPageName(page_name);
}

void InstallerWindow::SetProgress(int progress) noexcept {
  if (impl_) {
    impl_->SetProgressValue(progress);
  }
}

void InstallerWindow::ResetProgress(int progress) noexcept {
  if (impl_) {
    impl_->ResetProgressValue(progress);
  }
}

void InstallerWindow::SetStatus(const std::string& status) noexcept {
  if (impl_) {
    impl_->SetStatusText(status);
  }
}

void InstallerWindow::SetControlText(const std::string& name,
                                     const std::string& text) noexcept {
  if (impl_) {
    impl_->SetNamedControlText(name, text);
  }
}

void InstallerWindow::SetVersion(const std::string& version) noexcept {
  if (impl_) {
    impl_->SetVersionText(version);
  }
}

void InstallerWindow::SetInstallDirectory(
    const std::string& install_directory) noexcept {
  if (impl_) {
    impl_->SetInstallDirectoryText(install_directory);
  }
}

void InstallerWindow::SetPerUserInstallDirectory(
    const std::string& install_directory) noexcept {
  if (impl_) {
    impl_->SetPerUserInstallDirectoryText(install_directory);
  }
}

void InstallerWindow::SetPerMachineInstallDirectory(
    const std::string& install_directory) noexcept {
  if (impl_) {
    impl_->SetPerMachineInstallDirectoryText(install_directory);
  }
}

void InstallerWindow::ResetEvent() noexcept {
  if (impl_) {
    impl_->ResetWindowEvent();
  }
}

void InstallerWindow::NextFeature(int direction) noexcept {
  if (impl_) {
    impl_->ChangeFeature(direction);
  }
}

HWND InstallerWindow::hwnd() const noexcept {
  return impl_ ? impl_->GetHWND() : nullptr;
}

WindowEvent InstallerWindow::event() const noexcept {
  return impl_ ? impl_->event() : WindowEvent::none;
}

const std::string& InstallerWindow::error() const noexcept {
  static const std::string empty;
  return impl_ ? impl_->error() : empty;
}

}  // namespace nsis_dui
