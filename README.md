# ContextFileConcat

Eine performante Rust-Anwendung zum Zusammenfassen von Dateien in ein einziges Text-Dokument - perfekt für KI-Context-Generierung.

## Features

✅ **Intuitive GUI** - Modernes Interface mit macOS-Integration  
✅ **Async Directory Scanning** - Responsive UI auch bei großen Projekten  
✅ **Smart File Detection** - Automatische Erkennung von Text/Binary-Dateien  
✅ **Flexible Suche & Filter** - Regex-Support, Case-Sensitivity, Extension-Filter  
✅ **Ignore Patterns** - .gitignore-style Patterns (node_modules/, target/, etc.)  
✅ **File Preview** - Vorschau der ersten Zeilen direkt in der GUI  
✅ **ASCII Directory Tree** - Optionale Verzeichnisstruktur-Ausgabe  
✅ **Cross-Platform Config** - Automatische Konfigurationsspeicherung  
✅ **Progress Tracking** - Echtzeit-Fortschrittsanzeige  
✅ **Large File Handling** - 100MB Limit mit Warning-System

## Installation & Setup

### 1. Rust installieren (falls noch nicht gemacht)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### 2. Projekt erstellen

```bash
cargo new context-file-concat
cd context-file-concat
```

### 3. Dateien erstellen

Erstelle folgende Verzeichnisstruktur:

```
src/
├── main.rs
├── app/
│   ├── mod.rs
│   └── main_window.rs
├── core/
│   ├── mod.rs
│   ├── scanner.rs
│   ├── file_handler.rs
│   ├── search.rs
│   └── tree_generator.rs
├── config/
│   ├── mod.rs
│   └── settings.rs
└── utils/
    ├── mod.rs
    └── file_detection.rs
```

Kopiere alle Dateien aus den Artifacts in die entsprechenden Ordner.

### 4. Programm kompilieren und ausführen

**Debug-Version (für Entwicklung):**

```bash
cargo run
```

**Release-Version (optimiert):**

```bash
cargo build --release
./target/release/context-file-concat
```

**Cross-Platform kompilieren:**

```bash
# Für Windows (von macOS/Linux aus)
cargo build --release --target x86_64-pc-windows-gnu

# Für Linux (von macOS aus)
cargo build --release --target x86_64-unknown-linux-gnu
```

## Verwendung

1. **Directory auswählen**: Klicke auf "Select Directory" oder gib den Pfad manuell ein
2. **Scannen**: Klicke "Scan" um alle Dateien zu erfassen
3. **Filtern**: Nutze Search-Box, Extension-Filter oder Ignore-Patterns
4. **Auswählen**: Wähle Dateien mit Checkboxes aus (oder "Select All")
5. **Output konfigurieren**: Setze Ausgabepfad und Dateiname
6. **Generieren**: Klicke "Generate" für das finale concatenated File

## Konfiguration

Die App speichert Einstellungen automatisch in:

- **macOS**: `~/Library/Application Support/ContextFileConcat/`
- **Linux**: `~/.config/ContextFileConcat/`
- **Windows**: `%APPDATA%/ContextFileConcat/`

Du kannst Configs auch manuell exportieren/importieren über die GUI-Buttons.

## Ausgabe-Format

```
# ContextFileConcat Output
# Generated: 2025-07-14 15:30:45
# Total files: 42

/path/to/file1.rs
----------------------------------------------------
// File content here...
----------------------------------------------------

/path/to/file2.py
----------------------------------------------------
# File content here...
----------------------------------------------------

# DIRECTORY TREE (optional)
====================================================
project-root/
├── 📁 src/
│   ├── 📄 main.rs
│   └── 📁 components/
│       └── 📄 button.rs
└── 📄 README.md
====================================================
```

## Performance

- **Async scanning** für responsive UI
- **Smart memory management** für große Dateien
- **Optimierte Release builds** mit LTO
- **Cross-platform** dank Rust

## Troubleshooting

**Problem**: Kompilier-Fehler bei Dependencies  
**Lösung**: `cargo clean && cargo build`

**Problem**: GUI startet nicht auf Linux  
**Lösung**: Installiere X11-Development-Libraries

**Problem**: Langsames Scannen großer Verzeichnisse  
**Lösung**: Nutze mehr Ignore-Patterns (node_modules/, target/, etc.)

## Dependencies

- `egui` - Modern GUI Framework
- `tokio` - Async Runtime
- `walkdir` - Directory Traversal
- `rfd` - Native File Dialogs
- `serde` - Serialization
- `regex` - Pattern Matching
- `directories` - Cross-Platform Paths

## Lizenz

MIT License - Nutze es frei für deine Projekte!
