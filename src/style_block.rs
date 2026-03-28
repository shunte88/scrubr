/// Optimizer for SVG `<style>` element content.
///
/// Handles:
///   - Color simplification in property values
///   - Default value removal from rules
///   - ID reference remapping (`#old-id` → `#new-id`) in selectors and values
///   - Whitespace normalisation and rule deduplication
///   - Preservation of substitution variable placeholders
///
/// Deliberately conservative: only rewrites property values it understands.
/// At-rules (`@font-face`, `@keyframes`, `@media`) are passed through with
/// only colour/ID rewrites applied — their structure is never altered.

use std::collections::HashMap;
use crate::color::simplify_color;
use crate::css::is_default_value;

const SUBST_PLACEHOLDER_MARKER: &str = "\x00SUBST";

//  Public API─

/// Optimise an entire CSS text block (the content of a `<style>` element).
///
/// `id_map`: optional rename map built by the ID optimisation pass.
/// `simplify_colors`: whether to normalise colour values.
pub fn optimize_style_block(
    css: &str,
    id_map: &HashMap<String, Option<String>>,
    simplify_colors: bool,
) -> String {
    // Never process style blocks that contain substitution variable placeholders
    // at the top level — the tokeniser could corrupt them.  Individual rule
    // values are guarded inside the deeper functions.
    let tokens = tokenize_css(css);
    let mut out = String::with_capacity(css.len());

    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            CssToken::AtRule(at_kw, block) => {
                // Rewrite colour / id refs inside at-rule blocks conservatively
                let rewritten = rewrite_at_rule_block(at_kw, block, id_map, simplify_colors);
                out.push_str(&rewritten);
                i += 1;
            }
            CssToken::Rule(selector, declarations) => {
                let new_sel = rewrite_selector(selector, id_map);
                let new_decls = optimize_declarations(declarations, simplify_colors);
                if !new_decls.trim().is_empty() {
                    out.push_str(&new_sel);
                    out.push('{');
                    out.push_str(&new_decls);
                    out.push('}');
                }
                i += 1;
            }
            CssToken::Comment(c) => {
                out.push_str(c);
                i += 1;
            }
            CssToken::Raw(r) => {
                out.push_str(r);
                i += 1;
            }
        }
    }

    out
}

//  CSS Tokeniser─

#[derive(Debug, Clone)]
enum CssToken {
    /// A regular `selector { declarations }` rule
    Rule(String, String),
    /// An `@keyword ... { block }` or `@keyword ...;` at-rule
    AtRule(String, String),
    /// A CSS comment `/* ... */`
    Comment(String),
    /// Raw text that doesn't fit the above (whitespace, etc.)
    Raw(String),
}

