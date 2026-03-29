/// Path simplification and combination.
///
/// Simplification (luncheon/simplify-svg-path style):
///   - Convert all relative commands to absolute
///   - Expand H/V to L, S to C (with reflected control point), T to Q
///   - Remove zero-length segments
///   - Remove consecutive duplicate M commands
///   - Remove M..Z empty sub-paths
///   - Merge collinear consecutive L commands
///   - Re-serialize compactly
///
/// Combination (picosvg style):
///   - Merge consecutive <path> siblings that share identical presentation
///     attributes by concatenating their `d` values
///   - Paths containing a subst-var placeholder are NEVER combined (their `d`
///     may still be simplified independently since it contains a neutral value)
///
/// Subst-var rule: `value_has_subst(d, vars)` guards combination only.
/// Simplification operates on the neutral value and is harmless.

use crate::subst::{CapturedVar, value_has_subst};

// ─── Public API ───────────────────────────────────────────────────────────────

/// Simplify a path `d` string. Safe on subst-placeholder values — the neutral
/// `M0 0` passes through unchanged.
pub fn simplify_path_d(d: &str, precision: u8) -> String {
    let d = d.trim();
    if d.is_empty() { return d.to_string(); }
    let raw = parse_path_d(d);
    if raw.is_empty() { return d.to_string(); }
    let abs   = to_absolute(&raw);
    let clean = clean_commands(abs);
    serialize_abs(&clean, precision)
}

/// Given two path `d` strings, attempt to combine them into one.
/// Returns `None` if either contains a subst placeholder.
pub fn combine_path_d(d1: &str, d2: &str, vars: &[CapturedVar]) -> Option<String> {
    if value_has_subst(d1, vars) || value_has_subst(d2, vars) {
        return None;
    }
    let d1 = d1.trim();
    let d2 = d2.trim();
    if d1.is_empty() { return Some(d2.to_string()); }
    if d2.is_empty() { return Some(d1.to_string()); }
    Some(format!("{} {}", d1, d2))
}

// ─── Internal command representation (always absolute after conversion) ───────

#[derive(Debug, Clone)]
enum AbsCmd {
    Move(f64, f64),
    Line(f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64), // cp1x cp1y cp2x cp2y x y
    Quad(f64, f64, f64, f64),            // cpx cpy x y
    Arc { rx: f64, ry: f64, x_rot: f64, large: bool, sweep: bool, x: f64, y: f64 },
    Close,
}

// ─── Raw tokenized command (before absolutization) ────────────────────────────

#[derive(Debug, Clone)]
struct RawCmd {
    letter: char,
    args: Vec<f64>,
}

// ─── Parser ───────────────────────────────────────────────────────────────────

fn parse_path_d(d: &str) -> Vec<RawCmd> {
    let chars: Vec<char> = d.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    let mut cur_letter: Option<char> = None;

    while i < n {
        skip_sep(&chars, &mut i);
        if i >= n { break; }
        let c = chars[i];
        if is_cmd(c) {
            cur_letter = Some(c);
            i += 1;
            if c == 'Z' || c == 'z' {
                out.push(RawCmd { letter: c, args: vec![] });
                cur_letter = None;
                continue;
            }
        }
        let letter = match cur_letter {
            Some(l) => l,
            None => { i += 1; continue; }
        };
        let arity = cmd_arity(letter);
        if arity == 0 {
            out.push(RawCmd { letter, args: vec![] });
            cur_letter = None;
            continue;
        }
        let args = read_n_args(&chars, &mut i, letter, arity);
        if args.len() < arity { break; }
        // After M/m, subsequent groups are L/l
        if letter == 'M' || letter == 'm' {
            out.push(RawCmd { letter, args });
            cur_letter = Some(if letter == 'M' { 'L' } else { 'l' });
        } else {
            out.push(RawCmd { letter, args });
        }
    }
    out
}

fn is_cmd(c: char) -> bool { "MmZzLlHhVvCcSsQqTtAa".contains(c) }

fn cmd_arity(c: char) -> usize {
    match c.to_ascii_uppercase() {
        'M' | 'L' | 'T' => 2,
        'H' | 'V' => 1,
        'S' | 'Q' => 4,
        'C' => 6,
        'A' => 7,
        'Z' => 0,
        _ => 0,
    }
}

fn skip_sep(chars: &[char], i: &mut usize) {
    while *i < chars.len() && (chars[*i].is_whitespace() || chars[*i] == ',') {
        *i += 1;
    }
}

