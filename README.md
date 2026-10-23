# amigo-blinzel

Ein Bildbetrachter für Windows mit einem Ziel: **maximale Geschwindigkeit** beim Öffnen, beim Durchblättern und (bald) bei Thumbnails.

## Bedienung

Die Oberfläche hat keine Leisten. Links oben sitzt das Menü (☰), rechts oben Minimieren, Maximieren und Schließen. Alles andere steckt im Menü oder liegt auf einer Taste.

| Taste / Maus | Aktion |
|---|---|
| → / Leertaste / Bild↓, ← / Rück / Bild↑ | nächstes / vorheriges Bild (gezoomt: Pfeile verschieben) |
| Pos1 / Ende | erstes / letztes Bild |
| 1 / 2 / 3 | Ansicht: Fenster an Bild anpassen (Originalgröße) / Bild an Fenster anpassen / 100 % |
| Mausrad, + / −, 0 | Zoom um den Cursor, Zoom zurücksetzen |
| Links ziehen | Bild passt ins Fenster: Auswahl · Bild größer als das Fenster: verschieben (Umschalt+Ziehen = Auswahl) |
| Mittlere Maustaste | verschieben |
| Strg+C / Strg+X / Strg+A | Auswahl kopieren / ausschneiden (Ansicht wird zugeschnitten) / alles auswählen |
| Strg+S | Ausschnitt speichern unter … (JPEG verlustfrei, wo möglich) |
| Esc | Auswahl aufheben → Original anzeigen → Vollbild verlassen → schließen |
| R / L | drehen (nur Ansicht) |
| F11 / Eingabe / Doppelklick | Vollbild |
| Entf | in den Papierkorb |
| Strg+O, Drag & Drop | öffnen |

Bilddateien werden nie verändert. Kopieren ist sofort fertig: Die Pixel werden erst beim Einfügen decodiert, und zwar nur der ausgewählte Bereich in voller Auflösung.

Ein Ausschnitt lässt sich als neue Datei speichern. Bei JPEG-Quellen werden dabei, wenn möglich, die DCT-Koeffizienten verlustfrei übernommen statt neu kodiert.

**Systemvoraussetzung:** Windows 11 ab Version 24H2, x64 oder ARM64.

Der vollständige Plan mit allen Entscheidungen und Checklisten steht in [docs/PLAN.md](docs/PLAN.md).

## Woher die Geschwindigkeit kommt

- **Decodieren in Bildschirmauflösung.** JPEGs werden per DCT-Skalierung direkt in YCbCr-Ebenen decodiert, Farbumrechnung und Upsampling macht der Shader. Eingebettete Kamera-Vorschaubilder erscheinen zuerst.
- **Prefetch in Blätterrichtung.** Ein Prioritäts-Scheduler mit Abbruch lädt voraus, Ergebnisse liegen als GPU-Texturen bereit.
- **Paralleler Start.**
  - Das Fenster entsteht unsichtbar, ohne DWM-Animation.
  - Datei-Decode und D3D11-Init laufen parallel, die Ordnerliste im Hintergrund.
  - Sichtbar wird das Fenster mit dem ersten Bild.
- **Nativer Stack.** Win32 + Direct3D 11 + vorkompilierte Shader, statische Exe von ca. 2,5 MB, keine Runtime.
- **Benchmark-Gate.** Jede Library ist nur Messlatte; wo der Eigenbau gewinnt, wird er eingebaut (`crates/blinzel-bench`).

## Aufbau

| Crate | Inhalt |
|---|---|
| `blinzel-core` | Decoder (JPEG via libjpeg-turbo, PNG, GIF, eigener BMP), EXIF/MPF, Layout, Auswahl, Scheduler, Caches, CPU-Referenz-Renderer – plattformunabhängig, auf Linux getestet |
| `blinzel-jpeg` | eigener JPEG-Parser, Basis für den parallelen Decoder (nächster Meilenstein) |
| `blinzel-i18n` | Übersetzungen aus `i18n/*.toml`, zur Compile-Zeit in statische Tabellen übersetzt (DE, EN) |
| `blinzel-app` | `blinzel.exe`: Fenster, D3D11-Renderer, Lade-Pipeline, WIC-Fallback (TIFF, HEIC, AVIF, WebP, RAW …) |
| `blinzel-bench` | Testkorpus-Generator und Benchmarks Library vs. Eigenbau |

## Bauen

Windows mit Rust (stable), Visual Studio Build Tools, Windows SDK (für `fxc`), CMake und NASM:

```
cargo build --release -p blinzel-app      # target/release/blinzel.exe
cargo test --workspace
```

Benchmarks:

```
cargo run --release -p blinzel-bench -- corpus testdata/generated
cargo run --release -p blinzel-bench -- run testdata/generated --json bench.json
target/release/blinzel.exe --bench testdata/generated --json bench-app.json
```

Unter Linux laufen alle plattformunabhängigen Tests. `cargo xwin check --target x86_64-pc-windows-msvc` prüft die Windows-App.
