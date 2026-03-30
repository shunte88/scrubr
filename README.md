# scrubr

[![Build Status](https://github.com/shunte88/LyMonS/actions/workflows/rust.yml/badge.svg)](https://github.com/shunte88/scrubr/actions/workflows/rust.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache2-blue.svg)](https://www.apache.org/licenses/LICENSE-2.0)
(https://www.gnu.org/licenses/apache-2.0)
![version](version.svg)

The Rust SVG scrubber — an SVG optimizer/cleaner with full **substitution variable support** for dynamic SVG templating systems.

---

## Features

| Category | Optimization |
|---|---|
| **Structure** | Collapse empty `<g>` elements, remove unreferenced `<defs>` entries |
| **Editor data** | Strip Inkscape, Sodipodi, Adobe Illustrator, Sketch namespaces/attributes |
| **Descriptive** | Remove `<title>`, `<desc>`, `<metadata>` |
| **Comments** | Strip `<!-- -->` nodes |
| **Colors** | Normalize to shortest `#RGB` / `#RRGGBB` hex; convert named colors and `rgb(...)` |
| **Style** | Convert `style=""` presentation properties to XML attributes |
| **Embedded CSS** | Optimize `<style>` block content: color simplification, default stripping, ID remapping |
| **Defaults** | Remove attributes whose value equals the SVG specification default |
| **IDs** | Strip unreferenced IDs, shorten IDs to minimal length, prefix/list/Inkscape protection |
| **Gradients** | Deduplicate identical `<linearGradient>`, `<radialGradient>`, `<pattern>` definitions |
| **Paths** | Optimize `d=""` path data: precision rounding, redundant command removal, absolute conversion, `H`/`V`/`S`/`T` expansion, collinear `L` merging (`--simplify-paths`) |
| **Path combination** | Merge consecutive `<path>` siblings with identical presentation attributes (`--combine-paths`) |
| **Empty defs** | Remove empty `<defs/>` elements that contain no usable content |
| **Transforms** | Simplify `matrix(...)` to `translate/scale/rotate`, drop identities |
| **Numbers** | Apply configurable significant-digit precision to all numeric attributes |
| **Viewboxing** | Rewrite `width`/`height` to `100%` and add `viewBox` |
| **Group creation** | Group runs of siblings with identical presentation attributes into `<g>` |
| **SVGZ** | Read and write gzip-compressed `.svgz` files transparently |
| **Output** | Configurable indentation (space / tab / none), line-break control |

### Substitution Variable Support

SVG files used as templates often contain `{{variable}}`, `{{my-var}}`, or `{{my_var}}`
placeholders that are replaced at render time. scrubr **fully preserves** those tokens
throughout every optimization phase:

- Path data containing `{{...}}` is **not modified**
- Transform values containing `{{...}}` are **not modified**
- Color values containing `{{...}}` are **not simplified**
- `<style>` block rules whose values contain `{{...}}` are **not simplified or stripped**
- Gradient attribute values containing `{{...}}` make that gradient **unique** (never deduplicated)
- Attribute values with `{{...}}` have their **normal ID references still remapped** correctly
- `{{...}}` tokens inside text content, CDATA sections, and comments are preserved verbatim

---

## Installation

```bash
git clone https://github.com/shunte88/scrubr
cd scrubr
cargo build --release
# Binary: target/release/scrubr
```

```bash
cargo install --path .
```

---

## Usage

```
scrubr [INPUT.SVG [OUTPUT.SVG]] [OPTIONS]
```

stdin/stdout used when files are omitted. `.svgz` triggers automatic gzip handling.

### Quick examples

```bash
# Standard
scrubr -i input.svg -o output.svg

# Browser compatibility
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

# Substitution variables are always preserved automatically
scrubr -i template.svg -o template.min.svg --indent=none
# {{fill-color}}, {{icon_label}}, {{stroke_width}} → untouched in output
```

---

## All Options

### General

| Flag | Description |
|---|---|
| `-i INPUT.SVG` | Input file (default: stdin) |
| `-o OUTPUT.SVG` | Output file (default: stdout) |
| `-q`, `--quiet` | Suppress non-error output |
| `-v`, `--verbose` | Verbose output (file size, gradient dedup stats, etc.) |

### Optimization

| Flag | Default | Description |
|---|---|---|
| `--set-precision=NUM` | 5 | Significant digits for numeric values |
| `--set-c-precision=NUM` | same | Significant digits for path control points |
| `--disable-simplify-colors` | off | Don't convert colors to `#RRGGBB` |
| `--disable-style-to-xml` | off | Don't convert `style=""` to XML attributes |
| `--disable-group-collapsing` | off | Don't collapse empty `<g>` elements |
| `--create-groups` | off | Group siblings with identical presentation attrs into `<g>` |
| `--keep-editor-data` | off | Keep Inkscape/Illustrator/Sketch data |
| `--keep-unreferenced-defs` | off | Keep unreferenced `<defs>` entries |
| `--simplify-paths` | off | Simplify path data: absolute coords, expand `H`/`V`/`S`/`T`, merge collinear `L` |
| `--combine-paths` | off | Combine consecutive `<path>` siblings with identical attributes |
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
| `--nindent=NUM` | `1` | Spaces/tabs per indent level |
| `--no-line-breaks` | off | Output on a single line |
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

## Architecture

```
src/
├── main.rs         CLI parsing, I/O, SVGZ handling
├── optimizer.rs    Core engine: protect/parse/analyse/serialize/restore
│                     Gradient dedup integration
│                     <style> block delegation
│                     create-groups integration
│                     Group collapsing, editor stripping, viewboxing
│                     ID resolution, namespace handling
├── gradient.rs     Gradient/pattern deduplication
│                     Canonical key hashing, inheritance resolution
├── style_block.rs  <style> element CSS optimizer
│                     Tokeniser, rule parser, color rewrite, ID remapping
│                     @media/@keyframes pass-through
├── groups.rs       --create-groups implementation
│                     Run detection, attribute intersection, <g> wrapping
├── css.rs          style="" parser, style→XML conversion, default detection
├── color.rs        Full CSS color keyword table, hex normalization, rgb()
├── path.rs         SVG path d="" optimizer
├── path_simplify.rs Path simplification (absolute conversion, H/V/S/T expansion,
│                     collinear L merge) and picosvg-style path combination
├── transform.rs    Transform simplifier (matrix→translate/scale/rotate)
└── ids.rs          Short-ID generator, protection logic, rename-map builder
```

---

## Comparison with Python scour

| Feature | Python scour | scrubr |
|---|---|---|
| Substitution variables `{{...}}` | Corrupts them | **Fully preserved** |
| Gradient deduplication | ✓ | ✓ |
| `<style>` block CSS optimization | Basic | ✓ Full (color, defaults, IDs, @media) |
| `--create-groups` | ✓ | ✓ |
| Path simplification (`--simplify-paths`) | — | ✓ Absolute, H/V/S/T expand, collinear merge |
| Path combination (`--combine-paths`) | — | ✓ picosvg-style sibling merge |
| Empty `<defs/>` removal | ✓ | ✓ |
| Performance | ~seconds on large files | Typically 10–100× faster |
| SVGZ | ✓ | ✓ |

---

## License

Apache License 2.0


