/// Optimizer for SVG `<style>` element content.
///
/// Handles color simplification, default removal, ID remapping, and whitespace
/// normalisation. Values containing substitution variable placeholders are
/// detected via the `subst::value_has_subst` guard and left untouched.

use std::collections::HashMap;
use crate::color::simplify_color;
use crate::css::is_default_value;
use crate::subst::CapturedVar;

// ─── Public API ───────────────────────────────────────────────────────────────

pub fn optimize_style_block(
    css: &str,
    id_map: &HashMap<String, Option<String>>,
    simplify_colors: bool,
    vars: &[CapturedVar],
) -> String {
    let tokens = tokenize_css(css);
    let mut out = String::with_capacity(css.len());

    for tok in &tokens {
        match tok {
            CssToken::AtRule(kw, block) => {
                out.push_str(&rewrite_at_rule(kw, block, id_map, simplify_colors, vars));
            }
            CssToken::Rule(selector, decls) => {
                let new_sel = rewrite_selector(selector, id_map, vars);
                let new_decls = optimize_declarations(decls, simplify_colors, vars);
                if !new_decls.trim().is_empty() {
                    out.push_str(&new_sel);
                    out.push('{');
                    out.push_str(&new_decls);
                    out.push('}');
                }
            }
            CssToken::Comment(c) => out.push_str(c),
            CssToken::Raw(r)     => out.push_str(r),
        }
    }
    out
}

// ─── Tokeniser ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum CssToken {
    Rule(String, String),
    AtRule(String, String),
    Comment(String),
    Raw(String),
}

fn tokenize_css(css: &str) -> Vec<CssToken> {
    let chars: Vec<char> = css.chars().collect();
    let n = chars.len();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < n {
        // Whitespace
        if chars[i].is_whitespace() {
            let start = i;
            while i < n && chars[i].is_whitespace() { i += 1; }
            tokens.push(CssToken::Raw(chars[start..i].iter().collect()));
            continue;
        }
        // Comment
        if i + 1 < n && chars[i] == '/' && chars[i+1] == '*' {
            let start = i; i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i+1] == '/') { i += 1; }
            i += 2;
            tokens.push(CssToken::Comment(chars[start..i].iter().collect()));
            continue;
        }
        // At-rule
        if chars[i] == '@' {
            let (tok, end) = parse_at_rule(&chars, i);
            tokens.push(tok); i = end;
            continue;
        }
        // Selector { declarations }
        let sel_start = i;
        while i < n && chars[i] != '{' { i += 1; }
        if i >= n {
            tokens.push(CssToken::Raw(chars[sel_start..i].iter().collect()));
            break;
        }
        let selector: String = chars[sel_start..i].iter().collect();
        i += 1; // consume '{'
        let decl_start = i;
        let mut depth = 1usize;
        while i < n {
            match chars[i] { '{' => depth += 1, '}' => { depth -= 1; if depth == 0 { break; } } _ => {} }
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
    let mut i = start + 1;
    while i < n && !chars[i].is_whitespace() && chars[i] != '{' && chars[i] != ';' { i += 1; }
    let keyword: String = chars[start+1..i].iter().collect();
    while i < n && chars[i] != '{' && chars[i] != ';' { i += 1; }
    if i >= n { return (CssToken::AtRule(keyword, chars[start..i].iter().collect()), i); }
    if chars[i] == ';' { i += 1; return (CssToken::AtRule(keyword, chars[start..i].iter().collect()), i); }
    i += 1; // '{'
    let block_start = i;
    let mut depth = 1usize;
    while i < n {
        match chars[i] { '{' => depth += 1, '}' => { depth -= 1; if depth == 0 { break; } } _ => {} }
        i += 1;
    }
    let block: String = chars[block_start..i].iter().collect();
    i += 1;
    let full: String = chars[start..i].iter().collect();
    (CssToken::AtRule(keyword, full), i)
}

// ─── Rule Processing ──────────────────────────────────────────────────────────

