#pragma once

#include <algorithm>

namespace nsis_dui {

class ProgressTracker final {
 public:
  [[nodiscard]] int Update(int value) noexcept {
    value_ = (std::max)(value_, std::clamp(value, 0, 100));
    return value_;
  }

  [[nodiscard]] int UpdateManual(int value) noexcept {
    manual_updates_ = true;
    return Update(value);
  }

  [[nodiscard]] int ResetForNative(int value = 0) noexcept {
    manual_updates_ = false;
    value_ = std::clamp(value, 0, 100);
    return value_;
  }

  [[nodiscard]] int value() const noexcept { return value_; }
  [[nodiscard]] bool AllowsNativeUpdates() const noexcept {
    return !manual_updates_;
  }

 private:
  int value_ = 0;
  bool manual_updates_ = false;
};

}  // namespace nsis_dui
