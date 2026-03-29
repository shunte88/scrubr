/// Gradient deduplication for `<defs>`.
///
/// Scans all gradient/pattern elements in `<defs>`, canonicalises them, and
/// returns a rename map: duplicate_id → canonical_id.
///
/// A gradient whose key contains any placeholder from the substitution-variable
/// system is always treated as unique — its runtime values may differ.

use std::collections::HashMap;
use crate::subst::CapturedVar;

//  Public Types 

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GradientKey {
    pub tag: String,
    /// Sorted (name, value) pairs for the defining attributes (not id/href)
    pub attrs: Vec<(String, String)>,
    /// Sorted child stop descriptors
    pub stops: Vec<StopKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StopKey {
    pub offset: String,
    pub color: String,
    pub opacity: String,
}

#[derive(Debug, Clone)]
pub struct GradientDef {
    pub id: String,
    pub key: GradientKey,
    pub inherits: Option<String>,
}

/// Attributes that define the visual appearance of a gradient/pattern.
const GRADIENT_DEFINE_ATTRS: &[&str] = &[
    "cx", "cy", "r", "fx", "fy", "fr",
    "x1", "y1", "x2", "y2",
    "gradientUnits", "gradientTransform", "spreadMethod",
    "patternUnits", "patternTransform", "patternContentUnits",
    "x", "y", "width", "height",
    "viewBox", "preserveAspectRatio",
];

//  Public API 

/// Given gradient definitions extracted from `<defs>`, return a map of
/// `duplicate_id → canonical_id` for every gradient that is a duplicate.
pub fn find_duplicate_gradients(
    defs: &[GradientDef],
    vars: &[CapturedVar],
) -> HashMap<String, String> {
    let resolved = resolve_inheritance(defs);

    let mut canonical: HashMap<GradientKey, String> = HashMap::new();
    let mut renames: HashMap<String, String> = HashMap::new();

    for def in &resolved {
        if key_has_placeholder(&def.key, vars) {
            // Unique — just register it
            canonical.entry(def.key.clone()).or_insert_with(|| def.id.clone());
            continue;
        }
        match canonical.get(&def.key) {
            Some(existing_id) => {
                renames.insert(def.id.clone(), existing_id.clone());
            }
            None => {
                canonical.insert(def.key.clone(), def.id.clone());
            }
        }
    }

    renames
}

//  Key Construction 

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
    GradientKey { tag: tag.to_string(), attrs, stops }
}

pub fn make_stop_key(raw_attrs: &[(String, String)]) -> StopKey {
    let get = |name: &str| -> String {
        raw_attrs.iter().find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let style_map = parse_inline_style(&get("style"));
    let color = style_map.get("stop-color").cloned()
        .unwrap_or_else(|| get("stop-color"));
    let opacity = style_map.get("stop-opacity").cloned()
        .unwrap_or_else(|| get("stop-opacity"));
    StopKey { offset: get("offset"), color, opacity }
}

//  Inheritance Resolution 

fn resolve_inheritance(defs: &[GradientDef]) -> Vec<GradientDef> {
    let by_id: HashMap<&str, &GradientDef> =
        defs.iter().map(|d| (d.id.as_str(), d)).collect();
    defs.iter().map(|def| {
        if def.inherits.is_none() || !def.key.stops.is_empty() {
            return def.clone();
        }
        let stops = resolve_stops(&def.id, &by_id, 0);
        let mut resolved = def.clone();
        resolved.key.stops = stops;
        resolved
    }).collect()
}

fn resolve_stops<'a>(
    id: &str,
    by_id: &HashMap<&str, &'a GradientDef>,
    depth: usize,
) -> Vec<StopKey> {
    if depth > 16 { return Vec::new(); }
    let def = match by_id.get(id) { Some(d) => d, None => return Vec::new() };
    if !def.key.stops.is_empty() { return def.key.stops.clone(); }
    if let Some(parent_id) = &def.inherits {
        return resolve_stops(parent_id, by_id, depth + 1);
    }
    Vec::new()
}

//  Helpers 

fn key_has_placeholder(key: &GradientKey, vars: &[CapturedVar]) -> bool {
    if vars.is_empty() { return false; }
    key.attrs.iter().any(|(_, v)| crate::subst::value_has_subst(v, vars))
        || key.stops.iter().any(|s| {
            crate::subst::value_has_subst(&s.offset, vars)
                || crate::subst::value_has_subst(&s.color, vars)
                || crate::subst::value_has_subst(&s.opacity, vars)
        })
}

fn parse_inline_style(style: &str) -> HashMap<String, String> {
    style.split(';').filter_map(|decl| {
        let mut parts = decl.splitn(2, ':');
        let k = parts.next()?.trim().to_lowercase();
        let v = parts.next()?.trim().to_string();
        if k.is_empty() { None } else { Some((k, v)) }
    }).collect()
}
