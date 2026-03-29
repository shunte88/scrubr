/// Substitution variable protection engine for scrubr.
///
/// # The problem
///
/// SVG template files contain `{{keyword}}`, `{{key-word}}`, or `{{key_word}}`
/// placeholders replaced at render time.  We must:
///
///   1. Pass the SVG through `roxmltree`, which rejects any non-XML characters
///      — so null bytes or other control characters are forbidden as sentinels.
///   2. Prevent every optimizer pass from destroying or corrupting the placeholder.
///
/// # The solution
///
/// **Phase 1 (protect)** — forward-scan the raw text.  For each `{{var}}` token:
///   a. Infer what attribute (or text node) it sits inside by tracking the parser
///      state as we scan forward.
///   b. Choose a *neutral value* that is syntactically valid for that context
///      (e.g. `#000` for a color attribute, `0` for a number, `M0 0` for a path).
///   c. Choose a *sentinel tag* `__SN__` (smallest N such that the tag is absent
///      from the whole document) to guarantee no false-positive matches.
///   d. Write `<neutral><sentinel>VAR<k><sentinel>END` into the protected text.
///      This is pure ASCII alphanumeric + `_`, valid everywhere in XML.
///
/// **Phase 2** — parse and optimize the protected text normally.  The optimizer
/// sees a valid neutral value and leaves it alone (or simplifies it harmlessly).
/// Because the sentinel tag is appended, the full `<neutral><sentinel>…` string
/// is preserved verbatim in the output.
///
/// **Phase 3 (restore)** — string-replace every `<neutral><sentinel>…` back to
/// its original `{{var}}` token.
///
/// # Why forward-scan context inference works
///
/// We maintain a simple state machine as we walk the source text character by
/// character:
///   - `Outside`  — between tags (or before the first tag)
///   - `InTag`    — inside `<tagname ...>`, not yet inside a quoted value
///   - `InValue(attr_name, quote_char)` — inside a quoted attribute value
///   - `InText`   — inside element text content (between `>` and `<`)
///
/// When we reach `{{`, the current state tells us exactly which attribute (if
/// any) we are inside, giving a precise context.

use std::collections::HashMap;

//  Sentinel 

/// Find the smallest N such that `__SN__` is absent from `text`.
fn find_sentinel_n(text: &str) -> usize {
    (0usize..).find(|&n| !text.contains(&format!("__S{}__", n))).unwrap()
}

//  Context 

/// The syntactic location of a `{{var}}` token.
#[derive(Debug, Clone, PartialEq)]
pub enum SubstContext {
    Color,
    Path,
    Transform,
    Number,
    Id,
    StyleValue,
    TextContent,
    Generic,
}

/// Return the neutral XML-safe value for each context.
/// The value must be parseable by every optimizer pass that touches the context.
pub fn neutral_value(ctx: &SubstContext) -> &'static str {
    match ctx {
        SubstContext::Color       => "#000",
        SubstContext::Path        => "M0 0",
        SubstContext::Transform   => "translate(0)",
        SubstContext::Number      => "0",
        SubstContext::Id          => "scrubr-ph",
        SubstContext::StyleValue  => "inherit",
        SubstContext::TextContent => "0",
        SubstContext::Generic     => "0",
    }
}

//  Attribute name → context 

const COLOR_ATTRS: &[&str] = &[
    "fill", "stroke", "stop-color", "flood-color", "lighting-color", "color",
];
const NUMBER_ATTRS: &[&str] = &[
    "x", "y", "x1", "y1", "x2", "y2",
    "cx", "cy", "r", "rx", "ry", "fx", "fy",
    "width", "height", "offset",
    "opacity", "fill-opacity", "stroke-opacity", "stop-opacity", "flood-opacity",
    "stroke-width", "stroke-miterlimit", "stroke-dashoffset",
    "font-size", "letter-spacing", "word-spacing", "kerning",
    "stddeviation", "basefrequency", "k", "k1", "k2", "k3", "k4",
    "amplitude", "exponent", "intercept", "slope",
    "specularconstant", "specularexponent", "diffuseconstant",
    "surfacescale", "seed", "numoctaves",
    "viewbox", "points",
];
const ID_ATTRS: &[&str] = &["id", "href", "xlink:href"];
const TRANSFORM_ATTRS: &[&str] = &["transform", "patterntransform", "gradienttransform"];

