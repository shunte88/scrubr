/// Gradient deduplication for `<defs>`.
///
/// Scans all gradient elements (`<linearGradient>`, `<radialGradient>`,
/// `<pattern>`) inside `<defs>`, canonicalises their representation, and
/// builds a rename map: duplicate_id → canonical_id.
///
/// Substitution variables in gradient attributes make that gradient unique
/// (two gradients whose only difference is a substitution variable are NOT
/// considered identical, since the runtime value may differ).

use std::collections::HashMap;

/// A fully-resolved gradient description used as a dedup key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GradientKey {
    /// Element tag name
    pub tag: String,
    /// Sorted (name, value) pairs of defining attributes
    /// (everything except `id` and `xlink:href`/`href` inheritance links)
    pub attrs: Vec<(String, String)>,
    /// Canonicalised child stop list: sorted (offset, stop-color, stop-opacity)
    pub stops: Vec<StopKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StopKey {
    pub offset: String,
    pub color: String,
    pub opacity: String,
}

/// Attributes that define the *appearance* of a gradient
/// (exclude `id`, inheritance `href`/`xlink:href`, `gradientTransform` is included).
const GRADIENT_DEFINE_ATTRS: &[&str] = &[
    "cx", "cy", "r", "fx", "fy", "fr",
    "x1", "y1", "x2", "y2",
    "gradientUnits", "gradientTransform",
    "spreadMethod",
    "patternUnits", "patternTransform",
    "patternContentUnits",
    "x", "y", "width", "height",
    "viewBox", "preserveAspectRatio",
];

/// Parsed representation of a single gradient/pattern element extracted
/// from the raw SVG text for dedup purposes.
#[derive(Debug, Clone)]
pub struct GradientDef {
    pub id: String,
    pub key: GradientKey,
    /// The id this gradient inherits from (xlink:href / href), if any
    pub inherits: Option<String>,
}

//  Public API─

/// Given a flat list of gradient definitions extracted from `<defs>`,
/// return a map of `old_id → canonical_id` for every gradient that is
/// a duplicate of a previously-seen one.
///
/// The caller is responsible for:
///   1. Extracting gradient defs from the parsed document.
///   2. Applying the returned renames to all `url(#...)` references in the SVG.
///   3. Removing the now-unreferenced duplicate elements from `<defs>`.
pub fn find_duplicate_gradients(defs: &[GradientDef]) -> HashMap<String, String> {
    // First pass: resolve inheritance chains so every gradient has full stop data.
    let resolved = resolve_inheritance(defs);

    let mut canonical: HashMap<GradientKey, String> = HashMap::new();
    let mut renames: HashMap<String, String> = HashMap::new();

    for def in &resolved {
        // Gradients containing substitution variable placeholders are always unique
        if key_has_subst(&def.key) {
            canonical.entry(def.key.clone()).or_insert_with(|| def.id.clone());
            continue;
        }

        match canonical.get(&def.key) {
            Some(existing_id) => {
                // This gradient is a duplicate — map it to the first occurrence
                renames.insert(def.id.clone(), existing_id.clone());
            }
            None => {
                canonical.insert(def.key.clone(), def.id.clone());
            }
        }
    }

    renames
}

//  Inheritance Resolution─

fn resolve_inheritance(defs: &[GradientDef]) -> Vec<GradientDef> {
    // Build lookup by id
    let by_id: HashMap<&str, &GradientDef> =
        defs.iter().map(|d| (d.id.as_str(), d)).collect();

    defs.iter()
        .map(|def| {
            if def.inherits.is_none() || !def.key.stops.is_empty() {
                return def.clone();
            }
            // Walk the inheritance chain to find stops
            let stops = resolve_stops(&def.id, &by_id, 0);
            let mut resolved = def.clone();
            resolved.key.stops = stops;
            resolved
        })
        .collect()
}

fn resolve_stops<'a>(
    id: &str,
    by_id: &HashMap<&str, &'a GradientDef>,
    depth: usize,
) -> Vec<StopKey> {
    if depth > 16 {
        return Vec::new(); // guard against cycles
    }
    let def = match by_id.get(id) {
        Some(d) => d,
        None => return Vec::new(),
    };
    if !def.key.stops.is_empty() {
        return def.key.stops.clone();
    }
    if let Some(ref parent_id) = def.inherits {
        return resolve_stops(parent_id, by_id, depth + 1);
    }
    Vec::new()
}

//  Key Helpers

fn key_has_subst(key: &GradientKey) -> bool {
    key.attrs.iter().any(|(_, v)| v.contains('\x00'))
        || key.stops.iter().any(|s| {
            s.offset.contains('\x00')
                || s.color.contains('\x00')
                || s.opacity.contains('\x00')
        })
}

//  Extraction from raw text (called from optimizer)─

/// Build a `GradientKey` from raw attribute pairs and child stop data.
pub fn make_gradient_key(
    tag: &str,
    raw_attrs: &[(String, String)],
    stops: Vec<StopKey>,
) -> GradientKey {
    let mut attrs: Vec<(String, String)> = raw_attrs
        .iter()
        .filter(|(k, _)| GRADIENT_DEFINE_ATTRS.contains(&k.as_str()))
        .cloned()
        .collect();
    attrs.sort_by(|a, b| a.0.cmp(&b.0));

    GradientKey {
        tag: tag.to_string(),
        attrs,
        stops,
    }
}

/// Build a `StopKey` from raw stop attributes.
pub fn make_stop_key(raw_attrs: &[(String, String)]) -> StopKey {
    let get = |name: &str| -> String {
        raw_attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };

    // stop-color and stop-opacity may live in style="" or as direct attrs
    let style_map = parse_inline_style(&get("style"));
    let color = style_map
        .get("stop-color")
        .cloned()
        .unwrap_or_else(|| get("stop-color"));
    let opacity = style_map
        .get("stop-opacity")
        .cloned()
        .unwrap_or_else(|| get("stop-opacity"));

    StopKey {
        offset: get("offset"),
        color,
        opacity,
    }
}

fn parse_inline_style(style: &str) -> HashMap<String, String> {
    style
        .split(';')
        .filter_map(|decl| {
            let mut parts = decl.splitn(2, ':');
            let k = parts.next()?.trim().to_lowercase();
            let v = parts.next()?.trim().to_string();
            if k.is_empty() { None } else { Some((k, v)) }
        })
        .collect()
}
