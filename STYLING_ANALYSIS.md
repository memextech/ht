# HT Terminal Styling Analysis

## Overview
The `ht` (headless terminal) project uses the **asciinema-player** library for rendering terminal output in a web interface. The styling system is primarily CSS-based with embedded static assets served through Rust's `rust-embed` crate.

## Architecture

### 1. Asset Delivery System
- **Location**: `assets/` directory
- **Embedding**: Assets are embedded into the binary at compile time using `rust-embed`
- **Server**: HTTP server (`src/api/http.rs`) serves static files via `static_handler()`
- **Key Files**:
  - `index.html` - Main HTML page
  - `asciinema-player.css` - Comprehensive styling (2,352 lines)
  - `asciinema-player.min.js` - Player JavaScript logic

### 2. Web Server Integration
```rust
#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;
```
The server runs on a user-specified address (default: `127.0.0.1:8080`) and serves:
- `/` → `index.html`
- `/asciinema-player.css` → CSS file
- `/asciinema-player.min.js` → JavaScript player
- `/ws/alis` → WebSocket for live terminal streaming (ALiS protocol)
- `/ws/events` → WebSocket for event streaming

## Styling System Details

### 3. Main HTML Structure (`index.html`)

#### Current Theme Configuration:
```javascript
const opts = {
  logger: console,
  fit: 'both',
  theme: 'dracula',    // ← Current theme
  controls: false,
  autoPlay: true
};
```

#### Page-Level Styles:
```css
body {
  padding: 40px;
  background-color: #282a36;  /* Dracula background */
}

.ap-player {
  box-shadow: #141518 0px 0px 60px 5px;  /* Dark shadow effect */
  margin: auto 0px;
}
```

### 4. CSS Color System (`asciinema-player.css`)

The styling uses **CSS custom properties (variables)** for flexible theming:

#### Base Color Variables:
```css
.ap-player {
  --term-color-foreground: #ffffff;
  --term-color-background: #000000;
  --term-color-0 through --term-color-15: /* 16 terminal colors */
}
```

#### Color Mapping:
- `--term-color-0` to `--term-color-7`: Standard ANSI colors
- `--term-color-8` to `--term-color-15`: Bright ANSI colors
- Each color can be applied as foreground (`.fg-N`) or background (`.bg-N`)
- Bright colors (8-15) automatically get `font-weight: bold`

### 5. Available Themes

The player includes **7 built-in themes**:

#### 1. **asciinema** (Line 2191)
- Background: `#121314`
- Foreground: `#cccccc`
- Uses HSL colors for terminal palette

#### 2. **dracula** (Line 2214) ⭐ *Currently Active*
- Background: `#282a36`
- Foreground: `#f8f8f2`
- Based on: https://draculatheme.com
- Colors: Purple (`#bd93f9`), Pink (`#ff79c6`), Cyan (`#8be9fd`), etc.

#### 3. **monokai** (Line 2235)
- Background: `#272822`
- Foreground: `#f8f8f2`
- Based on: base16 collection

#### 4. **nord** (Line 2253)
- Background: `#2e3440`
- Foreground: `#eceff4`
- Based on: https://github.com/arcticicestudio/nord
- Polar night color palette

#### 5. **seti** (Line 2265)
- Background: `#111213`
- Foreground: `#cacecd`
- High contrast dark theme

#### 6. **solarized-dark** (Line 2281)
- Background: `#002b36`
- Foreground: `#839496`
- Based on: https://ethanschoonover.com/solarized/

#### 7. **solarized-light** (Line 2304)
- Background: `#fdf6e3`
- Foreground: `#657b83`
- Light theme variant with custom play button colors

#### 8. **tango** (Line 2333)
- Background: `#121314`
- Foreground: `#cccccc`
- Based on: Tango Desktop Project

### 6. Terminal Rendering Styles

#### Terminal Container:
```css
pre.ap-terminal {
  border-width: 0.75em;
  color: var(--term-color-foreground);
  background-color: var(--term-color-background);
  border-color: var(--term-color-background);
  line-height: var(--term-line-height);
  font-family: Consolas, Menlo, 'Bitstream Vera Sans Mono', monospace, 'Powerline Symbols';
  font-variant-ligatures: none;
}
```

#### Line Rendering:
- Each line has class `.ap-line`
- Fixed height: `var(--term-line-height)`
- Spans positioned absolutely using `--offset` and `--term-cols`

#### Text Styling Classes:
- `.ap-bright` → `font-weight: bold`
- `.ap-faint` → `opacity: 0.5`
- `.ap-underline` → Text underlined
- `.ap-italic` → Italic text
- `.ap-strikethrough` → Line through text
- `.ap-inverse` → Swaps foreground/background colors
- `.ap-blink` → Blinking text (hidden when `.ap-blink` class is not on terminal)

#### Cursor:
```css
pre.ap-terminal.ap-cursor-on .ap-line .ap-cursor {
  color: var(--bg);
  background-color: var(--fg);
  border-radius: 0.05em;
}
```

### 7. Control Bar Styling

