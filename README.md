# Skrinshotter

A lightweight, cross-platform screenshot application (**Windows** & **macOS**) built with **Rust** that runs quietly in the background (**System Tray** on Windows & **Status Bar / Menu Bar** on macOS).

## Key Features

1. **Background System Tray & macOS Status Bar**
* Runs quietly in the background (`ControlFlow::Wait`, 0% idle CPU usage).
* **Windows**: Sits in the System Tray (left-click to immediately *Capture Area*, right-click to open the context menu). In `--release` mode, no console window is displayed.
* **macOS**: Operates with `ActivationPolicy::Accessory`, appearing cleanly as a template icon in the **Status Bar (Menu Bar)** without cluttering the Dock.
* **Global Shortcut**: Press `Ctrl + Shift + S` anytime to start capturing an area.


2. **Custom Area Capture — Rectangle & Freeform (Multi-Monitor & HiDPI/Retina)**
* Instantly freezes the screen display and opens a borderless overlay.
* **Rectangle Mode (`▭ Rectangle`)**: Click and drag a selection box with a real-time pixel counter (`W × H px`).
* **Freeform Mode (`〰 Freeform / Lasso`)**: Draw freely around any custom shape on screen; areas outside the drawn path are automatically clipped to transparent with smooth anti-aliased edges.
* Switch modes directly inside the overlay via the top toolbar buttons, keyboard shortcuts (`Tab` / `F` / `1` / `2`), or straight from the System Tray / Status Bar menu. Press `Esc` or right-click to cancel.


3. **Capture Preview Box & Full-Color Annotation**
* Automatically appears once an area is selected (includes a transparency checkerboard background so **Freeform** cutouts are clearly visible).
* **Annotation Tools**: `✏ Freehand Pen`, `🖍 Highlighter`, `▭ Rectangle`, and `↗ Arrow`.
* **Any Color**: Equipped with a full color picker (RGB / HSV / Hex / Alpha) along with quick color swatch presets.
* Supports adjustable stroke thickness (`1–32 px`), `Undo` (`Ctrl/Cmd + Z`), `Clear Canvas`, and `Copy Image` to clipboard.
* All annotations are re-rendered and baked with anti-aliasing at the **native pixel resolution** upon saving.


4. **Metadata Form & Format Options (PNG / JPEG)**
* **Title**
* **Description / Comment**
* **Author** — can be saved as **Default Author** in Settings to automatically populate on future captures.
* **Tags / Keywords** — comma-separated (e.g., `bug, login, ui`).
* **Save as PNG or JPEG**:
* **PNG**: Lossless, sharp, and preserves transparency outside freeform boundaries. Embeds metadata using standard `iTXt` chunks (UTF-8), `XML:com.adobe.xmp` (Dublin Core XMP), and `eXIf`.
* **JPEG**: The outer area of freeform captures is smoothly composited over a clean white background. Embeds metadata through standard `APP1` EXIF segments, Windows Explorer UTF-16LE tags (`XPTitle`, `XPComment`, `XPAuthor`, `XPKeywords`), and `APP1` XMP for instant compatibility with Windows File Explorer *Details* and macOS Finder/Preview.





---

## How to Run & Build

### Prerequisites

* Rust toolchain (`rustc` & `cargo` 1.80+).

### Running in Development Mode

```bash
cargo run

```

### Building Production Binary (Release)

```bash
cargo build --release

```

The compiled binaries are available at:

* **Windows**: `target/release/skrinshotter.exe`
* **macOS**: `target/release/skrinshotter`

### macOS Note (Screen Recording Permission)

On macOS (Catalina and later), the system will prompt for **Screen Recording** permission the first time you initiate an area capture. Make sure to grant permission for your terminal or the `skrinshotter` binary under:
`System Settings` -> `Privacy & Security` -> `Screen Recording`.

### Settings Storage Path (`Default Author` & `Default Capture Mode`)

Preferences are automatically saved in JSON format within the standard OS configuration directory:

* **Windows**: `%APPDATA%\skrinshotter\settings.json`
* **macOS**: `~/Library/Application Support/skrinshotter/settings.json`
