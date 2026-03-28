# scrubr

The Rust SVG scrubber, AKA scrubr - an SVG optimizer/cleaner, with full **substitution variable support** for dynamic SVG templating systems.

---

## Features

Scrubr has the following feature set:

| Category | Optimization |
|---|---|
| **Structure** | Collapse empty `<g>` elements, remove unreferenced `<defs>` entries |
| **Editor data** | Strip Inkscape, Sodipodi, Adobe Illustrator, Sketch namespaces/attributes |
| **Descriptive** | Remove `<title>`, `<desc>`, `<metadata>` |
| **Comments** | Strip `<!-- -->` nodes |
| **Colors** | Normalize to shortest `#RGB` / `#RRGGBB` hex; convert named colors and `rgb(...)` |
| **Style** | Convert `style=""` presentation properties to XML attributes |
| **Defaults** | Remove attributes whose value equals the SVG specification default |
| **IDs** | Strip unreferenced IDs, shorten IDs to minimal length, prefix/list/Inkscape protection |
| **Paths** | Optimize `d=""` path data: precision rounding, redundant command removal |
| **Transforms** | Simplify `matrix(...)` to `translate/scale/rotate`, drop identities |
| **Numbers** | Apply configurable significant-digit precision to all numeric attributes |
| **Viewboxing** | Rewrite `width`/`height` to `100%` and add `viewBox` |
| **SVGZ** | Read and write gzip-compressed `.svgz` files transparently |
| **Output** | Configurable indentation (space / tab / none), line-break control |

### Substitution Variable Support (unique to this port)

SVG files used as templates often contain `{{variable}}`, `{{my-var}}`, or `{{my_var}}`
placeholders that are replaced at render time. This optimizer **fully preserves** those
tokens throughout every optimization phase:

- Path data containing `{{...}}` is **not modified**
- Transform values containing `{{...}}` are **not modified**
- Color values containing `{{...}}` are **not simplified**
- Attribute values with `{{...}}` have their **ID references still remapped** (normal IDs
  within the value are updated; the placeholder itself is never altered)
- `{{...}}` tokens inside text content, CDATA sections, and comments are preserved verbatim

This means you can freely run scrubr in a build pipeline before template substitution
without any risk of corrupting your variables.

---

## Installation

### From source

```bash
git clone <this-repo>
cd scrubr
cargo build --release
# Binary at: target/release/scrubr
```

### Install globally

```bash
cargo install --path .
```

---

## Usage

```
scrubr [INPUT.SVG [OUTPUT.SVG]] [OPTIONS]
```

If input/output are omitted, stdin/stdout are used. `.svgz` extension triggers
automatic gzip decompression/compression.

### Quick examples

```bash
# Standard optimization
scrubr -i input.svg -o output.svg

# Better browser compatibility (add viewBox)
scrubr -i input.svg -o output.svg --enable-viewboxing

# Maximum scrubbing
scrubr -i input.svg -o output.svg \
  --enable-viewboxing \
  --enable-id-stripping \
  --enable-comment-stripping \
  --shorten-ids \
  --indent=none

# Compressed output
scrubr -i input.svg -o output.svgz \
  --enable-viewboxing --enable-id-stripping \
  --enable-comment-stripping --shorten-ids --indent=none

# Preserve substitution variables (always automatic, no flag needed)
scrubr -i template.svg -o template.min.svg --indent=none
# {{color}}, {{icon-name}}, {{stroke_width}} are untouched in output
```

---

## All Options

### General

| Flag | Description |
|---|---|
| `-i INPUT.SVG` | Input file (default: stdin) |
| `-o OUTPUT.SVG` | Output file (default: stdout) |
| `-q`, `--quiet` | Suppress non-error output |
| `-v`, `--verbose` | Verbose output (file size statistics) |

### Optimization

