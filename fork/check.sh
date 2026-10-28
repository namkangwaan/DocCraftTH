#!/usr/bin/env bash
# Fork guard: fails when a fork-only file or symbol is gone, then runs the invariant tests.
# Usage: bash fork/check.sh [manifest|tests|all]
set -u -o pipefail
cd "$(dirname "$0")/.."
mode="${1:-all}"
fail=0

check_manifest() {
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in ''|'#'*) continue ;; esac
    path="${line%%::*}"
    if [ "$path" = "$line" ]; then
      [ -e "$path" ] || { echo "MISSING FILE   $path"; fail=1; }
    else
      needle="${line#*::}"
      if ! grep -rqF -- "$needle" "$path" 2>/dev/null; then
        echo "MISSING SYMBOL $path :: $needle"; fail=1
      fi
    fi
  done < fork/manifest.txt
}

# Runs cargo test; on Windows an antivirus scan can lock a build file (os error 32) or kill a
# rustc run. Those failures are transient and cargo resumes where it stopped, so retry unless
# the log shows a real compile error or a failed test.
t() {
  local log attempt
  log="$(mktemp)"
  for attempt in 1 2 3 4 5 6; do
    if timeout 1800 cargo test -j 2 "$@" 2>&1 | tee "$log"; then return 0; fi
    if grep -qE 'error\[E[0-9]+\]|test result: FAILED|panicked at' "$log"; then return 1; fi
    echo "retry $attempt: transient build failure"
  done
  return 1
}

run_tests() {
  t -p pdfcraft-engine --test doc_title_names --test thai_ocr || fail=1
  t -p pdfcraft-engine --lib thai_dates_use_buddhist_era || fail=1
  t -p pdfcraft-render --lib thai || fail=1
  t -p pdfcraft-render --lib combining_mark || fail=1
  t -p pdfcraft-ui-egui --test it fonts || fail=1
  t -p pdfcraft-ui-egui --test it thai_wrap || fail=1
  t -p pdfcraft-ui-egui --lib th_tests || fail=1
}

[ "$mode" = tests ] || check_manifest
[ "$mode" = manifest ] || run_tests
if [ "$fail" = 0 ]; then echo "FORK GUARD OK"; else echo "FORK GUARD FAILED"; exit 1; fi
