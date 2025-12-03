# Custom Styling Example

This directory demonstrates how to customize the terminal appearance when embedding `ht` in your application.

## Quick Start

```bash
# Run with example theme
./run-with-theme.sh

# Or manually
cargo build --release
../../target/release/ht \
  --listen 127.0.0.1:8080 \
  --custom-css tailwind-neutral.css
```

Then open http://127.0.0.1:8080 in your browser.

## How It Works

The `--custom-css` flag loads a CSS file from the filesystem at runtime. Your CSS cascades after the default `asciinema-player.css`, allowing you to override any styles.

**Key benefits:**
- No recompilation needed for CSS changes
- Refresh browser to see updates
- Graceful fallback when flag not provided

## Example Theme: Tailwind Neutral

The included `tailwind-neutral.css` demonstrates a complete theme override using Tailwind's neutral color palette:

- **Background**: `#0a0a0a` (neutral-950)
- **Foreground**: `#fafafa` (neutral-50)  
- **ANSI colors**: Vibrant Tailwind accent colors (red-500, green-500, blue-500, etc.)
- **Styling**: Modern shadows, 8px border radius, clean spacing

## Creating Your Own Theme

### 1. Create a CSS file

```css
/* my-theme.css */

/* Override page background */
body {
  background-color: #000000 !important;
  padding: 20px !important;
}

/* Override terminal colors */
.asciinema-player-theme-dracula {
  --term-color-foreground: #ffffff !important;
  --term-color-background: #000000 !important;
  --term-color-0: #000000 !important;  /* black */
  --term-color-1: #ff0000 !important;  /* red */
  --term-color-2: #00ff00 !important;  /* green */
  --term-color-3: #ffff00 !important;  /* yellow */
  --term-color-4: #0000ff !important;  /* blue */
  --term-color-5: #ff00ff !important;  /* magenta */
  --term-color-6: #00ffff !important;  /* cyan */
  --term-color-7: #ffffff !important;  /* white */
  /* Colors 8-15 are bright variants (also use !important) */
}

/* Customize player appearance */
.ap-player {
  box-shadow: 0 4px 6px rgba(0, 0, 0, 0.3) !important;
  border-radius: 8px !important;
}
```

### 2. Test it

```bash
ht --listen 127.0.0.1:8080 --custom-css my-theme.css
```

### 3. Iterate

Edit the CSS file and refresh your browser. No rebuild needed.

## CSS Variables Reference

**Terminal colors:**
- `--term-color-foreground` - Default text color
- `--term-color-background` - Terminal background
- `--term-color-0` through `--term-color-15` - ANSI color palette

**Key elements:**
- `body` - Page background and padding
- `.ap-player` - Player container (shadows, borders)
- `pre.ap-terminal` - Terminal element
- `.ap-cursor` - Cursor styling

## Embedding in Applications

### Node.js

```javascript
const { spawn } = require('child_process');
const path = require('path');

const ht = spawn('./ht', [
  '--listen', '127.0.0.1:8080',
  '--custom-css', path.join(__dirname, 'theme.css'),
  'bash'
]);

// Embed webview at http://127.0.0.1:8080
```

### Python

```python
import subprocess
import os

proc = subprocess.Popen([
    './ht',
    '--listen', '127.0.0.1:8080',
    '--custom-css', os.path.join(os.path.dirname(__file__), 'theme.css'),
    'bash'
])

# Embed webview at http://127.0.0.1:8080
```

## Testing Colors

Use the demo script to generate colorful output:

```bash
python3 demo-colors.py | ht --listen 127.0.0.1:8080 --custom-css theme.css
```

Or test manually with commands that produce colored output:
- `ls --color=auto`
- `git status`
- `grep --color=auto`

## Tips

1. **Use `!important`** - Default styles are specific, ensure your overrides apply
2. **Define all 16 colors** - Some apps use specific ANSI color codes
3. **Test with real programs** - Try vim, git, htop to see colors in practice
4. **Check contrast** - Verify text is readable against backgrounds
5. **Start with the example** - Copy `tailwind-neutral.css` as a starting point

## Troubleshooting

**CSS not loading?**
- Check file path is correct (relative to where you run `ht`)
- Verify file is readable
- Look for error messages in terminal output

**Styles not applying?**
- Add `!important` to your rules
- Use browser dev tools to inspect elements
- Clear browser cache (Cmd+Shift+R / Ctrl+Shift+R)

**Changes not visible?**
- For custom CSS: Refresh browser (no rebuild needed)
- For `index.html` changes: Rebuild with `cargo build --release`