/// Infer substitution context from an attribute name.
pub fn context_for_attr(attr_name: &str) -> SubstContext {
    let lower = attr_name.to_ascii_lowercase();
    if COLOR_ATTRS.contains(&lower.as_str()) {
        SubstContext::Color
    } else if lower == "d" {
        SubstContext::Path
    } else if TRANSFORM_ATTRS.contains(&lower.as_str()) {
        SubstContext::Transform
    } else if NUMBER_ATTRS.contains(&lower.as_str()) {
        SubstContext::Number
    } else if ID_ATTRS.contains(&lower.as_str()) {
        SubstContext::Id
    } else if lower == "style" {
        SubstContext::StyleValue
    } else {
        SubstContext::Generic
    }
}

/// Infer context for a CSS property name (inside `style=""` or `<style>`).
pub fn context_for_css_prop(prop: &str) -> SubstContext {
    let lower = prop.to_ascii_lowercase();
    if COLOR_ATTRS.contains(&lower.as_str()) {
        SubstContext::Color
    } else if lower.ends_with("opacity") || lower.ends_with("width") || lower.ends_with("size") {
        SubstContext::Number
    } else {
        SubstContext::StyleValue
    }
}

//  Captured variable 

/// One captured `{{...}}` token and its replacement data.
#[derive(Debug, Clone)]
pub struct CapturedVar {
    /// Original token, e.g. `{{fill-color}}`
    pub original: String,
    /// Unique placeholder string, e.g. `__S3__VAR0__S3__END`
    pub placeholder: String,
    /// Context-appropriate neutral value, e.g. `#000`
    pub neutral: String,
}

//  Forward-scan parser state 

#[derive(Debug, Clone)]
enum ScanState {
    /// Between tags, or before the document starts.
    Outside,
    /// Inside `<tagname`, reading the tag name.
    InTagName,
    /// Inside `<tagname ...>`, between attributes (or after tag name).
    InTag,
    /// Inside an attribute name: `<tag attrname...`
    InAttrName(String),
    /// After `attrname=`, before the opening quote.
    AfterEquals(String),
    /// Inside a quoted attribute value.
    InValue { attr: String, quote: char },
    /// Inside element text content.
    InText,
    /// Inside `<!-- ... -->`
    InComment,
    /// Inside `<![CDATA[ ... ]]>`
    InCdata,
}

//  Main protection pass 

/// Replace every `{{var}}` in `input` with a context-aware XML-safe placeholder.
/// Returns `(protected_svg, captured_vars)`.
pub fn protect_subst_vars(input: &str) -> (String, Vec<CapturedVar>) {
    let sentinel_n = find_sentinel_n(input);
    let sentinel   = format!("__S{}__", sentinel_n);

    let mut vars: Vec<CapturedVar> = Vec::new();
    let mut out  = String::with_capacity(input.len() + 64);

    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut state = ScanState::Outside;

    while i < n {
        // Detect `{{var}}` 
        if i + 1 < n && chars[i] == '{' && chars[i + 1] == '{' {
            // Scan forward for matching `}}`
            let var_start = i;
            let mut j = i + 2;
            let mut found_close = false;
            while j + 1 < n {
                if chars[j] == '}' && chars[j + 1] == '}' {
                    found_close = true;
                    j += 2;
                    break;
                }
                // Inner content may not span a quote or `>`
                if chars[j] == '<' || chars[j] == '>' { break; }
                j += 1;
            }

            if found_close {
                let inner: String = chars[var_start + 2..j - 2].iter().collect();
                if is_valid_inner(&inner) {
                    let original: String = chars[var_start..j].iter().collect();
                    let k = vars.len();

                    // Context from current parser state
                    let ctx = context_from_state(&state);
                    let neutral = neutral_value(&ctx).to_string();

                    // Placeholder: `<neutral><sentinel>VAR<k><sentinel>END`
                    // — only alphanumeric + `_`, valid everywhere in XML.
                    let placeholder = format!("{}VAR{}{}", sentinel, k, sentinel);
                    let protected   = format!("{}{}", neutral, placeholder);

                    vars.push(CapturedVar { original, placeholder, neutral });
                    out.push_str(&protected);
                    i = j;
                    continue;
                }
            }
        }

        // Advance parser state 
        let ch = chars[i];
        advance_state(&mut state, &chars, &mut i, ch, &mut out);
    }

    (out, vars)
}

