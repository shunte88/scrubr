/// `--create-groups` implementation.
///
/// Groups runs of sibling elements that share an identical set of
/// presentation attributes into a `<g>` wrapper, removing those
/// shared attributes from each child and placing them on the `<g>`.
///
/// Rules (matching Python scour behaviour):
///   - Only considers runs of **two or more** consecutive element siblings.
///   - Only groups sibling elements whose tag name is NOT `<g>`, `<defs>`,
///     `<symbol>`, `<marker>`, `<clipPath>`, `<mask>`, `<pattern>`,
///     or `<switch>` (structural/container elements).
///   - Only considers presentation attributes for grouping; layout,
///     geometry, and `id` attributes are left on individual elements.
///   - Substitution variable placeholders in attribute values make that
///     attribute ineligible for grouping (the value may differ at runtime).
///   - The resulting `<g>` carries the shared attributes; children carry
///     only what is unique to them.

use crate::css::PRESENTATION_ATTRS;

const SUBST_MARKER: &str = "\x00SUBST";

/// Attributes that can be lifted onto a `<g>` wrapper.
/// Must be presentation attributes AND safe to inherit.
/// Explicitly excluded: `id`, `class`, geometry (`x`,`y`,`d`,`points`,…).
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
    "clip-rule",
    "paint-order",
    "stop-color", "stop-opacity",
];

/// Non-groupable structural element tags
const STRUCTURAL: &[&str] = &[
    "g", "defs", "symbol", "marker", "clipPath",
    "mask", "pattern", "switch", "svg", "use",
];

//  Public types

/// A serialized element produced by the serialization pass, represented as
/// a text fragment.  We operate on text fragments because the optimizer
/// builds output as a String; group creation inserts wrapper tags around runs.
///
/// The optimizer calls `group_runs()` on the flat list of serialized child
/// fragments *before* they are written to the parent's output buffer.
#[derive(Debug, Clone)]
pub struct ElementFragment {
    /// The complete serialized text for this element, including its children.
    pub text: String,
    /// The tag name (e.g. "rect", "path").
    pub tag: String,
    /// Sorted presentation (attr, value) pairs eligible for grouping.
    pub groupable_attrs: Vec<(String, String)>,
}

impl ElementFragment {
    pub fn new(tag: &str, text: String, all_attrs: &[(String, String)]) -> Self {
        let groupable_attrs = extract_groupable(all_attrs);
        Self {
            text,
            tag: tag.to_string(),
            groupable_attrs,
        }
    }
}

/// Given a list of serialized sibling element fragments, find runs that share
/// groupable attributes and wrap each run in a `<g ...>` element.
/// Returns the rewritten output as a single String.
pub fn group_runs(
    fragments: &[ElementFragment],
    indent_str: &str,
    depth: usize,
    no_line_breaks: bool,
) -> String {
    if fragments.len() < 2 {
        return fragments.iter().map(|f| f.text.clone()).collect();
    }

    let nl = if no_line_breaks { "" } else { "\n" };
    let indent = indent_str.repeat(depth);
    let child_indent = indent_str.repeat(depth + 1);

    let mut out = String::new();
    let mut i = 0;

    while i < fragments.len() {
        // Find the longest run starting at i with shared attrs >= 1
        let (run_end, shared) = find_run(fragments, i);
        let run_len = run_end - i;

        if run_len >= 2 && !shared.is_empty() {
            // Emit a wrapping <g> with shared attrs
            out.push_str(&indent);
            out.push_str("<g");
            for (k, v) in &shared {
                out.push_str(&format!(" {}=\"{}\"", k, escape_attr(v)));
            }
            out.push('>');
            out.push_str(nl);

            for frag in &fragments[i..run_end] {
                // Strip the shared attrs from each child's text
                let stripped = strip_attrs_from_text(&frag.text, &shared);
                out.push_str(&child_indent);
                out.push_str(stripped.trim_start());
                out.push_str(nl);
            }

            out.push_str(&indent);
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

//  Run Detection─

/// Returns `(exclusive_end_index, shared_attrs)` for the longest run of
/// groupable siblings starting at `start`.
fn find_run(
    fragments: &[ElementFragment],
    start: usize,
) -> (usize, Vec<(String, String)>) {
    if start >= fragments.len() || is_structural(&fragments[start].tag) {
        return (start + 1, Vec::new());
    }

    // Seed with the attributes of the first element
    let mut shared: Vec<(String, String)> = fragments[start].groupable_attrs.clone();
    if shared.is_empty() {
        return (start + 1, Vec::new());
    }

    let mut end = start + 1;
    while end < fragments.len() {
        let next = &fragments[end];
        if is_structural(&next.tag) {
            break;
        }
        // Intersect shared with next element's groupable attrs
        let next_set: std::collections::HashMap<&str, &str> = next
            .groupable_attrs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        shared.retain(|(k, v)| next_set.get(k.as_str()).map(|nv| *nv == v.as_str()).unwrap_or(false));

        if shared.is_empty() {
            break;
        }
        end += 1;
    }

    (end, shared)
}

fn is_structural(tag: &str) -> bool {
    STRUCTURAL.contains(&tag)
}

//  Attribute Extraction

fn extract_groupable(attrs: &[(String, String)]) -> Vec<(String, String)> {
    attrs
        .iter()
        .filter(|(k, v)| {
            GROUPABLE.contains(&k.as_str())
                && !v.contains(SUBST_MARKER)
                && !v.contains("{{")
                && PRESENTATION_ATTRS.contains(&k.as_str())
        })
        .cloned()
        .collect()
}

//  Text-Level Attribute Stripping─

/// Remove specific `key="value"` pairs from a serialized XML element opening
/// tag.  This is necessarily text-based since we are working with the already-
/// serialized output string.  Conservative: if an attribute cannot be reliably
/// stripped, the original text is returned unchanged.
fn strip_attrs_from_text(text: &str, attrs: &[(String, String)]) -> String {
    let mut result = text.to_string();
    for (k, v) in attrs {
        // Match ` key="value"` or ` key='value'`
        let double = format!(" {}=\"{}\"", k, escape_attr(v));
        let single = format!(" {}='{}'", k, escape_attr(v));
        if result.contains(&double) {
            result = result.replacen(&double, "", 1);
        } else if result.contains(&single) {
            result = result.replacen(&single, "", 1);
        }
    }
    result
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
