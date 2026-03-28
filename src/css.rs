/// CSS style attribute parsing and conversion helpers.
/// Substitution variables ({{...}}) are preserved throughout.

use crate::color::simplify_color;

/// SVG presentation attributes — these can be inlined from style=""
pub const PRESENTATION_ATTRS: &[&str] = &[
    "alignment-baseline", "baseline-shift", "clip", "clip-path", "clip-rule",
    "color", "color-interpolation", "color-interpolation-filters", "color-profile",
    "color-rendering", "cursor", "direction", "display", "dominant-baseline",
    "enable-background", "fill", "fill-opacity", "fill-rule", "filter",
    "flood-color", "flood-opacity", "font-family", "font-size", "font-size-adjust",
    "font-stretch", "font-style", "font-variant", "font-weight", "glyph-orientation-horizontal",
    "glyph-orientation-vertical", "image-rendering", "kerning", "letter-spacing",
    "lighting-color", "marker", "marker-end", "marker-mid", "marker-start", "mask",
    "opacity", "overflow", "paint-order", "pointer-events", "shape-rendering",
    "stop-color", "stop-opacity", "stroke", "stroke-dasharray", "stroke-dashoffset",
    "stroke-linecap", "stroke-linejoin", "stroke-miterlimit", "stroke-opacity",
    "stroke-width", "text-anchor", "text-decoration", "text-rendering",
    "unicode-bidi", "vector-effect", "visibility", "word-spacing", "writing-mode",
];

/// Color-related style properties
const COLOR_PROPS: &[&str] = &[
    "fill", "stroke", "stop-color", "flood-color", "lighting-color", "color",
];

/// Parse a CSS style string into ordered (property, value) pairs.
/// Preserves substitution variables verbatim.
pub fn parse_style(style: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    // We need to split by ';' but be careful not to split inside {{ }}
    let mut decls = smart_split_style(style);
    for decl in &mut decls {
        let decl = decl.trim();
        if decl.is_empty() { continue; }
        // Find first ':' that is not inside {{ }}
        if let Some(colon_pos) = find_colon(decl) {
            let prop = decl[..colon_pos].trim().to_lowercase();
            let val  = decl[colon_pos+1..].trim().to_string();
            if !prop.is_empty() {
                result.push((prop, val));
            }
        }
    }
    result
}

/// Split a style string by ';', but not inside {{ ... }}
fn smart_split_style(style: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let chars: Vec<char> = style.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '{' && chars[i+1] == '{' {
            depth += 1;
            cur.push(chars[i]);
            cur.push(chars[i+1]);
            i += 2;
            continue;
        }
        if i + 1 < chars.len() && chars[i] == '}' && chars[i+1] == '}' {
            if depth > 0 { depth -= 1; }
            cur.push(chars[i]);
            cur.push(chars[i+1]);
            i += 2;
            continue;
        }
        if chars[i] == ';' && depth == 0 {
            parts.push(cur.clone());
            cur.clear();
            i += 1;
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

fn find_colon(s: &str) -> Option<usize> {
    let chars: Vec<char> = s.chars().collect();
    let mut depth = 0usize;
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '{' && chars[i+1] == '{' {
            depth += 1;
            i += 2;
            continue;
        }
        if i + 1 < chars.len() && chars[i] == '}' && chars[i+1] == '}' {
            if depth > 0 { depth -= 1; }
            i += 2;
            continue;
        }
        if chars[i] == ':' && depth == 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Serialize (property, value) pairs back to a style string, sorted for determinism.
pub fn serialize_style(decls: &[(String, String)]) -> String {
    let mut sorted = decls.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    sorted.iter()
        .map(|(k, v)| format!("{}:{}", k, v))
        .collect::<Vec<_>>()
        .join(";")
}

/// Simplify color values in a parsed style declaration list.
/// Leaves substitution variables intact.
pub fn simplify_style_colors(decls: &mut Vec<(String, String)>, simplify: bool) {
    if !simplify { return; }
    for (prop, val) in decls.iter_mut() {
        if COLOR_PROPS.contains(&prop.as_str()) {
            *val = simplify_color(val);
        }
    }
}

/// Returns declarations that are valid presentation attributes (can be moved to XML attrs).
/// Removes them from the input and returns them separately.
pub fn extract_presentation_attrs(
    decls: &mut Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut extracted = Vec::new();
    decls.retain(|d| {
        if PRESENTATION_ATTRS.contains(&d.0.as_str()) {
            extracted.push(d.clone());
            false
        } else {
            true
        }
    });
    extracted
}

/// Default values for presentation attributes — don't emit if value equals default.
pub fn is_default_value(prop: &str, val: &str) -> bool {
    // Don't strip defaults that contain substitution variables
    if val.contains("{{") { return false; }
    match prop {
        "fill"             => val == "black" || val == "#000" || val == "#000000",
        "fill-opacity"     => val == "1",
        "fill-rule"        => val == "nonzero",
        "stroke"           => val == "none",
        "stroke-opacity"   => val == "1",
        "stroke-width"     => val == "1",
        "stroke-linecap"   => val == "butt",
        "stroke-linejoin"  => val == "miter",
        "stroke-miterlimit"=> val == "4",
        "stroke-dasharray" => val == "none",
        "stroke-dashoffset"=> val == "0",
        "opacity"          => val == "1",
        "display"          => val == "inline",
        "visibility"       => val == "visible",
        "overflow"         => val == "visible",
        "color-interpolation" => val == "sRGB",
        "color-rendering"  => val == "auto",
        "image-rendering"  => val == "auto",
        "shape-rendering"  => val == "auto",
        "text-rendering"   => val == "auto",
        "font-style"       => val == "normal",
        "font-variant"     => val == "normal",
        "font-weight"      => val == "normal",
        "font-stretch"     => val == "normal",
        "text-decoration"  => val == "none",
        "text-anchor"      => val == "start",
        "direction"        => val == "ltr",
        "unicode-bidi"     => val == "normal",
        "cursor"           => val == "auto",
        "pointer-events"   => val == "visiblePainted",
        "stop-opacity"     => val == "1",
        "flood-opacity"    => val == "1",
        _ => false,
    }
}