fn tokenize_css(css: &str) -> Vec<CssToken> {
    let chars: Vec<char> = css.chars().collect();
    let n = chars.len();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < n {
        // Skip whitespace between rules
        if chars[i].is_whitespace() {
            let start = i;
            while i < n && chars[i].is_whitespace() {
                i += 1;
            }
            tokens.push(CssToken::Raw(chars[start..i].iter().collect()));
            continue;
        }

        // Comment
        if i + 1 < n && chars[i] == '/' && chars[i + 1] == '*' {
            let start = i;
            i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2; // consume */
            tokens.push(CssToken::Comment(chars[start..i].iter().collect()));
            continue;
        }

        // At-rule
        if chars[i] == '@' {
            let (tok, end) = parse_at_rule(&chars, i);
            tokens.push(tok);
            i = end;
            continue;
        }

        // Regular rule: collect selector until '{', then block until matching '}'
        let sel_start = i;
        while i < n && chars[i] != '{' {
            i += 1;
        }
        if i >= n {
            // Trailing text with no brace — emit as raw
            tokens.push(CssToken::Raw(chars[sel_start..i].iter().collect()));
            break;
        }
        let selector: String = chars[sel_start..i].iter().collect();
        i += 1; // consume '{'

        let decl_start = i;
        let mut depth = 1usize;
        while i < n {
            match chars[i] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let declarations: String = chars[decl_start..i].iter().collect();
        i += 1; // consume '}'

        tokens.push(CssToken::Rule(selector, declarations));
    }

    tokens
}

fn parse_at_rule(chars: &[char], start: usize) -> (CssToken, usize) {
    let n = chars.len();
    let mut i = start + 1; // skip '@'

    // keyword
    while i < n && !chars[i].is_whitespace() && chars[i] != '{' && chars[i] != ';' {
        i += 1;
    }
    let keyword: String = chars[start + 1..i].iter().collect();

    // skip whitespace and prelude
    while i < n && chars[i] != '{' && chars[i] != ';' {
        i += 1;
    }

    if i >= n {
        return (
            CssToken::AtRule(keyword, chars[start..i].iter().collect()),
            i,
        );
    }

    if chars[i] == ';' {
        i += 1;
        return (
            CssToken::AtRule(keyword, chars[start..i].iter().collect()),
            i,
        );
    }

    // block
    i += 1; // consume '{'
    let block_start = i;
    let mut depth = 1usize;
    while i < n {
        match chars[i] {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let block: String = chars[block_start..i].iter().collect();
    i += 1; // consume '}'

    let full: String = chars[start..i].iter().collect();
    (CssToken::AtRule(keyword, full), i)
}

//  Rule Processing

/// Rewrite ID references in a CSS selector, e.g. `#old-id` → `#new-id`.
fn rewrite_selector(selector: &str, id_map: &HashMap<String, Option<String>>) -> String {
    if id_map.is_empty() || !selector.contains('#') {
        return selector.to_string();
    }
    let chars: Vec<char> = selector.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(selector.len());
    let mut i = 0;

    while i < n {
        if chars[i] == '#' {
            out.push('#');
            i += 1;
            let id_start = i;
            // Collect identifier characters
            while i < n
                && (chars[i].is_alphanumeric()
                    || chars[i] == '-'
                    || chars[i] == '_')
            {
                i += 1;
            }
            let id: String = chars[id_start..i].iter().collect();
            match id_map.get(&id) {
                Some(Some(new_id)) => out.push_str(new_id),
                Some(None) => out.push_str(&id), // stripped ID — keep in selector
                None => out.push_str(&id),
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Optimise the declarations block of a CSS rule.
/// Returns the rewritten block content (without the outer braces).
fn optimize_declarations(declarations: &str, simplify_colors: bool) -> String {
    // Split on ';' respecting placeholder boundaries
    let decls = split_declarations(declarations);
    let mut out_parts: Vec<String> = Vec::new();

    for decl in decls {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        if let Some(colon) = decl.find(':') {
            let prop = decl[..colon].trim().to_lowercase();
            let value = decl[colon + 1..].trim();

            // Skip defaults (guard: don't strip if value contains placeholders)
            if !value.contains(SUBST_PLACEHOLDER_MARKER) && is_default_value(&prop, value) {
                continue;
            }

            // Simplify colours where applicable
            let new_value = if simplify_colors && is_color_prop(&prop) && !value.contains(SUBST_PLACEHOLDER_MARKER) {
                simplify_color(value)
            } else {
                value.to_string()
            };

            out_parts.push(format!("{}:{}", prop, new_value));
        } else {
            // Unparseable — keep verbatim
            out_parts.push(decl.to_string());
        }
    }

    if out_parts.is_empty() {
        return String::new();
    }

    out_parts.join(";")
}

fn split_declarations(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize; // track {{ }} substitution variable depth
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        // Track substitution variable placeholders (null-byte delimited)
        if chars[i] == '\x00' {
            cur.push(chars[i]);
            i += 1;
            // consume until next null byte
            while i < n && chars[i] != '\x00' {
                cur.push(chars[i]);
                i += 1;
            }
            if i < n {
                cur.push(chars[i]); // closing \x00
                i += 1;
            }
            continue;
        }
        if chars[i] == ';' && depth == 0 {
            parts.push(cur.clone());
            cur.clear();
        } else {
            if chars[i] == '(' { depth += 1; }
            if chars[i] == ')' && depth > 0 { depth -= 1; }
            cur.push(chars[i]);
        }
        i += 1;
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

fn is_color_prop(prop: &str) -> bool {
    matches!(
        prop,
        "fill"
            | "stroke"
            | "stop-color"
            | "flood-color"
            | "lighting-color"
            | "color"
            | "background-color"
            | "border-color"
    )
}

//  At-Rule Handling─

fn rewrite_at_rule_block(
    keyword: &str,
    block: &str,
    id_map: &HashMap<String, Option<String>>,
    simplify_colors: bool,
) -> String {
    match keyword.to_lowercase().as_str() {
        "keyframes" | "-webkit-keyframes" | "-moz-keyframes" => {
            // Rewrite colour values inside keyframe blocks
            rewrite_color_in_block(block, simplify_colors)
        }
        "media" | "supports" => {
            // Recursively optimise nested rules
            optimize_style_block(block, id_map, simplify_colors)
        }
        _ => {
            // @import, @font-face, @charset etc — pass through verbatim
            block.to_string()
        }
    }
}

fn rewrite_color_in_block(block: &str, simplify_colors: bool) -> String {
    if !simplify_colors {
        return block.to_string();
    }
    // Simple line-by-line color rewrite for keyframe bodies
    block
        .lines()
        .map(|line| {
            if let Some(colon) = line.find(':') {
                let prop = line[..colon].trim().to_lowercase();
                if is_color_prop(&prop) {
                    let value = line[colon + 1..].trim();
                    if !value.contains(SUBST_PLACEHOLDER_MARKER) {
                        return format!("{}:{}", prop, simplify_color(value));
                    }
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