fn read_n_args(chars: &[char], i: &mut usize, cmd: char, n: usize) -> Vec<f64> {
    let mut args = Vec::with_capacity(n);
    let is_arc = cmd.to_ascii_uppercase() == 'A';
    for idx in 0..n {
        skip_sep(chars, i);
        if *i >= chars.len() { break; }
        if is_arc && (idx == 3 || idx == 4) {
            // arc flags: single 0/1 digit
            if chars[*i] == '0' || chars[*i] == '1' {
                args.push(if chars[*i] == '1' { 1.0 } else { 0.0 });
                *i += 1;
                continue;
            }
        }
        match read_float(chars, i) {
            Some(v) => args.push(v),
            None => break,
        }
    }
    args
}

fn read_float(chars: &[char], i: &mut usize) -> Option<f64> {
    let n = chars.len();
    let start = *i;
    if *i < n && (chars[*i] == '+' || chars[*i] == '-') { *i += 1; }
    let mut digits = false;
    while *i < n && chars[*i].is_ascii_digit() { digits = true; *i += 1; }
    if *i < n && chars[*i] == '.' {
        *i += 1;
        while *i < n && chars[*i].is_ascii_digit() { digits = true; *i += 1; }
    }
    if !digits { *i = start; return None; }
    if *i < n && (chars[*i] == 'e' || chars[*i] == 'E') {
        let save = *i; *i += 1;
        if *i < n && (chars[*i] == '+' || chars[*i] == '-') { *i += 1; }
        let mut ed = false;
        while *i < n && chars[*i].is_ascii_digit() { ed = true; *i += 1; }
        if !ed { *i = save; }
    }
    chars[start..*i].iter().collect::<String>().parse::<f64>().ok()
}

// ─── Absolutization + expansion ───────────────────────────────────────────────

fn to_absolute(raw: &[RawCmd]) -> Vec<AbsCmd> {
    let mut out = Vec::with_capacity(raw.len());
    let mut cx = 0f64; let mut cy = 0f64; // current point
    let mut mx = 0f64; let mut my = 0f64; // last moveto
    // last control point for S/T reflection
    let mut last_cubic_cp2: Option<(f64, f64)> = None;
    let mut last_quad_cp:   Option<(f64, f64)> = None;

    for cmd in raw {
        let rel = cmd.letter.is_lowercase() && cmd.letter != 'z';
        let upper = cmd.letter.to_ascii_uppercase();
        let a = &cmd.args;

        macro_rules! ax { ($n:expr) => { a[$n] + if rel { cx } else { 0.0 } } }
        macro_rules! ay { ($n:expr) => { a[$n] + if rel { cy } else { 0.0 } } }

        match upper {
            'M' => {
                let (x, y) = (ax!(0), ay!(1));
                mx = x; my = y; cx = x; cy = y;
                out.push(AbsCmd::Move(x, y));
                last_cubic_cp2 = None; last_quad_cp = None;
            }
            'L' => {
                let (x, y) = (ax!(0), ay!(1));
                if !approx_eq(x, cx) || !approx_eq(y, cy) {
                    out.push(AbsCmd::Line(x, y));
                }
                cx = x; cy = y;
                last_cubic_cp2 = None; last_quad_cp = None;
            }
            'H' => {
                let x = a[0] + if rel { cx } else { 0.0 };
                if !approx_eq(x, cx) { out.push(AbsCmd::Line(x, cy)); }
                cx = x;
                last_cubic_cp2 = None; last_quad_cp = None;
            }
            'V' => {
                let y = a[0] + if rel { cy } else { 0.0 };
                if !approx_eq(y, cy) { out.push(AbsCmd::Line(cx, y)); }
                cy = y;
                last_cubic_cp2 = None; last_quad_cp = None;
            }
            'C' => {
                let (cp1x, cp1y) = (ax!(0), ay!(1));
                let (cp2x, cp2y) = (ax!(2), ay!(3));
                let (x, y)       = (ax!(4), ay!(5));
                out.push(AbsCmd::Cubic(cp1x, cp1y, cp2x, cp2y, x, y));
                last_cubic_cp2 = Some((cp2x, cp2y)); last_quad_cp = None;
                cx = x; cy = y;
            }
            'S' => {
                // Reflect last cubic cp2 to get cp1
                let cp1 = last_cubic_cp2.map(|(px, py)| (2.0*cx - px, 2.0*cy - py))
                                        .unwrap_or((cx, cy));
                let (cp2x, cp2y) = (ax!(0), ay!(1));
                let (x, y)       = (ax!(2), ay!(3));
                out.push(AbsCmd::Cubic(cp1.0, cp1.1, cp2x, cp2y, x, y));
                last_cubic_cp2 = Some((cp2x, cp2y)); last_quad_cp = None;
                cx = x; cy = y;
            }
            'Q' => {
                let (cpx, cpy) = (ax!(0), ay!(1));
                let (x, y)     = (ax!(2), ay!(3));
                out.push(AbsCmd::Quad(cpx, cpy, x, y));
                last_quad_cp = Some((cpx, cpy)); last_cubic_cp2 = None;
                cx = x; cy = y;
            }
            'T' => {
                let cp = last_quad_cp.map(|(px, py)| (2.0*cx - px, 2.0*cy - py))
                                     .unwrap_or((cx, cy));
                let (x, y) = (ax!(0), ay!(1));
                out.push(AbsCmd::Quad(cp.0, cp.1, x, y));
                last_quad_cp = Some(cp); last_cubic_cp2 = None;
                cx = x; cy = y;
            }
            'A' => {
                let (x, y) = (ax!(5), ay!(6));
                out.push(AbsCmd::Arc {
                    rx: a[0].abs(), ry: a[1].abs(),
                    x_rot: a[2],
                    large: a[3] != 0.0,
                    sweep: a[4] != 0.0,
                    x, y,
                });
                cx = x; cy = y;
                last_cubic_cp2 = None; last_quad_cp = None;
            }
            'Z' => {
                out.push(AbsCmd::Close);
                cx = mx; cy = my;
                last_cubic_cp2 = None; last_quad_cp = None;
            }
            _ => {}
        }
    }
    out
}

