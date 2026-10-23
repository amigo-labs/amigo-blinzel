# amigo-blinzel: Plan

Dieses Dokument ist die verbindliche Planung. Am Ende wird jeder Punkt gegen die Umsetzung geprüft.

**Status-Legende:**
- ✅ umgesetzt und verifiziert
- 🟡 umgesetzt, Verifikation (CI oder manuell unter Windows) steht aus
- ⬜ offen

Stand: 2026-10-23. Der aktuelle Arbeitsstand für die nächste Session steht in [Abschnitt 16](#16-übergabe-nächste-schritte).

---

## 1. Ziel

IrfanView und ImageGlass sind zu langsam. amigo-blinzel soll beim **Öffnen**, beim **Durchblättern** und bei **Thumbnails** messbar schneller sein als jede Alternative, und das auf einer Website mit nachvollziehbaren Benchmarks belegen.

**Leitlinie: das Maximum rausholen.** Keine Library wird ungeprüft übernommen. Jeder heiße Pfad wird gemessen, und wo ein Eigenbau schneller ist, bauen wir selbst.

## 2. Entscheidungen

| Thema | Entscheidung |
|---|---|
| Sprache | Rust |
| Plattform | **Nur Windows 11 ab 24H2** (Build 26100+), x64 und ARM64. Nur Versionen mit Microsoft-Support. |
| Formate | alle inkl. RAW; native Decoder per Benchmark-Gate, sonst WIC |
| Thumbnails | Filmstreifen + Raster-Browser, beide standardmäßig aus |
| Resident-Modus | optional (Tray, warm gehalten) |
| UI | rahmenlos, keine Leisten; links oben ☰, rechts oben Min/Max/Schließen, alles andere im Menü |
| Ansichtsmodi | Fenster an Bild anpassen (Originalgröße), Bild an Fenster anpassen, 100 % |
| Sprachen | mehrsprachig, Start mit Deutsch + Englisch |
| Auswahl | Rechteck; Strg+C kopiert; Strg+X = kopieren + Ansicht zuschneiden |
| Speichern | Bilddateien werden nie verändert; **Ausschnitt optional speichern** (Strg+S, „Speichern unter“) |
| Maus | kontextabhängig: Bild passt → Links-Ziehen wählt aus; Bild größer → Links-Ziehen verschiebt, Umschalt+Ziehen wählt aus |
| Installer | kein Installer-Framework; die Exe installiert sich selbst |
| Signatur | **SignPath Foundation** (kostenlos für Open Source) |
| Updates | **still im Hintergrund**: höchstens 1× täglich prüfen, signiertes Update laden, verifizieren, beim nächsten Start aktiv |
| Website | **Cloudflare Workers** (statische Assets), mit Wettbewerbsvergleich und Benchmarks |

## 3. Methodik: Benchmark-Gate „Library vs. Eigenbau“

`crates/blinzel-bench` erzeugt einen deterministischen Korpus und misst jeden Kandidaten. Eingebaut wird der Gewinner; die Library bleibt Messlatte und Pixel-Referenz.

| Heißer Pfad | Messlatte | Eigenbau-Hebel | Stand |
|---|---|---|---|
| JPEG-Decode | libjpeg-turbo, zune-jpeg | eigener paralleler Decoder (M3) | ✅ `blinzel-jpeg` für das sichtbare Bild: sequentiell 4,1×, progressive 1,7× turbo (16 Threads), byte-identisch; ein Thread und Regionen bleiben bei turbo |
| Weg zur GPU | RGBA vom Decoder | YCbCr-Planes, Farbkonvertierung im Shader | ✅ gemessen: 24 MP 118 statt 147 ms, 45 MP 190 statt 410 ms |
| PNG | zune-png, png | `png` + eigene fusionierte RGBA-Expansion | ✅ 1,9× (Foto) bzw. 3,2× (Screenshot) schneller als zune-png |
| Speicher | System-Allocator | committed Pufferpool | ✅ 96 MB: 0,6 ms statt 40 ms |
| Thumbnail-Skalierung | fast_image_resize | DCT 1/8 + SIMD-Reducer / GPU | ⬜ M4 |
| Thumbnail-Speicher | SQLite | eigene Pack-Datei | ⬜ M4 |
| Verzeichnis | FindFirstFileExW | NtQueryDirectoryFileEx | ⬜ messen (10k-Ordner im App-Bench) |
| SIMD-Stufen | Basis x86-64-v2 | Laufzeit-Dispatch je Kernel (`kernels.rs`: SSE4.2, AVX, AVX2+FMA) | ✅ gemessen: keine Stufe bringt ≥ 10 % auf einem Hot Path, daher kein Dispatch ([Abschnitt 15](#simd-stufen)) |

**Erkenntnis aus den Messungen:** DCT-Skalierung bringt weniger als erwartet (24 MP: 1/8 = 74 ms, voll = 118 ms). Das Huffman-Decoding dominiert. Der **parallele Huffman-Decoder (M3)** ist deshalb der größte verbleibende Hebel; progressive JPEGs (ca. 320 ms) brauchen ihn besonders.

### Eigener JPEG-Decoder `blinzel-jpeg` (M3)

1. **Parallelität innerhalb eines Bildes:**
   - Restart-Segmente parallel.
   - Spekulatives paralleles Huffman-Decoding über Selbstsynchronisation (Weißenberger & Schmidt), DC-Korrektur per Prefix-Summe.
   - Pipeline aus Entropie-Decode und IDCT.
2. **Weniger Arbeit:**
   - DCT-Skalierung.
   - DC-only-Modus für 1/8 und Thumbnails (AC nur überspringen).
   - YCbCr-Ausgabe.
   - AVX2/NEON-IDCT.
3. **Direkt in den Zielspeicher** schreiben.

Reihenfolge der Stufen:
- sequentielle Baseline
- Restart-parallel
- paralleles Huffman
- DC-only
- progressive

**Entwurf (2026-10-23):**
- **Referenz:** Die Ausgabe ist byte-identisch mit libjpeg-turbo (islow-IDCT, für 1/2, 1/4, 1/8 die reduzierten IDCTs aus `jidctred.c`). Tests vergleichen jede Plane 1:1 mit turbo.
- **Ausgabe:** YCbCr-Planes ohne Upsampling, direkt in vom Aufrufer gestellte Puffer (Pufferpool). Jede Plane ist auf ganze MCUs gepolstert, damit Blöcke ohne Randfälle geschrieben werden.
- **Entropie-Daten:** Ein Vorlauf entfernt das Byte-Stuffing (`FF 00`) und merkt sich die Restart-Marker. Danach ist jede Bitposition eine einfache Zahl, und der Bitleser braucht im heißen Pfad keine Markerprüfung.
- **Huffman:** 10-Bit-Lookup; für AC zusätzlich eine kombinierte Tabelle (Code + Zusatzbits → Lauf, Wert, Länge).
- **Parallel ohne Restart-Marker** (Weißenberger & Schmidt):
  1. Spekulativ: Jeder Thread decodiert seinen Abschnitt ab einer geratenen Position und notiert an jedem MCU-Anfang Bitposition und DC-Summen.
  2. Synchronisieren: Jeder Thread decodiert über sein Ende hinaus, bis er auf einen notierten Zustand des Nachfolgers trifft. Ab dort sind beide identisch.
  3. Präfixsummen ergeben die absolute MCU-Nummer und die DC-Prädiktoren an jedem notierten MCU-Anfang.
  4. Endgültig: Die Threads decodieren MCU-Zeilenbänder ab dem bekannten Zustand und schreiben direkt in ihr eigenes Band der Planes (sicheres Rust, `split_at_mut`).
- **Mit Restart-Markern** ist Schritt 1 exakt, und Schritt 2 entfällt.
- **Rückfall:** Arithmetische Kodierung, 12 Bit, verlustfrei und CMYK bleiben bei libjpeg-turbo.

## 4. Architektur

| Crate | Inhalt | Stand |
|---|---|---|
| `blinzel-core` | Pixelpuffer, Pufferpool, Formaterkennung, Decoder (JPEG/turbojpeg, PNG, GIF, eigener BMP), EXIF/MPF, Layout, Auswahl, natsort, Dateiliste, Scheduler, LRU, CPU-Referenz-Renderer, Timing, Settings, Export | 🟡 (auf Linux getestet) |
| `blinzel-jpeg` | strenger Marker-Parser; paralleler Decoder (Huffman, 8 Bit, sequentiell und progressive) in YCbCr-Planes, byte-identisch mit libjpeg-turbo | 🟡 |
| `blinzel-i18n` | `i18n/*.toml` → statische Tabellen (DE/EN) | 🟡 |
| `blinzel-bench` | Korpus + Library-vs-Eigenbau-Benches (JSON) | 🟡 |
| `blinzel-app` | `blinzel.exe`: Fenster, Chrome, Menü, D3D11-Renderer, Loader, WIC, Zwischenablage, Dialoge, `--bench` | 🟡 |
| `blinzel-race` | Wettbewerbs-Benchmark (M8): Testbilder mit Farbsignatur, Desktop Duplication auf allen Monitoren, Öffnen/Weiter/Dauerfeuer | 🟡 |
| `website/` + `blinzel-site` | Website für Cloudflare Workers (M8): statisch, DE/EN, Diagramme als Inline-SVG aus `website/data/race.json` | 🟡 |

## 5. Kernprinzipien Start und Blättern

- 🟡 **Kaltstart parallel:**
  - Der Einzelinstanz-Handoff (`WM_COPYDATA` + `AllowSetForegroundWindow`) kommt zuerst.
  - Dann wird der Decode-Job eingereiht, und `d3d11.dll` wird per `LoadLibrary` auf einem Worker geladen.
  - Das Fenster entsteht gecloakt, mit `DWMWA_TRANSITIONS_FORCEDISABLED` und `WS_EX_NOREDIRECTIONBITMAP`.
  - Die Ordnerliste entsteht im Hintergrund.
  - Das Fenster wird mit dem ersten Bild entcloakt, spätestens nach 400 ms.
  - *Abweichung vom ursprünglichen Plan:* Statt den Header vor `CreateWindow` zu lesen, wird das noch unsichtbare Fenster vor dem Entcloaken auf Bildgröße gebracht. Das Ergebnis ist gleich, es blockiert nur nicht.
- 🟡 **Shader:** vorkompiliert mit fxc. COM, WIC, Direct2D und DirectWrite starten erst bei Bedarf; DirectWrite wird nach dem ersten Present vorgewärmt.
- 🟡 **Eingebettete Kamera-Vorschau** zuerst, wenn sie mindestens halb so groß wie der Bildschirm ist.
- 🟡 **Prefetch-Scheduler:**
  - Zwei Stufen (I/O, CPU), Prioritäten, Generation-Cancel.
  - Richtungsabhängig: 4 voraus, 2 zurück.
  - Jede Datei wird nur einmal gelesen; ein Job liefert Vorschau und Stufe.
- 🟡 **GPU:**
  - Kachel-Texturen (4096, mit Rand), Upload direkt von Workern.
  - VRAM-Budget über `QueryVideoMemoryInfo`.
  - EXIF-Orientierung als Transform, nie Pixel drehen.
- 🟡 **I/O:**
  - `ReadFile` mit Pufferpool, kein mmap.
  - Cloud-Platzhalter werden beim Prefetch übersprungen.
  - HDD-Erkennung (Seek-Penalty) → 1 I/O-Thread.
- 🟡 **Hybrid-CPUs:** Worker nur auf P-Kernen (EfficiencyClass), ABOVE_NORMAL, ohne Power-Throttling.
- 🟡 **Robust:** kein `panic=abort`, jeder Decode läuft in `catch_unwind`.
- ⬜ **Delay-Load** für shell32/ole32: erst Startzeit messen (`--exit-after-first-frame`), dann entscheiden.

## 6. UI und Fensterverhalten

- 🟡 **Rahmenloses Fenster:**
  - `WM_NCCALCSIZE` entfernt die Caption; Ränder und Schatten bleiben.
  - Oberer Resize-Rand und Drag-Streifen per `WM_NCHITTEST`.
  - Snap-Layouts über `HTMAXBUTTON`.
- 🟡 **Bedienelemente:** ☰ + Min/Max/Schließen als SDF-Formen über dem Bild, mit Halo und Hover-Flächen (Schließen in Rot).
- 🟡 **Hamburger-Menü:** natives `TrackPopupMenuEx` im Dark-Mode (uxtheme 135/133/136), lokalisiert.
- 🟡 **Ansichtsmodi** (1/2/3), Zoom um den Cursor (Rad, +/−, 0), Pan (Maus, Pfeile bei Überlauf), Vollbild (F11, Eingabe, Doppelklick), Drehen nur in der Ansicht (R/L).
- 🟡 Modus 1 („Originalgröße“): Das Fenster folgt jedem Bild, begrenzt auf den Arbeitsbereich und um die Mitte zentriert.
- 🟡 Leerer Start zeigt einen Hinweis; Drag & Drop, Strg+O, Entf (Papierkorb), Maus-Zurück/Vor.
- ⬜ Filmstreifen und Raster (M4).
- ⬜ Fensterposition und -größe merken.

## 7. Auswahl, Kopieren, Ausschneiden, Speichern

Grundsatz: Bilddateien werden nie überschrieben. Gespeichert wird nur über „Speichern unter“, mit der Überschreib-Rückfrage von Windows.

- 🟡 **Auswahlrechteck** in Vollauflösungs-Koordinaten:
  - Anfasser, Verschieben, Größenanzeige `B × H` per DirectWrite.
  - Abgedunkelte Umgebung.
  - Strg+A, Esc.
- 🟡 **Strg+C** mit verzögertem Clipboard-Rendering (`CF_DIBV5` + `PNG`): Nur die Region wird decodiert, in voller Auflösung und erst beim Einfügen.
- 🟡 **Strg+X:** kopieren und die Ansicht auf die Auswahl zuschneiden (reiner Ansichtszustand). Esc bzw. „Original anzeigen“ stellt das Bild wieder her.
- 🟡 **Strg+S „Ausschnitt speichern unter…“** (Auswahl, sonst Zuschnitt):
  - **JPEG verlustfrei**, wenn die Quelle ein JPEG ist, der Ausschnitt an MCU-Grenzen beginnt und keine Ansichtsdrehung aktiv ist.
    - Umsetzung über `tj3Transform`-Crop, ohne Neukodierung, sehr schnell.
    - Die EXIF-Daten bleiben erhalten, auch die Orientierung.
    - Das veraltete EXIF-Vorschaubild wird abgehängt, das MPF-Segment entfernt.
  - **Sonst:**
    - Region in voller Auflösung decodieren und orientieren.
    - Als JPEG (Qualität 95, Subsampling der Quelle, ICC-Profil übernommen) oder als PNG speichern.
  - Läuft im Hintergrund-Thread; Fehler erscheinen als Meldung.

## 8. Mehrsprachigkeit

- 🟡 `i18n/<sprache>.toml`; `build.rs` erzeugt `enum Msg` und statische Tabellen. Zur Laufzeit ist eine Übersetzung nur ein Array-Index.
- 🟡 Fehlende Keys fallen auf Englisch zurück (Build-Warnung). Platzhalterfehler brechen den Build ab.
- 🟡 Sprachwahl über `GetUserPreferredUILanguages`; im Menü überschreibbar, ohne Neustart.
- 🟡 Plural-Regeln (CLDR-Familien).
- ⬜ RTL-Sprachen.

## 9. Plattform: Windows 11 ab 24H2

Nur Versionen mit Microsoft-Support. Stand Oktober 2026:

| Version | Support |
|---|---|
| Windows 11 25H2 | Home/Pro bis Okt. 2027, Enterprise bis Okt. 2028 |
| Windows 11 24H2 | Enterprise/Education bis Okt. 2027 |

Windows 10 wird nicht unterstützt: Für Home/Pro ist der Support beendet, nur Commercial ESU und LTSC laufen noch.

**Konsequenzen:**
- 🟡 Minimum Direct3D Feature Level 11_0 und Shader Model 5. Jedes Windows-11-Gerät hat eine DX12/WDDM-2-GPU. Keine FL-10-Fallbacks.
- 🟡 Versionsprüfung: Build < 26100 → lokalisierte Meldung beim Start bzw. im Installer.
- 🟡 Anwendungsmanifest: supportedOS, PerMonitorV2, longPathAware, UTF-8-Codepage.
- 🟡 ARM64-Build im CI (cross von x64); Tests laufen auf x64.
- Ohne Fallbacks nutzbar: EcoQoS, CPU-Sets, Snap-Layouts, Dark-Mode-Attribute, WIC-Codecs für HEIF, AVIF und WebP (soweit die Store-Erweiterungen installiert sind).

## 10. Installation, Updates, Signierung (M7)

### Setup-Exe

- 🟡 **Release-Assets** pro Release auf GitHub:
  - `amigo-blinzel-setup-x64.exe` und `amigo-blinzel-setup-arm64.exe`, das ist dieselbe Exe wie der Viewer.
  - `SHA256SUMS` und `SHA256SUMS.sig`.
- 🟡 **Installationsdialog:** Als Setup gestartet (Dateiname enthält `setup` oder `--install`), erscheint ein Dialog in Viewer-Optik, rahmenlos und mehrsprachig.
- 🟡 **Installation pro Benutzer, ohne Admin**, nach `%LOCALAPPDATA%\Programs\amigo-blinzel\`:
  - Startmenü-Verknüpfung (`IShellLinkW`, AppUserModelID).
  - Eintrag unter `HKCU\…\Uninstall`.
  - ProgIDs, Capabilities und RegisteredApplications anlegen.
  - Danach die Standard-Apps-Seite öffnen (programmatisch setzen erlaubt Windows nicht).
  - `SHChangeNotify`.
- 🟡 **Deinstallation:** über die Systemeinstellungen. Die Exe kopiert sich nach `%TEMP%` und räumt von dort auf.
- 🟡 **Silent-Modus:** `--install --silent` und `--uninstall --silent` (für winget).
- 🟡 **Versionsprüfung im Setup:** Build < 26100 → abbrechen mit Meldung.
- 🟡 **winget-Manifest**; ein PR an winget-pkgs nach jedem Release, optional automatisiert.

### Signierung über SignPath Foundation

Voraussetzungen, die bei dir liegen:
1. ⬜ OSI-Lizenz wählen (z. B. MIT oder Apache-2.0) und als `LICENSE` ins Repo legen. Das Repo ist bereits öffentlich ✅.
2. ⬜ Antrag bei der SignPath Foundation stellen (Projekt, Code-Signing-Policy, Datenschutz-Seite auf der Website).
3. ⬜ In SignPath Projekt, Artefakt-Konfiguration und Signing-Policy anlegen; Secret `SIGNPATH_API_TOKEN` und die Org-ID ins Repo.

Umsetzung:
- 🟡 Release-Workflow bei Tag `v*` auf GitHub-gehosteten Runnern (SignPath prüft die Build-Herkunft):
  1. x64 und ARM64 bauen.
  2. Signieren über `signpath/github-action-submit-signing-request` (mit manueller Freigabe in SignPath).
  3. `SHA256SUMS` erzeugen und mit Ed25519 signieren.
  4. Release als Entwurf anlegen und veröffentlichen.
- 🟡 **Eigene Signatur der Prüfsummen (Ed25519).** Den Public Key kennt die App fest. Grund: Das SignPath-Foundation-Zertifikat signiert viele Open-Source-Projekte. Authenticode allein beweist also nicht, dass es *unsere* Exe ist.

**Umsetzung (Branch `m7-install`):**
- `blinzel-app/src/setup/`: Moduserkennung, Installation über ein `Layout` (echte Pfade oder Sandbox für Tests), Registry-Hilfe nur für HKCU, Startmenü-Verknüpfung mit AppUserModelID `amigo-labs.amigo-blinzel`, Setup-Fenster mit Renderer und DirectWrite des Viewers (ohne Direct3D: Meldungsfenster).
- Exit-Codes des Setups: 0 installiert, 1 fehlgeschlagen, 2 Windows zu alt, 3 abgebrochen.
- `packaging/winget/`: Vorlagen der drei Manifeste (Scope user, Silent-Schalter, ProductCode = Uninstall-Schlüssel).
- `.github/workflows/release.yml`: baut x64 und ARM64, signiert mit SignPath nur, wenn die Variable `SIGNPATH_ORGANIZATION_ID` gesetzt ist, erzeugt und signiert `SHA256SUMS`, legt einen Release-Entwurf an.
- Test `blinzel-app/tests/setup.rs`: Installation, Update an Ort und Stelle und Deinstallation in einer Sandbox (Temp-Ordner, eigener HKCU-Zweig).

### Self-Updater (still im Hintergrund)

- 🟡 **Prüfen:**
  - Höchstens 1× pro Tag, frühestens 10 s nach dem Start, auf einem Hintergrund-Thread (nie auf dem kritischen Pfad).
  - Quelle: `GET https://api.github.com/repos/amigo-labs/amigo-blinzel/releases/latest` über WinHTTP, das per `LoadLibrary` geladen wird und den Proxy des Systems nutzt.
- 🟡 **Laden** des passenden Assets (x64/ARM64) plus `SHA256SUMS(.sig)` nach `%LOCALAPPDATA%\amigo-blinzel\updates\`.
- 🟡 **Prüfen in drei Schritten:**
  1. Ed25519-Signatur der Prüfsummen.
  2. SHA-256 der Exe.
  3. Authenticode über `WinVerifyTrust`.

  Erst dann wird getauscht.
- 🟡 **Tauschen:**
  - Die laufende Exe in `.old` umbenennen (unter NTFS erlaubt) und die neue an ihren Platz legen.
  - Der nächste Start nutzt die neue Version ohne Zusatzkosten.
  - `.old` wird beim nächsten Start gelöscht.
- 🟡 **Rollback:** Startet die neue Version dreimal nicht bis zum ersten Frame, wird `.old` zurückgeholt.
- 🟡 **Menü:** „Nach Updates suchen“ und „Über amigo-blinzel (Version)“. Einstellung `auto_update = true`.

**Umsetzung:**
- Entscheidungen ohne Windows in `blinzel-core::update` (Version, Release-JSON über einen kleinen eigenen JSON-Leser, `SHA256SUMS`, Ed25519 über `ed25519-compact`, Zeitplan, Startzähler) mit Unit-Tests.
- Windows-Teil in `blinzel-app/src/update.rs`: WinHTTP, `BCryptHash` und `WinVerifyTrust` werden erst bei Bedarf geladen. Nur eine installierte Kopie aktualisiert sich; Entwickler- und portable Builds nie.
- `blinzel-release keygen | sign | verify` für den eigenen Signaturschlüssel.
- **Solange der öffentliche Schlüssel fehlt** (`RELEASE_KEY = None` in `update.rs`), prüft und installiert der Updater nichts. Erzeugen: `cargo run -p blinzel-release -- keygen <datei>`, den Inhalt der Datei als Secret `RELEASE_SIGNING_KEY` hinterlegen, die Datei löschen, die ausgegebene Zeile in `update.rs` eintragen.
- Bis SignPath eingerichtet ist, sind Releases nicht Authenticode-signiert, und der Updater lehnt sie ab (Schritt 3). Das ist Absicht.

## 11. Website und Wettbewerbsvergleich (M8)

### Website auf Cloudflare Workers

- 🟡 `website/`: statisches HTML/CSS mit minimalem JS, extrem schnell (passend zum Produkt), Deutsch und Englisch.
  - Seiten: Start, Funktionen, **Vergleich**, **Benchmarks**, Methodik, Download, Changelog, Datenschutz, Code-Signing-Policy (für SignPath).
  - Charts werden beim Build als Inline-SVG aus den Benchmark-JSONs erzeugt; keine Chart-Library im Browser.
  - Der Download-Link zeigt auf `releases/latest/download/amigo-blinzel-setup-x64.exe` (stabile URL).
- 🟡 **Deployment** über Cloudflare Workers Static Assets (`wrangler.toml`), per GitHub Action (`cloudflare/wrangler-action`) bei Änderungen an `website/` und bei jedem Release.
  - Secrets: `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` (bei dir).
  - Zunächst die `workers.dev`-Subdomain; eine eigene Domain kann jederzeit dazukommen.

### Wettbewerbs-Benchmark `blinzel-race`

Ein eigenes Messwerkzeug für Windows misst alle Viewer gleich, von außen.

**Umsetzung (`crates/blinzel-race`, Branch `m8-race`):**
- `corpus <dir>`: 24 JPEGs mit 24 MP und je ca. 8 MB (Sensorrauschen, damit der Entropie-Decoder so viel arbeitet wie bei echten Fotos), jedes um eine von sechs Signaturfarben (die farbigen Ecken des RGB-Würfels). Aufeinanderfolgende Bilder unterscheiden sich immer.
- Erkennung: Desktop Duplication auf jedem Monitor (Viewer öffnen dort, wo sie zuletzt waren); jeder präsentierte Frame liefert `LastPresentTime` (QPC) und ein Raster aus 40 × 24 Abtastpunkten.
- Tasten gehen nur raus, solange der Viewer das Vordergrundfenster besitzt.
- `list` zeigt die erkannten Viewer mit Version (Versionsressource; `blinzel.exe` hat jetzt eine).
- `run <dir> [--reps N] [--cold] [--only …] [--viewer name=exe] [--json …]`. `--cold` leert die Standby-Liste und braucht Administratorrechte.

**Erste Probeläufe** (Laptop, Ryzen 7 PRO 6850H, Radeon 680M, 3 Monitore; nur Funktionsnachweis, keine Referenzzahlen):

| Viewer | Öffnen warm | Weiter | Dauerfeuer 6 Tasten |
|---|---|---|---|
| amigo-blinzel 0.1.0 | 270–295 ms | 9–14 ms | alle gezeigt |
| ImageGlass 8.8 | 1,8 s | 0,8 s | alle, langsam |
| Windows Photos 2026.11 | 1,2 s | 45 ms | 3 gezeigt, Tasten verschluckt |

- 🟡 **Kandidaten:** IrfanView 64, ImageGlass, XnView MP, FastStone Image Viewer, Windows Fotos; optional JPEGView und qimgv. Jeweils mit Standardeinstellungen; Versionen werden protokolliert.
- 🟡 **Messgrößen:**
  1. **Öffnen:** Zeit vom Prozessstart bis das Bild sichtbar ist, warm und kalt (Standby-Liste geleert).
     - Erkannt über die Desktop Duplication API (Frame-Zeitstempel `LastPresentTime`).
     - Die Testbilder tragen eine eindeutige Farbsignatur.
  2. **Weiterblättern:** `SendInput` (Pfeil rechts), Zeit bis die Farbsignatur wechselt; einzeln nach Leerlauf (Prefetch-Fall) und als Dauerfeuer mit 30 Hz (wie viele Bilder werden wirklich gezeigt).
  3. **Thumbnails** (ab M4): Zeit bis das Raster eines 1000er-Ordners gefüllt ist, für Viewer mit Raster-Modus.
- 🟡 **Fairness und Recht:**
  - Gleiche Maschine, gleiche Dateien, 10 Wiederholungen, Median und p95.
  - Rohdaten, Versionen und Methodik werden veröffentlicht.
  - Damit sachlich, nachprüfbar und relevant, wie es vergleichende Werbung (UWG §6) verlangt.
- ⬜ **Zahlen:** Referenzwerte von einer definierten Referenzmaschine (Hardware wird angegeben). CI-Werte nur als Trend.
- ⬜ **Vergleichstabelle der Funktionen:** Formate, Startzeit, Speicherbedarf, Größe der Installation, Lizenz, Werbung/Telemetrie, ARM64-nativ.

## 12. Meilensteine

| | Inhalt | Ziel | Status |
|---|---|---|---|
| **M0** | Workspace, CI (Linux + Windows), Korpus, Bench-Harness | Baseline-JSON im CI | 🟡 |
| **M1** | Einzelbild: Startsequenz, Chrome, Menü, Ansichtsmodi, i18n, D3D11, JPEG/PNG/BMP/GIF, WIC-Fallback, EXIF/MPF | t0 → Present ≤ 50 ms warm, ≤ 120 ms kalt; kein weißer Frame | 🟡 |
| **M2** | Blättern, Prefetch, Budgets, Full-Res bei Zoom, Ordner-Watch, P/E-Pools, Auswahl, Strg+C/X/S | Next ≤ 8 ms p99 vorgeladen; 10k-Ordner ≤ 25 ms; Strg+C < 1 ms | 🟡 |
| **M3** | `blinzel-jpeg`: paralleler Decoder | Full-Res 24 MP ≥ 3× turbo (8 Kerne); DC-only ≥ 2× | 🟡 Ziele erreicht (4,1× bzw. 3,6×), progressive 1,7×; CI-Bestätigung steht aus |
| **M4** | Thumbnails: Fast-Paths, Pack-Cache, Atlas, Filmstreifen + Raster | 200 Kacheln aus dem Cache ≤ 30 ms; kalt ≥ 150/s | ⬜ |
| **M5** | WebP (animiert), GIF-Animation, AVIF, HEIC, JXL, TIFF, RAW nativ (jeweils Bench-Gate) | nativ schneller als WIC, sonst bleibt WIC | ⬜ |
| **M6** | Tray/Resident, Einstellungen, ICC-Farbmanagement | Resident-Handoff → sichtbar ≤ 25 ms | ⬜ |
| **M7** | Setup-Exe, SignPath-Signatur, Release-Workflow, Self-Updater, winget | signiert, keine SmartScreen-Warnung, Update ohne Benutzeraktion | 🟡 umgesetzt; offen: Lizenz, SignPath, Release-Schlüssel, erster Release |
| **M8** | `blinzel-race` + Website auf Cloudflare Workers | öffentliche, reproduzierbare Vergleichszahlen | 🟡 Werkzeug und Website fertig; offen: Referenzmessung, Funktionstabelle mit Quellen, Cloudflare-Secrets |

**Reihenfolge:**
1. PR #1: M0–M2 inklusive Windows-11-Baseline und „Ausschnitt speichern“.
2. M3.
3. M7, damit Releases früh signiert verteilt werden können.
4. M8 (Website mit ersten Vergleichszahlen für Öffnen und Blättern).
5. M4, M5, M6. Die Website-Zahlen werden mit jedem Meilenstein ergänzt.

## 13. Verifikation

- **Linux (Container/CI):**
  - `cargo test --workspace`: core, jpeg, i18n, Scheduler, Layout, Auswahl, Export.
  - `cargo clippy -D warnings`, `cargo fmt --check`.
  - `cargo xwin clippy --target x86_64-pc-windows-msvc`.
- **Windows-CI:**
  - Build, Tests, WARP-Golden-Tests (Shader gegen CPU-Referenz: Orientierungen, Kachelnähte, Zoom, Zuschnitt, Grau, Alpha, Text).
  - `blinzel-bench`, `blinzel.exe --bench`, Startup-Smoke-Test.
  - `blinzel-bench` zusätzlich als x86-64-v3-Build; `blinzel-bench compare` schreibt beide Läufe nebeneinander in die Step-Summary.
  - ARM64-Build.
- **Lokal unter Windows 11** (Entwicklerrechner):
  - Der rustup-Host ist dort `windows-gnu`; gebaut wird mit `cargo +1.99.1-x86_64-pc-windows-msvc …`.
  - NASM liegt unter `%LOCALAPPDATA%\bin\NASM`, CMake kommt aus Visual Studio (`Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin`); beides muss im `PATH` stehen.
  - Laptop-Messungen schwanken zwischen aufeinanderfolgenden Läufen um bis zu 30 % (Drosselung). Builds deshalb abwechselnd messen und libjpeg-turbo als Kontrollgröße mitlaufen lassen.
- **Manuell unter Windows 11:**
  - Doppelklick-Start und Blättern (auch Dauerfeuer).
  - Ansichtsmodi, Snap-Layouts, Menü per Maus und Tastatur, Sprachwechsel.
  - Auswahl → Einfügen in Paint/Word/GIMP; Strg+X/Esc; Strg+S mit beiden Pfaden (verlustfrei und neu kodiert).
  - Papierkorb, Drag & Drop, Einzelinstanz.
- **Release (M7):** Signatur prüfen (Eigenschaften → Digitale Signaturen); Update-Pfad von v0.x.y nach v0.x.y+1 auf einer sauberen VM.

## 14. Offene Punkte, die bei dir liegen

- ⬜ Lizenz wählen (Voraussetzung für die SignPath Foundation).
- ⬜ SignPath-Foundation-Antrag sowie `SIGNPATH_API_TOKEN` (Secret) und die Org-ID (Variable `SIGNPATH_ORGANIZATION_ID`).
- ⬜ Release-Signaturschlüssel: `cargo run -p blinzel-release -- keygen <datei>`, Inhalt als Secret `RELEASE_SIGNING_KEY`, die ausgegebene `RELEASE_KEY`-Zeile in `crates/blinzel-app/src/update.rs`.
- ⬜ Cloudflare-Konto: `CLOUDFLARE_API_TOKEN` (Workers bearbeiten) und `CLOUDFLARE_ACCOUNT_ID` als Repo-Secrets; optional eine Domain.
- ⬜ Referenzmaschine für die Website-Benchmarks festlegen, dort `blinzel-race run … --cold --json website/data/race.json` (als Administrator) laufen lassen und die JSON committen.
- ⬜ Impressum: Angaben zum Betreiber der Website (Name, Anschrift, Kontakt), falls die Seite geschäftsmäßig betrieben wird. Die Website hat dafür noch keine Seite; die Angaben kann nur der Betreiber liefern.
- ⬜ Erster manueller Test unter Windows 11 (siehe Verifikation).

## 15. Messwerte (Stand)

Linux-Container, 4 vCPU, ein Kern je Decode, Bench im Schnellmodus:

| Fall | Ergebnis |
|---|---|
| JPEG 24 MP, voll | turbojpeg YCbCr 118 ms; BGRA 147 ms; zune-jpeg 202 ms |
| JPEG 24 MP, Bildschirm (1440p) | 92 ms |
| JPEG 24 MP, 1/8 | 74 ms |
| JPEG 24 MP, progressiv | ca. 320 ms, unabhängig von der Skalierung |
| JPEG 45 MP | YCbCr 190 ms; BGRA 410 ms |
| PNG 12 MP RGB | eigener Pfad 117 ms; png 132 ms; zune-png 216 ms |
| PNG 8 MP Screenshot | eigener Pfad 8,4 ms; beide Libraries mit RGBA-Expansion 24–27 ms |
| natürliche Sortierung, 10k Namen | 2,5 ms |
| Puffer 96 MB | Pool 0,6 ms; neu 40 ms |
| `blinzel.exe` (x64, Release, statische CRT) | 1,49 MB (Cross-Build mit clang-cl/lld); 2,5 MB mit MSVC im CI |

Windows-CI (`windows-2025`, ohne GPU, Artefakt `windows-bench-and-exe`), vor und nach dem libjpeg-turbo-Fix (PR #2):

| Fall | vorher | nachher |
|---|---|---|
| JPEG 24 MP 4:2:0, turbojpeg YCbCr voll | 228 ms | 93 ms |
| JPEG 24 MP 4:2:0, 1/8 | 195 ms | 70 ms |
| JPEG 24 MP 4:4:4 / Graustufen, voll | 284 / 217 ms | 118 / 86 ms |
| JPEG 45 MP, voll | 423 ms | 175 ms |
| JPEG 24 MP progressiv, voll | 658 ms | ca. 300 ms |
| zune-jpeg 24 MP (Vergleich) | 160 ms | 129 ms; turbojpeg jetzt 1,2–1,4× schneller |
| App: 24 MP öffnen (Decode auf 2250 × 1500) | 240 ms | 87 ms (Upload 1 ms) |
| Blättern mit 30 Tasten/s, p99 | 604 ms | 240 ms (19 von 22 aus dem Prefetch, Median 0 ms) |
| Ordner mit 10k Dateien | 9,9 ms | 8,8 ms |
| Start bis erstes Present | 617 ms | 150 ms, davon 134 ms Decode |

Der Runner hat keine GPU: Die Startmessung meldet `"adapter": "Microsoft Basic Render Driver"`, also Software-Rendering durch Windows. Echte GPU-Startzeiten müssen auf einem Rechner mit Grafikkarte gemessen werden (`blinzel.exe bild.jpg --exit-after-first-frame --json start.json`).

Der C-Teil von libjpeg-turbo war dabei unoptimiert: cmake-rs entfernt unter MSVC mit Visual-Studio-Generator alle `/O`-Flags, MSBuild kompiliert dann mit `/Od`. Behoben durch eine gepatchte Kopie von cmake-rs (`third_party/cmake-rs`, per `[patch.crates-io]`), die CMakes Standard-Flags je Konfiguration behält. libjpeg-turbo baut außerdem als „Release“ (`/O2 /Ob2`). `blinzel-bench run --check` schlägt fehl, falls zune-jpeg den ausgelieferten JPEG-Pfad je wieder schlägt. Die Startmessung schreibt den Adapternamen mit, damit Software-Rendering erkennbar ist.

### SIMD-Stufen

Lokal gemessen auf AMD Ryzen 7 PRO 6850H (8 Kerne, AVX2, kein AVX-512), Windows 11, Netzbetrieb, Energieschema „Ausbalanciert“; Mediane, ein Kern je Decode.

**libjpeg-turbo** auf `jpeg-24mp-420.jpg`, jede Stufe als eigener Kindprozess (`JSIMD_FORCE*`):

| Fall | ohne SIMD | SSE2 | AVX2 | AVX2 gegenüber SSE2 |
|---|---|---|---|---|
| YCbCr voll | 120 ms | 81 ms | 76 ms | 5 % |
| BGRA voll (inkl. Farbumrechnung, Upsampling) | 211 ms | 99 ms | 91 ms | 9 % |
| 1/8 | 59 ms | 55 ms | 54 ms | ≈ 0 |

AVX2 wählt libjpeg-turbo ohnehin selbst. Der Rest ist der skalare Huffman-Decoder, also der Hebel für M3.

**Eigene Kernels** (`blinzel_core::kernels`) auf 12 MP, zwei Läufe:

| Kernel | Baseline (v2) | AVX2+FMA |
|---|---|---|
| RGB → RGBA | 6,2 / 5,5 ms | 4,6 / 5,1 ms |
| Grau+Alpha → RGBA | 5,0 / 4,7 ms | 4,4 / 4,6 ms |
| RGBA → BGRA | 5,5 / 5,5 ms | 5,5 / 4,8 ms |
| YCbCr 4:2:0 → BGRA (CPU-Referenz, nicht der Anzeigepfad) | 583 / 530 ms | 500 / 557 ms |

Die gepackten Kernels sind speichergebunden. Gewinnt AVX2 dort 0,5–1,5 ms, sind das bei einem 12-MP-PNG (≈ 90 ms) höchstens 2 %.

**Obergrenze „alles mit AVX2“** (x86-64-v3-Build, damit auch die Crates `png` und `fdeflate`): Drei abwechselnde Läufe ergeben für PNG 2–5 % (Minima); JPEG bleibt gleich, weil der C-Teil von `RUSTFLAGS` unberührt bleibt. Ein einzelner Vergleich hintereinander hatte 24 % gezeigt, der Baseline-Lauf war aber gedrosselt.

**Ergebnis nach dem Kriterium (≥ 10 % auf einem Hot Path):** Keine Stufe erreicht es. Deshalb gibt es keinen Laufzeit-Dispatch; die Kernels laufen mit `Level::Baseline`, die Basis bleibt x86-64-v2. Kernel-Modul und Bench-Gruppe `simd` bleiben als Werkzeug für M3 (AVX2-IDCT, Farbumrechnung im eigenen Decoder).

**Windows-CI** (PR #4, `windows-2025`, AMD EPYC 7763, AVX2; die Läufe sind ruhig, das Minimum liegt beim Median):

| Messung | Ergebnis |
|---|---|
| libjpeg-turbo YCbCr voll: ohne SIMD / SSE2 / AVX2 | 159 / 100 / 95 ms |
| libjpeg-turbo BGRA voll: ohne SIMD / SSE2 / AVX2 | 253 / 122 / 111 ms |
| libjpeg-turbo 1/8: ohne SIMD / SSE2 / AVX2 | jeweils 72 ms |
| Kernel RGB → RGBA: Baseline / AVX2 | 4,4 / 3,4 ms (1,28×, ≈ 1 % des 12-MP-PNG-Decodes von 113 ms) |
| Kernel Grau+Alpha → RGBA, RGBA → BGRA: Baseline / AVX2 | 3,0 / 2,6 ms; 3,8 / 3,5 ms |
| Kernel YCbCr → BGRA, alle Stufen | 604–636 ms, kein Unterschied |
| x86-64-v3 gegenüber v2: PNG (RGB, RGBA, Screenshot) | 1,02× / 1,02× / 1,06× |
| x86-64-v3 gegenüber v2: JPEG (C-Code unverändert) | 1,01–1,03× |

Das bestätigt die lokale Messung. Bei 1/8 bringt SIMD libjpeg-turbo auf dem Runner gar nichts mehr.

## 16. Übergabe: nächste Schritte

**Stand (2026-10-23):**
- PR #1–#3 sind gemergt.
- PR #4 (`simd-levels`): SIMD-Stufen gemessen, kein Dispatch ([SIMD-Stufen](#simd-stufen)). Wartet auf Merge.
- PR #5 (`m3-jpeg`, auf #4 gestapelt): eigener JPEG-Decoder.
  - Sequentiell (Baseline und Extended, 8 Bit, jede Unterabtastung, Restart-Marker, mehrere Scans), Skalen 8/8, 4/8, 2/8, 1/8.
  - Parallel: exakt mit Restart-Markern, sonst spekulativ mit Selbstsynchronisation. Ein Rateversuch, der auf einen ungültigen Code läuft, startet ein Bit später neu.
  - Byte-identisch mit libjpeg-turbo; geprüft in `blinzel-core/tests/own_jpeg.rs` und mit `blinzel-bench run --check` über den ganzen Korpus.
  - Eingebunden über `decode::decode_with(data, req, threads)`: Das sichtbare Bild läuft auf allen P-Kernen über `blinzel-jpeg`, Prefetch, Regionen und Fehlerfälle bei libjpeg-turbo.
  - `blinzel-bench jpeg-ab <datei> [skala] [threads]` misst turbo und `blinzel-jpeg` abwechselnd im selben Prozess.

**Messwerte M3** (Ryzen 7 PRO 6850H, 8 Kerne / 16 Threads, A/B abwechselnd, Mediane):

| Datei | turbo | blinzel-jpeg, 1 Thread | 16 Threads | Faktor |
|---|---|---|---|---|
| 24 MP 4:2:0 voll | 77 ms | 90 ms | 20 ms | 3,9× |
| 24 MP 4:2:0 1/2 | 66 ms | 78 ms | 19 ms | 3,5× |
| 24 MP 4:2:0 1/8 | 57 ms | 57 ms | 16 ms | 3,6× |
| 24 MP 4:4:4 voll | 102 ms | 109 ms | 26 ms | 3,9× |
| 45 MP 4:2:0 voll | 151 ms | 206 ms | 38 ms | 4,0× |
| 12 MP 4:2:0 voll | 37 ms | 45 ms | 12 ms | 3,1× |

App-Benchmark mit echter GPU (Radeon 680M): 24-MP-JPEG öffnen 28,7 ms gesamt (Decode 19,9 ms auf 3000 × 2000).

**Progressive** (seit diesem Stand): Alle Scans werden vorab gelesen; mit mehreren Threads läuft jeder Scan in einem eigenen Thread, zeilenweise hinter den Scans, von denen er abhängt (gleiche Komponente, überlappendes Band). Koeffizienten liegen in Blockzeilen hinter Mutexen, werden erst beim ersten Zugriff angelegt und während der parallelen IDCT freigegeben. Verfeinerungsscans führen je Block eine Maske der Nicht-Null-Koeffizienten.

| 24 MP 4:2:0 progressive | turbo | blinzel-jpeg, 1 Thread | 16 Threads |
|---|---|---|---|
| voll | 244 ms | 330 ms | 142 ms (1,7×) |
| 1/8 | 238 ms | 312 ms | 138 ms (1,7×) |

Grenze: Der letzte Y-Verfeinerungsscan (3,7 MB) läuft allein ca. 100 ms. Ohne Kenntnis der Blocknummer lässt er sich nicht spekulativ teilen; schneller wird er nur durch einen schnelleren sequentiellen Decoder.

Sequentiell (Baseline) ist `blinzel-jpeg` seit dem Akkumulator-Bitleser gleichauf mit turbo (1,01×), mit 16 Threads 4,1×.

**M7 (Branch `m7-install`, PR auf `m3-jpeg` gestapelt):** Setup-Exe, Deinstallation, Silent-Modus, Self-Updater mit drei Prüfungen und Rollback, Release-Workflow, winget-Vorlagen, Menüeinträge. Details in [Abschnitt 10](#10-installation-updates-signierung-m7). Offen sind nur Punkte aus Abschnitt 14 (Lizenz, SignPath, Release-Schlüssel) und der erste echte Release.

**Befund Startzeit (echte GPU, 2026-10-23):** Öffnen eines 24-MP-JPEGs aus einem neu gestarteten Prozess dauert 230–330 ms bis zum ersten Bild. Das verfehlt das M1-Ziel (≤ 50 ms warm) klar. Die Startmarken (`--exit-after-first-frame --json`) zeigen, wo die Zeit liegt:

| Phase | GPU (Radeon 680M) | WARP |
|---|---|---|
| vor `main` (Loader, Defender) | 42–60 ms | 43–52 ms |
| `D3D11CreateDevice` allein | 115–150 ms | 65–85 ms |
| Device fertig → erstes Bild | ca. 60 ms | ca. 50 ms |

Der Decode (44 ms bei 1/2) ist nicht der Engpass. Wege, nach Aufwand geordnet:
1. Die Lücke zwischen fertigem Device und erstem Bild (ca. 60 ms) mit Startmarken aufklären und schließen (Renderer, Swapchain und Upload laufen heute nacheinander auf dem UI-Thread bzw. warten aufeinander).
2. Erstes Bild ohne Direct3D: ein vorübergehendes Fenster per `UpdateLayeredWindow` mit dem fertig decodierten Bild, bis der Direct3D-Pfad übernimmt. Erwartet ca. 110 ms statt ca. 270 ms.
3. Resident-Modus (M6): Der warme Prozess zeigt ein neues Bild ohne Prozess- und Device-Start. Nur so ist ≤ 50 ms erreichbar.

**M8 (Branch `m8-race`, PR auf `m7-install` gestapelt):** `blinzel-race` (siehe Abschnitt 11) und die Website: `crates/blinzel-site` baut `website/dist` aus `website/src` (Layout, eine HTML-Datei je Seite und Sprache), `CHANGELOG.md` und `website/data/race.json`; ohne Referenzmessung sagen die Seiten das, statt Zahlen zu zeigen. Strikte CSP ohne Inline-Skript, Handy-Layout geprüft. Deployment über `.github/workflows/website.yml`, sobald die Cloudflare-Secrets da sind.

**Als Nächstes, in dieser Reihenfolge:**
1. Startzeit nach dem Befund oben: zuerst die Lücke zwischen fertigem Device und erstem Bild schließen, dann ein erstes Bild ohne Direct3D, dann M6 mit dem Resident-Modus.
2. Funktionstabelle der anderen Viewer für die Vergleichsseite, jede Angabe mit Quelle und geprüfter Version.
3. M4, M5 wie in Abschnitt 12.