#### Structure:
```css
div.ap-player div.ap-control-bar {
  height: 32px;
  color: var(--term-color-foreground);
  opacity: 0;  /* Hidden by default */
  transition: opacity 0.15s linear;
  border-top: 2px solid color-mix(in oklab, black 33%, var(--term-color-background));
  z-index: 30;
}
```

#### Components:
- **Playback button**: 12x12px SVG icon, padding 10px
- **Timer**: Consolas/Menlo font, 13px, shows elapsed/remaining time
- **Progress bar**: Visual timeline with markers
- **Fullscreen button**: Standard control

#### Current Configuration:
The `index.html` sets `controls: false`, so the control bar is disabled in the current build.

### 8. Overlay System

#### Types of Overlays:
1. **Start Overlay** (`.ap-overlay-start`):
   - Play button with drop shadow
   - 80px height, centered
   - SVG icon with white fill

2. **Loading Overlay** (`.ap-overlay-loading`):
   - Animated spinner using CSS animation
   - Uses `color-mix()` for gradient effect
   - 48x48px rotating border

3. **Info Overlay** (`.ap-overlay-info`):
   - Full background color
   - 2em font size
   - Terminal font family

4. **Help Overlay** (`.ap-overlay-help`):
   - Dark semi-transparent background (`rgba(0, 0, 0, 0.8)`)
   - Keyboard shortcuts display
   - Responsive sizing using container queries

5. **Error Overlay** (`.ap-overlay-error`):
   - 8em font size for error messages

### 9. Special Character Rendering

The CSS includes extensive support for Unicode box-drawing characters:
- `.cp-2580` through `.cp-259f`: Various block elements
- `.cp-e0b0`, `.cp-e0b2`: Powerline separators
- Uses CSS borders and `box-sizing` for precise rendering

### 10. Box Shadow & Visual Effects

```css
.ap-player {
  box-shadow: #141518 0px 0px 60px 5px;  /* Soft glow effect */
  border-radius: 4px;
}
```

## Customization Points

### Easy Modifications:

1. **Change Theme**: Edit `index.html`, line ~36:
   ```javascript
   theme: 'nord',  // or 'monokai', 'solarized-dark', etc.
   ```

2. **Change Page Background**: Edit `index.html`, body style:
   ```css
   background-color: #000000;
   ```

3. **Adjust Padding**: Edit `index.html`, body padding:
   ```css
   padding: 20px;  /* Reduce from 40px */
   ```

4. **Enable Controls**: Edit `index.html`, opts object:
   ```javascript
   controls: true,
   ```

5. **Adjust Shadow**: Edit `index.html`, `.ap-player` style:
   ```css
   box-shadow: rgba(0, 0, 0, 0.8) 0px 10px 50px;
   ```

### Advanced Modifications:

1. **Create Custom Theme**: Add new theme class to `asciinema-player.css`:
   ```css
   .asciinema-player-theme-custom {
     --term-color-foreground: #yourcolor;
     --term-color-background: #yourcolor;
     --term-color-0: #yourcolor;
     /* ... define all 16 colors ... */
   }
   ```

2. **Modify Terminal Border**: Edit `.ap-terminal` border-width property

3. **Change Font**: Edit `font-family` in `pre.ap-terminal`

4. **Adjust Line Height**: Modify `--term-line-height` variable

## Current Configuration Summary

| Setting | Value |
|---------|-------|
| **Theme** | dracula |
| **Background Color (page)** | #282a36 |
| **Background Color (terminal)** | #282a36 |
| **Font** | Consolas, Menlo, Bitstream Vera Sans Mono |
| **Border Width** | 0.75em |
| **Controls** | Disabled |
| **Auto-play** | Enabled |
| **Fit Mode** | both |
| **Shadow** | #141518 0px 0px 60px 5px |
| **Padding** | 40px |

## File Locations

```
ht/
├── assets/
│   ├── index.html              ← Main configuration
│   ├── asciinema-player.css    ← All themes and styles
│   └── asciinema-player.min.js ← Player logic
├── src/
│   ├── api/
│   │   └── http.rs             ← Asset serving
│   └── cli.rs                  ← CLI options (no theme options)
```

## Terminal Environment

The PTY (pseudo-terminal) is configured with:
```rust
env::set_var("TERM", "xterm-256color");
```

This enables 256-color support for applications running inside the terminal.

## Rebuild Requirements

After modifying assets:
1. Files are embedded at **compile time**
2. Must run `cargo build --release` to rebuild
3. Changes to `index.html` or CSS require recompilation
4. No hot-reload for asset changes

## WebSocket Protocols

### ALiS (asciinema live stream):
- Endpoint: `/ws/alis`
- Sends terminal output in real-time
- Compatible with asciinema-player's live streaming

### Event Stream:
- Endpoint: `/ws/events`
- Events: init, output, resize, snapshot
- Subscribe via query parameter: `?sub=output+resize`

## Browser Compatibility

The CSS uses modern features:
- CSS custom properties (variables)
- `color-mix()` function
- Container queries (`container-type: inline-size`)
- CSS animations
- Flexbox layout

Requires modern browsers (Chrome 88+, Firefox 91+, Safari 15+).
