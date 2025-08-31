#!/usr/bin/env bash
set -euo pipefail

# Create new test folders/files for Coverage Hardening (Step 4.4.5)
# This script only creates files if they don't already exist.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UI_DIR="$ROOT_DIR/src/ui"
TEST_DIR="$UI_DIR/src/tests"
MOD_DIR="$TEST_DIR/modules"

mkdir -p "$MOD_DIR"

create_file() {
  local file="$1"
  if [[ -e "$file" ]]; then
    echo "SKIP  $file (exists)"
  else
    echo "CREATE $file"
    cat > "$file" <<'EOF'
// Placeholder — paste the test content from the assistant's code block.
export {};
EOF
  fi
}

create_file "$MOD_DIR/undo.test.ts"
create_file "$MOD_DIR/commands.test.ts"
create_file "$MOD_DIR/toast.test.ts"
create_file "$MOD_DIR/clipboard.test.ts"
create_file "$MOD_DIR/keyboard.test.ts"
create_file "$MOD_DIR/i18n.test.ts"

echo "✔ Done. Files created. Now paste the provided contents into each file."
echo "NOTE: You'll also paste the updated vitest.config.ts content."
