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
✅ **Large File Handling** - 20MB Limit mit Warning-System

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

# CFC - Context File Concatenator: Entwicklungs-Anleitung

Dieses Dokument beschreibt, wie die Entwicklungsumgebung für dieses Projekt eingerichtet und gestartet wird. Die Anwendung besteht aus einem Rust-Backend und einem JavaScript-Frontend (HTML/CSS/JS), das in einer WebView läuft.

## Projektstruktur

Die wichtigsten Teile für die Entwicklung sind:

- `/src`: Enthält den gesamten Rust-Source-Code.
- `/src/main.rs`: Der Haupteinstiegspunkt der Rust-Anwendung.
- `/src/ui`: Enthält alle Frontend-Dateien.
  - `/src/ui/js`: Der Source-Code für das JavaScript-Frontend, aufgeteilt in ES-Module.
  - `/src/ui/dist`: Das Verzeichnis für die gebündelte JavaScript-Datei.
  - `/src/ui/package.json`: Die Konfigurationsdatei für die JavaScript-Entwicklungsumgebung.
- `/build.rs`: (Optional, für Icons) Ein Build-Skript für Rust.
- `/icons`: (Optional, für Icons) Ein Ordner für die Anwendungs-Icons.

## Voraussetzungen

1.  **Rust:** Stelle sicher, dass die [Rust-Toolchain](https://www.rust-lang.org/tools/install) installiert ist.
2.  **Node.js:** Stelle sicher, dass [Node.js](https://nodejs.org/) (welches npm beinhaltet) installiert ist. Dies wird für das Bündeln des JavaScript-Codes benötigt.

## Einmalige Einrichtung

Bevor du mit der Entwicklung beginnst, musst du die JavaScript-Abhängigkeiten installieren.

1.  Öffne ein Terminal.
2.  Navigiere in das UI-Verzeichnis:
    ```bash
    cd src/ui
    ```
3.  Installiere die nötigen Pakete mit npm:
    ```bash
    npm install
    ```
    Dieser Befehl liest die `package.json` und installiert `esbuild` und `concurrently` im `node_modules`-Ordner.

## Entwicklung starten

Um die Anwendung im Entwicklungsmodus zu starten, benötigst du nur noch einen einzigen Befehl.

1.  Stelle sicher, dass du dich im Terminal im Verzeichnis `src/ui` befindest.
2.  Führe den folgenden Befehl aus:
    ```bash
    npm run dev
    ```

### Was passiert im Hintergrund?

Der `npm run dev`-Befehl ist ein Skript, das in der `package.json` definiert ist. Es nutzt das Werkzeug `concurrently`, um zwei Prozesse gleichzeitig zu starten:

1.  **`npm run watch`**:

    - Dieser Prozess startet den `esbuild`-Bundler im "Watch"-Modus.
    - `esbuild` liest die `js/main.js`, folgt allen `import`-Anweisungen und bündelt den gesamten JavaScript-Code in eine einzige Datei: `dist/bundle.js`.
    - Er überwacht kontinuierlich alle `.js`-Dateien. Sobald du eine Änderung speicherst, wird die `dist/bundle.js` automatisch und blitzschnell neu erstellt.

2.  **`cargo run`**:
    - Dieser Prozess kompiliert und startet die Rust-Anwendung.
    - Die `main.rs` liest den Inhalt der `dist/bundle.js` und injiziert ihn zur Laufzeit in die WebView.

Durch diesen Aufbau kannst du einfach deinen JavaScript-Code ändern, und die Änderungen werden nach einem Neuladen der WebView (oft `Ctrl+R` oder `Cmd+R`) sofort sichtbar, ohne dass du die Rust-Anwendung neu starten musst.

## Anwendungs-Icon hinzufügen (macOS)

Damit deine App im Dock und im Finder ein richtiges Logo anzeigt, musst du ein Build-Skript einrichten, das ein macOS-Anwendungspaket (`.app`) mit den nötigen Metadaten erstellt.

### Schritt 1: Icon-Datei erstellen

- macOS verwendet das `.icns`-Format für Icons.
- Erstelle ein quadratisches Bild (z.B. 1024x1024 Pixel) im PNG-Format.
- Benutze einen Online-Konverter oder ein Kommandozeilen-Tool, um deine PNG-Datei in eine `.icns`-Datei umzuwandeln. Ein einfaches Online-Tool ist z.B. [cloudconvert.com](https://cloudconvert.com/png-to-icns).

### Schritt 2: Verzeichnisstruktur anlegen

1.  Erstelle im **Hauptverzeichnis** deines Projekts (auf derselben Ebene wie `src` und `Cargo.toml`) einen neuen Ordner namens `icons`.
2.  Platziere deine konvertierte Icon-Datei in diesem Ordner und nenne sie `icon.icns`.

Deine Struktur sollte nun so aussehen:

```
.
├── Cargo.toml
├── icons/
│   └── icon.icns
└── src/
    ├── main.rs
    └── ui/
        └── ...
```

### Schritt 3: Build-Skript hinzufügen

1.  Erstelle im **Hauptverzeichnis** deines Projekts eine neue Datei namens `build.rs`.
2.  Füge den folgenden Inhalt in `build.rs` ein. Dieses Skript wird während des Build-Prozesses ausgeführt.

    ```rust
    // build.rs
    fn main() {
        // Diese Zeile ist nur für macOS relevant.
        // Sie wird auf anderen Betriebssystemen ignoriert.
        #[cfg(target_os = "macos")]
        {
            use std::env;
            use std::path::PathBuf;

            // Lade das tao-build Skript, das die Info.plist für macOS generiert.
            tao_build::build().unwrap();

            // Zusätzlicher Schritt: Kopiere das Icon in das Ressourcen-Verzeichnis des App-Bundles.
            // Dies ist oft der zuverlässigste Weg.
            let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
            let icon_path = out_dir.join("icon.icns");

            // Kopiere das Icon aus deinem Projektverzeichnis in das Build-Verzeichnis.
            std::fs::copy("icons/icon.icns", icon_path).unwrap();
        }
    }
    ```

### Schritt 4: `Cargo.toml` anpassen

Öffne deine `Cargo.toml`-Datei und füge zwei Dinge hinzu:

1.  Eine `build`-Anweisung, die auf dein neues Skript verweist.
2.  Eine `[build-dependencies]`-Sektion für `tao-build`.

```toml
[package]
name = "dein-app-name"
version = "0.1.0"
edition = "2021"
build = "build.rs" # <-- DIESE ZEILE HINZUFÜGEN

# ... deine [dependencies] Sektion ...

[build-dependencies]
tao-build = "0.2.0" # <-- DIESE SEKTION HINZUFÜGEN
```

### Schritt 5: Ergebnis prüfen

Das Icon wird am zuverlässigsten bei einem **Release-Build** angezeigt, da hier ein komplettes `.app`-Paket erstellt wird.

1.  Führe im Hauptverzeichnis deines Projekts aus:
    ```bash
    cargo build --release
    ```
2.  Nach Abschluss findest du die fertige Anwendung im Verzeichnis `target/release/`. Auf macOS wird sie als `dein-app-name.app` angezeigt.
3.  Wenn du diese `.app`-Datei startest, wird sie mit dem korrekten Icon im Dock erscheinen.

_Hinweis: Während der Entwicklung mit `npm run dev` (`cargo run`) kann es sein, dass macOS weiterhin das generische Terminal-Symbol anzeigt. Das ist normal. Verlasse dich für das Icon auf den Release-Build._

## Produktions-Build erstellen

Wenn du eine finale, optimierte Version der Anwendung erstellen möchtest, gehst du wie folgt vor:

1.  **JavaScript bündeln und minifizieren:**
    Führe im `src/ui`-Verzeichnis aus:

    ```bash
    npm run build
    ```

    Dies erstellt eine optimierte und verkleinerte `dist/bundle.js`.

2.  **Rust-Anwendung kompilieren:**
    Führe im Hauptverzeichnis des Projekts aus:
    ```bash
    cargo build --release
    ```
    Die fertige ausführbare Datei befindet sich dann im `/target/release`-Verzeichnis.
