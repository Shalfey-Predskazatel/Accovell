#include "nsis_dui/version.hpp"

#include <cstdlib>

int main() {
  return nsis_dui::Version() == "0.1.0" ? EXIT_SUCCESS : EXIT_FAILURE;
}

