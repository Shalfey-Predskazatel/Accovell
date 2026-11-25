#include "nsis_dui/progress.hpp"

int main() {
  nsis_dui::ProgressTracker progress;
  if (!progress.AllowsNativeUpdates()) {
    return 1;
  }
  if (progress.Update(18) != 18 || progress.Update(43) != 43 ||
      progress.Update(39) != 43 || progress.Update(101) != 100) {
    return 2;
  }
  if (progress.ResetForNative() != 0 || !progress.AllowsNativeUpdates()) {
    return 3;
  }
  if (progress.UpdateManual(72) != 72 || progress.AllowsNativeUpdates()) {
    return 4;
  }
  if (progress.ResetForNative() != 0 || !progress.AllowsNativeUpdates() ||
      progress.Update(7) != 7) {
    return 5;
  }
  return 0;
}