/// Determine the substitution context from the current parser state.
fn context_from_state(state: &ScanState) -> SubstContext {
    match state {
        ScanState::InValue { attr, .. } => context_for_attr(attr),
        ScanState::InText => SubstContext::TextContent,
        _ => SubstContext::Generic,
    }
}

/// Advance the parser state machine by one character, emitting to `out`.
/// `i` is the current index; advance it as needed (default: +1 at end of match).
fn advance_state(
    state: &mut ScanState,
    chars: &[char],
    i: &mut usize,
    ch: char,
    out: &mut String,
) {
    let n = chars.len();

    match state {
        ScanState::Outside | ScanState::InText => {
            if ch == '<' {
                // Check for comment or CDATA
                if *i + 3 < n && chars[*i+1] == '!' && chars[*i+2] == '-' && chars[*i+3] == '-' {
                    *state = ScanState::InComment;
                } else if *i + 8 < n
                    && chars[*i+1] == '!'
                    && chars[*i+2..=*i+8].iter().collect::<String>() == "[CDATA["
                {
                    *state = ScanState::InCdata;
                } else {
                    *state = ScanState::InTagName;
                }
            } else if ch == '>' {
                *state = ScanState::InText;
            }
            out.push(ch);
            *i += 1;
        }

        ScanState::InComment => {
            // Scan until `-->`
            if ch == '-' && *i + 2 < n && chars[*i+1] == '-' && chars[*i+2] == '>' {
                out.push('-'); out.push('-'); out.push('>');
                *i += 3;
                *state = ScanState::Outside;
            } else {
                out.push(ch);
                *i += 1;
            }
        }

        ScanState::InCdata => {
            // Scan until `]]>`
            if ch == ']' && *i + 2 < n && chars[*i+1] == ']' && chars[*i+2] == '>' {
                out.push(']'); out.push(']'); out.push('>');
                *i += 3;
                *state = ScanState::InText;
            } else {
                out.push(ch);
                *i += 1;
            }
        }

        ScanState::InTagName => {
            if ch == '>' {
                *state = ScanState::InText;
                out.push(ch);
            } else if ch == '/' && *i + 1 < n && chars[*i+1] == '>' {
                out.push('/'); out.push('>');
                *i += 2;
                *state = ScanState::Outside;
                return;
            } else if ch.is_whitespace() {
                *state = ScanState::InTag;
                out.push(ch);
            } else {
                // Still reading tag name
                out.push(ch);
            }
            *i += 1;
        }

        ScanState::InTag => {
            if ch == '>' {
                *state = ScanState::InText;
                out.push(ch);
                *i += 1;
            } else if ch == '/' && *i + 1 < n && chars[*i+1] == '>' {
                out.push('/'); out.push('>');
                *i += 2;
                *state = ScanState::Outside;
            } else if ch.is_whitespace() || ch == ',' {
                out.push(ch);
                *i += 1;
            } else {
                // Start of attribute name
                *state = ScanState::InAttrName(ch.to_string());
                out.push(ch);
                *i += 1;
            }
        }

        ScanState::InAttrName(name) => {
            if ch == '=' {
                let attr = name.clone();
                *state = ScanState::AfterEquals(attr);
                out.push(ch);
                *i += 1;
            } else if ch == '>' {
                *state = ScanState::InText;
                out.push(ch);
                *i += 1;
            } else if ch.is_whitespace() {
                // Boolean attribute (no value); back to InTag
                *state = ScanState::InTag;
                out.push(ch);
                *i += 1;
            } else {
                name.push(ch);
                out.push(ch);
                *i += 1;
            }
        }

        ScanState::AfterEquals(attr) => {
            if ch == '"' || ch == '\'' {
                let attr_name = attr.clone();
                *state = ScanState::InValue { attr: attr_name, quote: ch };
                out.push(ch);
                *i += 1;
            } else {
                // Unquoted attribute (unusual in SVG but handle gracefully)
                out.push(ch);
                *i += 1;
            }
        }

        ScanState::InValue { attr: _, quote } => {
            let q = *quote;
            if ch == q {
                // Closing quote — back to InTag
                *state = ScanState::InTag;
                out.push(ch);
                *i += 1;
            } else {
                out.push(ch);
                *i += 1;
            }
        }
    }
}