| Flag | Default | Description |
|---|---|---|
| `--set-precision=NUM` | 5 | Significant digits for numeric values |
| `--set-c-precision=NUM` | same as above | Significant digits for path control points |
| `--disable-simplify-colors` | off | Don't convert colors to `#RRGGBB` |
| `--disable-style-to-xml` | off | Don't convert `style=""` to XML attributes |
| `--disable-group-collapsing` | off | Don't collapse empty `<g>` elements |
| `--create-groups` | off | Create `<g>` for identical-attribute runs |
| `--keep-editor-data` | off | Keep Inkscape/Illustrator/Sketch data |
| `--keep-unreferenced-defs` | off | Keep unreferenced `<defs>` entries |
| `--renderer-workaround` | on | Apply librsvg bug workarounds |
| `--no-renderer-workaround` | off | Disable renderer workarounds |

### SVG Document

| Flag | Description |
|---|---|
| `--strip-xml-prolog` | Omit `<?xml version="1.0"?>` declaration |
| `--remove-titles` | Remove `<title>` elements |
| `--remove-descriptions` | Remove `<desc>` elements |
| `--remove-metadata` | Remove `<metadata>` elements |
| `--remove-descriptive-elements` | Remove all of `<title>`, `<desc>`, `<metadata>` |
| `--enable-comment-stripping` | Remove all `<!-- -->` comments |
| `--disable-embed-rasters` | Don't embed rasters as base64 |
| `--enable-viewboxing` | Set `width`/`height` to 100% and add `viewBox` |

### Output Formatting

| Flag | Default | Description |
|---|---|---|
| `--indent=TYPE` | `space` | Indentation: `none`, `space`, `tab` |
| `--nindent=NUM` | `1` | Number of spaces/tabs per indent level |
| `--no-line-breaks` | off | Output on a single line (also disables indent) |
| `--strip-xml-space` | off | Remove `xml:space="preserve"` from root `<svg>` |

### ID Attributes

| Flag | Description |
|---|---|
| `--enable-id-stripping` | Remove all unreferenced IDs |
| `--shorten-ids` | Shorten IDs to minimum length (`a`, `b`, …, `aa`, …) |
| `--shorten-ids-prefix=PREFIX` | Prefix for shortened IDs |
| `--protect-ids-noninkscape` | Don't remove IDs not ending with a digit |
| `--protect-ids-list=LIST` | Comma-separated list of IDs to never remove |
| `--protect-ids-prefix=PREFIX` | Don't remove IDs starting with this prefix |

### SVG Compatibility

| Flag | Description |
|---|---|
| `--error-on-flowtext` | Exit with error if SVG uses nonstandard flowing text |

---

## Substitution Variable Reference

Variables matching the pattern `{{word}}`, `{{word-word}}`, or `{{word_word}}`
(where `word` is alphanumeric with optional `-` or `_`) are protected end-to-end.

**Supported in:**
- Attribute values: `fill="{{primary-color}}"`, `d="M {{x}} {{y}} L …"`
- Style values: `style="fill:{{color}};opacity:{{opacity}}"`
- Text content: `<text>{{label}}</text>`
- Any combination of the above

**Not treated as substitution variables:**
- `{{ spaced }}` — spaces inside braces
- `{{!special}}` — special characters inside braces
- `{single}` — single braces (passed through unchanged)

---

## Architecture

```
src/
├ main.rs        CLI parsing, I/O, SVGZ handling
├ optimizer.rs   Core engine: XML parse → analyse → serialize
│                  Substitution variable protect/restore
│                  Group collapsing, editor data stripping,
│                  descriptive element removal, viewboxing,
│                  ID resolution, namespace handling
├ css.rs         Style attribute parser, style→XML conversion,
│                  default value detection
├ color.rs       Color keyword table, hex normalization, rgb() parsing
├ path.rs        SVG path `d` attribute optimizer
├ transform.rs   Transform simplifier (matrix→translate/scale/rotate)
└ ids.rs         ID shortening, protection logic, rename-map builder
```

---

## Differences from Python scour

| Feature | Python scour | scrubr (Rust) |
|---|---|---|
| Substitution variables `{{...}}` | Breaks them | **Fully preserved** |
| Performance | ~seconds on large files | Typically 10–100× faster |
| SVGZ | ✓ | ✓ |
| API | Python library | Rust crate (lib usable too) |
| Gradient deduplication | ✓ | Planned |
| CSS embedded in `<style>` | Basic | Planned |
| `--create-groups` | ✓ | Planned (flag accepted, no-op) |

incredible speed and type safety

---

## License

Apache License 2.0