fn approx_eq(a: f64, b: f64) -> bool { (a - b).abs() < 1e-9 }

// ─── Cleaning passes ──────────────────────────────────────────────────────────

fn clean_commands(cmds: Vec<AbsCmd>) -> Vec<AbsCmd> {
    let after_dedup_m  = remove_consecutive_moves(cmds);
    let after_empty_sp = remove_empty_subpaths(after_dedup_m);
    let after_collin   = merge_collinear_lines(after_empty_sp);
    after_collin
}

/// Drop all but the last of consecutive M commands (the intermediate ones draw nothing).
fn remove_consecutive_moves(cmds: Vec<AbsCmd>) -> Vec<AbsCmd> {
    let mut out = Vec::with_capacity(cmds.len());
    let n = cmds.len();
    for (i, cmd) in cmds.iter().enumerate() {
        if let AbsCmd::Move(_, _) = cmd {
            if i + 1 < n {
                if let AbsCmd::Move(_, _) = cmds[i + 1] {
                    continue; // skip this M, keep the next one
                }
            }
        }
        out.push(cmd.clone());
    }
    out
}

/// Remove M immediately followed by Z (draws nothing).
fn remove_empty_subpaths(cmds: Vec<AbsCmd>) -> Vec<AbsCmd> {
    let mut out = Vec::with_capacity(cmds.len());
    let n = cmds.len();
    let mut i = 0;
    while i < n {
        if let AbsCmd::Move(_, _) = &cmds[i] {
            if i + 1 < n {
                if let AbsCmd::Close = &cmds[i + 1] {
                    i += 2; // skip M + Z
                    continue;
                }
            }
        }
        out.push(cmds[i].clone());
        i += 1;
    }
    out
}

/// Merge runs of collinear L commands: if three consecutive Line endpoints are
/// collinear, drop the middle one.
fn merge_collinear_lines(cmds: Vec<AbsCmd>) -> Vec<AbsCmd> {
    if cmds.len() < 3 { return cmds; }
    let mut out: Vec<AbsCmd> = Vec::with_capacity(cmds.len());

    for cmd in &cmds {
        if let AbsCmd::Line(x2, y2) = cmd {
            if out.len() >= 2 {
                // Check if last two outputs plus this new point are collinear
                if let (Some(AbsCmd::Line(x1, y1)), Some(prev_cmd)) =
                    (out.last(), out.get(out.len().saturating_sub(2)))
                {
                    let (px, py) = match prev_cmd {
                        AbsCmd::Line(a, b) => (*a, *b),
                        AbsCmd::Move(a, b) => (*a, *b),
                        _ => { out.push(cmd.clone()); continue; }
                    };
                    let (x1, y1) = (*x1, *y1);
                    // Cross product of (p1-p0) × (p2-p0)
                    let cross = (x1 - px) * (y2 - py) - (y1 - py) * (x2 - px);
                    if cross.abs() < 1e-9 {
                        // Collinear — replace last Line with this endpoint
                        *out.last_mut().unwrap() = AbsCmd::Line(*x2, *y2);
                        continue;
                    }
                }
            }
        }
        out.push(cmd.clone());
    }
    out
}