fn rewrite_selector(
    selector: &str,
    id_map: &HashMap<String, Option<String>>,
    vars: &[CapturedVar],
) -> String {
    if id_map.is_empty() || !selector.contains('#') { return selector.to_string(); }
    // Don't rewrite if the selector contains a placeholder
    if crate::subst::value_has_subst(selector, vars) { return selector.to_string(); }

    let chars: Vec<char> = selector.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(selector.len());
    let mut i = 0;
    while i < n {
        if chars[i] == '#' {
            out.push('#'); i += 1;
            let id_start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '_') { i += 1; }
            let id: String = chars[id_start..i].iter().collect();
            match id_map.get(&id) {
                Some(Some(new_id)) => out.push_str(new_id),
                _ => out.push_str(&id),
            }
        } else { out.push(chars[i]); i += 1; }
    }
    out
}

fn optimize_declarations(
    decls: &str,
    simplify_colors: bool,
    vars: &[CapturedVar],
) -> String {
    let parts = split_declarations(decls, vars);
    let mut out: Vec<String> = Vec::new();

    for decl in parts {
        let decl = decl.trim();
        if decl.is_empty() { continue; }
        if let Some(colon) = find_colon_outside_placeholder(decl, vars) {
            let prop  = decl[..colon].trim().to_lowercase();
            let value = decl[colon+1..].trim();

            // Leave values that contain a placeholder untouched
            if crate::subst::value_has_subst(value, vars) {
                out.push(format!("{}:{}", prop, value));
                continue;
            }
            // Remove defaults
            if is_default_value(&prop, value) { continue; }
            // Simplify colors
            let new_value = if simplify_colors && is_color_prop(&prop) {
                simplify_color(value)
            } else {
                value.to_string()
            };
            out.push(format!("{}:{}", prop, new_value));
        } else {
            out.push(decl.to_string()); // unparseable — keep verbatim
        }
    }
    out.join(";")
}

fn split_declarations(s: &str, vars: &[CapturedVar]) -> Vec<String> {
    // Split on `;` but not inside a placeholder token
    let mut parts = Vec::new();
    let mut cur = String::new();

    // Collect all placeholder strings for fast scanning
    let placeholders: Vec<&str> = vars.iter().map(|v| v.placeholder.as_str()).collect();

    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;

    'outer: while i < n {
        // Check if we're at the start of a placeholder
        for ph in &placeholders {
            let ph_chars: Vec<char> = ph.chars().collect();
            if chars[i..].starts_with(&ph_chars) {
                for &c in &ph_chars { cur.push(c); }
                i += ph_chars.len();
                continue 'outer;
            }
        }
        if chars[i] == ';' {
            parts.push(cur.clone());
            cur.clear();
        } else {
            cur.push(chars[i]);
        }
        i += 1;
    }
    if !cur.trim().is_empty() { parts.push(cur); }
    parts
}

fn find_colon_outside_placeholder(s: &str, vars: &[CapturedVar]) -> Option<usize> {
    let placeholders: Vec<&str> = vars.iter().map(|v| v.placeholder.as_str()).collect();
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;

    'outer: while i < n {
        for ph in &placeholders {
            let ph_chars: Vec<char> = ph.chars().collect();
            if chars[i..].starts_with(&ph_chars) {
                i += ph_chars.len();
                continue 'outer;
            }
        }
        if chars[i] == ':' { return Some(i); }
        i += 1;
    }
    None
}

fn is_color_prop(prop: &str) -> bool {
    matches!(prop,
        "fill" | "stroke" | "stop-color" | "flood-color" |
        "lighting-color" | "color" | "background-color"
    )
}

fn rewrite_at_rule(
    keyword: &str,
    block: &str,
    id_map: &HashMap<String, Option<String>>,
    simplify_colors: bool,
    vars: &[CapturedVar],
) -> String {
    match keyword.to_lowercase().as_str() {
        "media" | "supports" => optimize_style_block(block, id_map, simplify_colors, vars),
        "keyframes" | "-webkit-keyframes" | "-moz-keyframes" => {
            rewrite_color_in_block(block, simplify_colors, vars)
        }
        _ => block.to_string(),
    }
}

fn rewrite_color_in_block(block: &str, simplify_colors: bool, vars: &[CapturedVar]) -> String {
    if !simplify_colors { return block.to_string(); }
    block.lines().map(|line| {
        if let Some(c) = line.find(':') {
            let prop = line[..c].trim().to_lowercase();
            if is_color_prop(&prop) {
                let val = line[c+1..].trim();
                if !crate::subst::value_has_subst(val, vars) {
                    return format!("{}:{}", prop, simplify_color(val));
                }
            }
        }
        line.to_string()
    }).collect::<Vec<_>>().join("\n")
}
