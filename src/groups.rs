/// --create-groups: group sibling elements with identical presentation attributes.

use crate::css::PRESENTATION_ATTRS;
use crate::subst::CapturedVar;

const GROUPABLE: &[&str] = &[
    "fill", "fill-opacity", "fill-rule",
    "stroke", "stroke-dasharray", "stroke-dashoffset",
    "stroke-linecap", "stroke-linejoin", "stroke-miterlimit",
    "stroke-opacity", "stroke-width",
    "opacity",
    "color", "color-interpolation", "color-rendering",
    "cursor", "direction", "display",
    "font-family", "font-size", "font-size-adjust",
    "font-stretch", "font-style", "font-variant", "font-weight",
    "letter-spacing", "word-spacing",
    "text-anchor", "text-decoration", "text-rendering",
    "visibility", "pointer-events",
    "shape-rendering", "image-rendering",
    "clip-rule", "paint-order",
];

const STRUCTURAL: &[&str] = &[
    "g", "defs", "symbol", "marker", "clipPath",
    "mask", "pattern", "switch", "svg", "use",
];

//  Public Types 

#[derive(Debug, Clone)]
pub struct ElementFragment {
    pub text: String,
    pub tag: String,
    pub groupable_attrs: Vec<(String, String)>,
}

impl ElementFragment {
    pub fn new(
        tag: &str,
        text: String,
        all_attrs: &[(String, String)],
        vars: &[CapturedVar],
    ) -> Self {
        let groupable_attrs = extract_groupable(all_attrs, vars);
        Self { text, tag: tag.to_string(), groupable_attrs }
    }
}

pub fn group_runs(
    fragments: &[ElementFragment],
    indent_unit: &str,
    depth: usize,
    no_line_breaks: bool,
) -> String {
    if fragments.len() < 2 {
        return fragments.iter().map(|f| f.text.clone()).collect();
    }
    let nl  = if no_line_breaks { "" } else { "\n" };
    let ind = indent_unit.repeat(depth);
    let cind = indent_unit.repeat(depth + 1);

    let mut out = String::new();
    let mut i = 0;

    while i < fragments.len() {
        let (run_end, shared) = find_run(fragments, i);
        if run_end - i >= 2 && !shared.is_empty() {
            out.push_str(&ind);
            out.push_str("<g");
            for (k, v) in &shared {
                out.push_str(&format!(" {}=\"{}\"", k, escape_attr(v)));
            }
            out.push('>');
            out.push_str(nl);
            for frag in &fragments[i..run_end] {
                let stripped = strip_attrs_from_text(&frag.text, &shared);
                out.push_str(&cind);
                out.push_str(stripped.trim_start());
                out.push_str(nl);
            }
            out.push_str(&ind);
            out.push_str("</g>");
            out.push_str(nl);
            i = run_end;
        } else {
            out.push_str(&fragments[i].text);
            i += 1;
        }
    }
    out
}

//  Run Detection 

fn find_run(fragments: &[ElementFragment], start: usize) -> (usize, Vec<(String, String)>) {
    if start >= fragments.len() || is_structural(&fragments[start].tag) {
        return (start + 1, Vec::new());
    }
    let mut shared = fragments[start].groupable_attrs.clone();
    if shared.is_empty() { return (start + 1, Vec::new()); }

    let mut end = start + 1;
    while end < fragments.len() {
        if is_structural(&fragments[end].tag) { break; }
        let next_map: std::collections::HashMap<&str, &str> = fragments[end]
            .groupable_attrs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        shared.retain(|(k, v)| next_map.get(k.as_str()).map(|nv| *nv == v.as_str()).unwrap_or(false));
        if shared.is_empty() { break; }
        end += 1;
    }
    (end, shared)
}

fn is_structural(tag: &str) -> bool { STRUCTURAL.contains(&tag) }

//  Attribute Helpers 

fn extract_groupable(attrs: &[(String, String)], vars: &[CapturedVar]) -> Vec<(String, String)> {
    attrs.iter().filter(|(k, v)| {
        GROUPABLE.contains(&k.as_str())
            && PRESENTATION_ATTRS.contains(&k.as_str())
            && !crate::subst::value_has_subst(v, vars)
    }).cloned().collect()
}

fn strip_attrs_from_text(text: &str, attrs: &[(String, String)]) -> String {
    let mut result = text.to_string();
    for (k, v) in attrs {
        let dq = format!(" {}=\"{}\"", k, escape_attr(v));
        let sq = format!(" {}='{}'", k, escape_attr(v));
        if result.contains(&dq) { result = result.replacen(&dq, "", 1); }
        else if result.contains(&sq) { result = result.replacen(&sq, "", 1); }
    }
    result
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;")
     .replace('>', "&gt;").replace('"', "&quot;")
}