fn is_valid_inner(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

//  Restore 

/// Replace every `neutral+placeholder` string back to its original `{{token}}`.
pub fn restore_subst_vars(output: &str, vars: &[CapturedVar]) -> String {
    if vars.is_empty() { return output.to_string(); }
    let mut result = output.to_string();
    // Process in reverse so placeholder 10 is not a prefix of 1 (already unique,
    // but reverse order is an extra safety net).
    for cv in vars.iter().rev() {
        let key = format!("{}{}", cv.neutral, cv.placeholder);
        result = result.replace(&key, &cv.original);
    }
    result
}

//  Guard helper 

/// Return true if `val` contains any placeholder.  Used by optimizer passes to
/// skip further transformation on values that encode a captured variable.
pub fn value_has_subst(val: &str, vars: &[CapturedVar]) -> bool {
    if vars.is_empty() { return false; }
    vars.iter().any(|cv| val.contains(&cv.placeholder))
}

//  Tests 

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(input: &str) -> String {
        let (protected, vars) = protect_subst_vars(input);
        // Verify the protected string contains no {{ }}
        assert!(!protected.contains("{{"), "protected still contains {{");
        // Verify it contains no null bytes
        assert!(!protected.contains('\x00'), "protected contains null byte");
        restore_subst_vars(&protected, &vars)
    }

    #[test]
    fn test_color_attr() {
        let svg = r#"<rect fill="{{primary-color}}" />"#;
        assert_eq!(round_trip(svg), svg);
    }

    #[test]
    fn test_path_attr() {
        let svg = r#"<path d="{{path-data}}" />"#;
        assert_eq!(round_trip(svg), svg);
    }

    #[test]
    fn test_number_attr() {
        let svg = r#"<circle cx="{{cx}}" cy="10" />"#;
        assert_eq!(round_trip(svg), svg);
    }

    #[test]
    fn test_text_content() {
        let svg = r#"<text>{{label}}</text>"#;
        assert_eq!(round_trip(svg), svg);
    }

    #[test]
    fn test_multiple_vars() {
        let svg = r#"<rect fill="{{bg}}" stroke="{{border}}" width="{{w}}" />"#;
        assert_eq!(round_trip(svg), svg);
    }

    #[test]
    fn test_no_vars() {
        let svg = r#"<rect fill="red" />"#;
        assert_eq!(round_trip(svg), svg);
    }

    #[test]
    fn test_style_attr() {
        let svg = r#"<rect style="fill:{{color}};opacity:{{opacity}}" />"#;
        assert_eq!(round_trip(svg), svg);
    }

    #[test]
    fn test_sentinel_collision_avoidance() {
        // Document already contains __S0__ — should use __S1__
        let svg = r#"<rect id="__S0__test" fill="{{color}}" />"#;
        let (protected, vars) = protect_subst_vars(svg);
        assert!(!protected.contains("{{"));
        // __S1__ should be used
        assert!(protected.contains("__S1__"));
        assert_eq!(restore_subst_vars(&protected, &vars), svg);
    }

    #[test]
    fn test_protected_is_valid_xml() {
        let svg = r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"><rect fill="{{color}}" width="{{w}}" height="{{h}}"/></svg>"#;
        let (protected, vars) = protect_subst_vars(svg);
        // Must parse without error
        roxmltree::Document::parse(&protected).expect("protected SVG must be valid XML");
        assert_eq!(restore_subst_vars(&protected, &vars), svg);
    }

    #[test]
    fn test_invalid_inner_not_replaced() {
        // Spaces inside braces are not valid — must pass through unchanged
        let svg = r#"<rect fill="{{ not a var }}" />"#;
        let (protected, vars) = protect_subst_vars(svg);
        assert!(vars.is_empty());
        assert_eq!(protected, svg);
    }
}
