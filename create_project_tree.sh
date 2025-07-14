#!/usr/bin/env bash
# Datei: create_project_tree.sh
# Erstellt die angegebene Ordner- & Dateistruktur unterhalb des aktuellen Pfads.

set -euo pipefail

# Verzeichnisse (mit führendem src/)
declare -a DIRS=(
  "src"
  "src/app"
  "src/core"
  "src/config"
  "src/utils"
)

# Dateien (komplett relativ angegeben)
declare -a FILES=(
  "src/main.rs"

  "src/app/mod.rs"
  "src/app/main_window.rs"

  "src/core/mod.rs"
  "src/core/scanner.rs"
  "src/core/file_handler.rs"
  "src/core/search.rs"
  "src/core/tree_generator.rs"

  "src/config/mod.rs"
  "src/config/settings.rs"

  "src/utils/mod.rs"
  "src/utils/file_detection.rs"
)

echo "➤ Erstelle Verzeichnisse …"
for d in "${DIRS[@]}"; do
  mkdir -p "$d"
done

echo "➤ Erstelle Dateien …"
for f in "${FILES[@]}"; do
  # lege Datei nur an, falls sie noch nicht existiert
  [[ -e "$f" ]] || touch "$f"
done

echo "✅ Struktur erfolgreich angelegt."