// ─── Serialization ────────────────────────────────────────────────────────────

fn serialize_abs(cmds: &[AbsCmd], precision: u8) -> String {
    let mut out = String::new();
    let mut last_letter: Option<char> = None;

    for cmd in cmds {
        let (letter, args): (char, Vec<f64>) = match cmd {
            AbsCmd::Move(x, y)  => ('M', vec![*x, *y]),
            AbsCmd::Line(x, y)  => ('L', vec![*x, *y]),
            AbsCmd::Cubic(cp1x, cp1y, cp2x, cp2y, x, y) =>
                ('C', vec![*cp1x, *cp1y, *cp2x, *cp2y, *x, *y]),
            AbsCmd::Quad(cpx, cpy, x, y) =>
                ('Q', vec![*cpx, *cpy, *x, *y]),
            AbsCmd::Arc { rx, ry, x_rot, large, sweep, x, y } =>
                ('A', vec![*rx, *ry, *x_rot,
                    if *large { 1.0 } else { 0.0 },
                    if *sweep { 1.0 } else { 0.0 },
                    *x, *y]),
            AbsCmd::Close => {
                out.push('Z');
                last_letter = Some('Z');
                continue;
            }
        };

        let is_arc = letter == 'A';
        let repeat = last_letter == Some(letter);

        if !repeat {
            if !out.is_empty() { out.push(' '); }
            out.push(letter);
        }

        for (idx, &v) in args.iter().enumerate() {
            let s = fmt_abs(v, precision, letter, idx);
            // Arc flags: no separator (0/1 self-delimiting)
            let need_sep = !(is_arc && (idx == 3 || idx == 4));
            if need_sep && (idx == 0 || !s.starts_with('-')) {
                out.push(' ');
            } else if !need_sep {
                // no sep before arc flag
            } else {
                // negative number acts as its own separator
            }
            out.push_str(&s);
        }

        last_letter = Some(letter);
    }

    out
}

fn fmt_abs(v: f64, precision: u8, letter: char, idx: usize) -> String {
    // Arc flags are always 0 or 1
    if letter == 'A' && (idx == 3 || idx == 4) {
        return if v != 0.0 { "1".to_string() } else { "0".to_string() };
    }
    if v.is_nan() || v.is_infinite() { return "0".to_string(); }
    if v.fract() == 0.0 && v.abs() < 1e15 { return format!("{}", v as i64); }

    // Determine significant digits: control points use same precision
    let sig = precision as usize;
    let rounded = round_sig(v, sig);
    let s = format!("{:.10}", rounded);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if let Some(r) = s.strip_prefix("-0.") { format!("-.{}", r) }
    else if let Some(r) = s.strip_prefix("0.") { format!(".{}", r) }
    else { s.to_string() }
}

fn round_sig(v: f64, sig: usize) -> f64 {
    if v == 0.0 || sig == 0 { return 0.0; }
    let mag = v.abs().log10().floor() as i32;
    let factor = 10f64.powi(sig as i32 - 1 - mag);
    (v * factor).round() / factor
}

// ─── Presentation attribute comparison for path combination ──────────────────

/// Presentation attributes considered when deciding if two paths can be merged.
const COMBINE_ATTRS: &[&str] = &[
    "fill", "fill-opacity", "fill-rule",
    "stroke", "stroke-width", "stroke-opacity",
    "stroke-linecap", "stroke-linejoin", "stroke-miterlimit",
    "stroke-dasharray", "stroke-dashoffset",
    "opacity", "clip-path", "mask", "filter",
    "transform", "visibility", "display",
];

/// Check whether two attribute lists are equal on the COMBINE_ATTRS subset.
/// Both must have the same value (or both absent) for every attribute in the list.
pub fn paths_are_combinable(
    attrs1: &[(String, String)],
    attrs2: &[(String, String)],
    vars: &[CapturedVar],
) -> bool {
    let get = |attrs: &[(String, String)], key: &str| -> Option<String> {
        attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    };
    for &attr in COMBINE_ATTRS {
        let v1 = get(attrs1, attr);
        let v2 = get(attrs2, attr);
        // If either value contains a subst placeholder, not combinable
        if v1.as_deref().map(|v| value_has_subst(v, vars)).unwrap_or(false) { return false; }
        if v2.as_deref().map(|v| value_has_subst(v, vars)).unwrap_or(false) { return false; }
        if v1 != v2 { return false; }
    }
    true
}
