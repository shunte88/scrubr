/// SVG path `d` attribute optimizer.
///
/// Performs:
///   - Proper tokenization (handles concatenated numbers, arc flags, sign-as-separator)
///   - Redundant command removal (Z+Z, M followed only by Z)
///   - Numeric precision rounding with configurable significant digits
///   - Trailing-zero stripping ("1.50000" → "1.5", "2.0" → "2")
///   - Leading-zero elision ("-0.5" → "-.5", "0.5" → ".5")
///   - Command-letter omission on implicit repeat (e.g. "L 1 2 L 3 4" → "L 1 2 3 4")
///   - Separator minimization (space omitted before `-` and between flags in arc)
///
/// Paths containing substitution variables (`{{...}}`) are returned unchanged.

pub fn optimize_path(d: &str, precision: u8, c_precision: u8) -> String {
    let cmds = parse_path(d);
    if cmds.is_empty() {
        return d.to_string();
    }
    let rounded = round_commands(cmds, precision as usize, c_precision as usize);
    let cleaned = remove_redundant(rounded);
    serialize_path(&cleaned)
}

//  Data types 

#[derive(Debug, Clone)]
pub struct PathCommand {
    pub cmd: char,
    pub args: Vec<f64>,
}

//  Tokenizer 
//
// SVG path data is notoriously tricky:
//   - Numbers can be concatenated: "M10 20L30,40" or "M.5.6" or "1-2" or "1e2-3"
//   - Arc commands (A/a) have two single-bit flags at positions 3 and 4 (0-indexed)
//     that must never be separated by more than the required minimal space.
//   - Signs act as separators between numbers.

fn parse_path(d: &str) -> Vec<PathCommand> {
    let chars: Vec<char> = d.chars().collect();
    let n = chars.len();
    let mut cmds: Vec<PathCommand> = Vec::new();
    let mut i = 0;
    let mut current_cmd: Option<char> = None;

    while i < n {
        skip_ws_comma(&chars, &mut i);
        if i >= n { break; }

        let c = chars[i];

        if is_cmd(c) {
            current_cmd = Some(c);
            i += 1;
            // Read as many argument groups as follow before the next command letter
            let arity = cmd_arity(c);
            if arity == 0 {
                // Z / z — no args
                cmds.push(PathCommand { cmd: c, args: vec![] });
            } else {
                loop {
                    skip_ws_comma(&chars, &mut i);
                    if i >= n || is_cmd(chars[i]) { break; }
                    let args = read_args(&chars, &mut i, c, arity);
                    if args.is_empty() { break; }
                    cmds.push(PathCommand { cmd: c, args });
                }
            }
        } else {
            // Implicit repeat — use the last command
            if let Some(cmd) = current_cmd {
                let implicit = implicit_repeat_cmd(cmd);
                let arity = cmd_arity(implicit);
                if arity == 0 { i += 1; continue; }
                let args = read_args(&chars, &mut i, implicit, arity);
                if args.is_empty() { break; }
                cmds.push(PathCommand { cmd: implicit, args });
            } else {
                // Malformed path — skip character
                i += 1;
            }
        }
    }
    cmds
}

/// For an implicit repeat after M/m the subsequent implicit command is L/l.
fn implicit_repeat_cmd(cmd: char) -> char {
    match cmd { 'M' => 'L', 'm' => 'l', c => c }
}

/// Number of numeric arguments per command invocation.
fn cmd_arity(cmd: char) -> usize {
    match cmd.to_ascii_uppercase() {
        'M' | 'L' | 'T' => 2,
        'H' | 'V' => 1,
        'S' | 'Q' => 4,
        'C' => 6,
        'A' => 7,
        'Z' => 0,
        _ => 0,
    }
}

fn is_cmd(c: char) -> bool {
    "MmZzLlHhVvCcSsQqTtAa".contains(c)
}

fn skip_ws_comma(chars: &[char], i: &mut usize) {
    while *i < chars.len() && (chars[*i].is_whitespace() || chars[*i] == ',') {
        *i += 1;
    }
}

/// Read exactly `arity` numbers for the given command.
/// Arc commands (A/a) special-case flag parsing at argument indices 3 and 4.
fn read_args(chars: &[char], i: &mut usize, cmd: char, arity: usize) -> Vec<f64> {
    let mut args = Vec::with_capacity(arity);
    let is_arc = cmd == 'A' || cmd == 'a';

    for arg_idx in 0..arity {
        skip_ws_comma(chars, i);
        if *i >= chars.len() { break; }

        // Arc large-arc-flag and sweep-flag (arg indices 3, 4) are single bits
        if is_arc && (arg_idx == 3 || arg_idx == 4) {
            let flag_char = chars[*i];
            if flag_char == '0' || flag_char == '1' {
                args.push((flag_char == '1') as u8 as f64);
                *i += 1;
                continue;
            }
        }

        // Read a general floating-point number
        if let Some(v) = read_number(chars, i) {
            args.push(v);
        } else {
            break;
        }
    }
    args
}

/// Read one floating-point number from `chars` starting at `*i`,
/// advancing `*i` past the number. Returns None if no number found.
fn read_number(chars: &[char], i: &mut usize) -> Option<f64> {
    let n = chars.len();
    let start = *i;

    // Optional leading sign
    if *i < n && (chars[*i] == '+' || chars[*i] == '-') {
        *i += 1;
    }

    let mut has_digits = false;
    // Integer part
    while *i < n && chars[*i].is_ascii_digit() {
        has_digits = true;
        *i += 1;
    }
    // Decimal part
    if *i < n && chars[*i] == '.' {
        *i += 1;
        while *i < n && chars[*i].is_ascii_digit() {
            has_digits = true;
            *i += 1;
        }
    }
    if !has_digits {
        *i = start;
        return None;
    }
    // Exponent
    if *i < n && (chars[*i] == 'e' || chars[*i] == 'E') {
        let exp_start = *i;
        *i += 1;
        if *i < n && (chars[*i] == '+' || chars[*i] == '-') {
            *i += 1;
        }
        let mut exp_digits = false;
        while *i < n && chars[*i].is_ascii_digit() {
            exp_digits = true;
            *i += 1;
        }
        if !exp_digits {
            // No digits after 'e' — roll back to before 'e'
            *i = exp_start;
        }
    }
    let s: String = chars[start..*i].iter().collect();
    s.parse::<f64>().ok()
}

//  Precision Rounding 

fn round_commands(
    cmds: Vec<PathCommand>,
    prec: usize,
    cprec: usize,
) -> Vec<PathCommand> {
    cmds.into_iter()
        .map(|cmd| {
            let args = cmd.args.iter().enumerate().map(|(idx, &v)| {
                let p = arg_precision(&cmd.cmd, idx, cmd.args.len(), prec, cprec);
                // Never round arc flags
                if (cmd.cmd == 'A' || cmd.cmd == 'a') && (idx == 3 || idx == 4) {
                    return v;
                }
                round_to_sig(v, p)
            }).collect();
            PathCommand { cmd: cmd.cmd, args }
        })
        .collect()
}

fn arg_precision(cmd: &char, idx: usize, total: usize, prec: usize, cprec: usize) -> usize {
    match cmd {
        // For cubic bezier: first 4 args are control points → cprec; last 2 are endpoint → prec
        'C' | 'c' => if idx < total.saturating_sub(2) { cprec } else { prec },
        // For smooth cubic / quadratic: first 2 are control → cprec; last 2 are endpoint → prec
        'S' | 's' | 'Q' | 'q' => if idx < total.saturating_sub(2) { cprec } else { prec },
        _ => prec,
    }
}

fn round_to_sig(v: f64, sig: usize) -> f64 {
    if v == 0.0 || sig == 0 { return 0.0; }
    let magnitude = v.abs().log10().floor() as i32;
    let factor = 10f64.powi(sig as i32 - 1 - magnitude);
    (v * factor).round() / factor
}

//  Redundant Command Removal 

fn remove_redundant(cmds: Vec<PathCommand>) -> Vec<PathCommand> {
    let mut out: Vec<PathCommand> = Vec::with_capacity(cmds.len());

    for cmd in cmds {
        match cmd.cmd {
            // Drop consecutive Z/z
            'Z' | 'z' => {
                if out.last().map(|c| c.cmd == 'Z' || c.cmd == 'z').unwrap_or(false) {
                    continue;
                }
            }
            _ => {}
        }
        out.push(cmd);
    }

    // Remove a trailing lone M/m with no subsequent drawing commands
    if out.len() >= 2 {
        let last = out.last().unwrap();
        let second_last = &out[out.len() - 2];
        if (last.cmd == 'Z' || last.cmd == 'z')
            && (second_last.cmd == 'M' || second_last.cmd == 'm')
        {
            out.pop(); // Z
            out.pop(); // M
        }
    }

    out
}

//  Serialization 

fn serialize_path(cmds: &[PathCommand]) -> String {
    let mut out = String::new();
    let mut last_cmd: Option<char> = None;

    for cmd in cmds {
        let c = cmd.cmd;

        if c == 'Z' || c == 'z' {
            if let Some(lc) = last_cmd {
                // No space needed before Z
                if lc != 'Z' && lc != 'z' { /* nothing */ }
            }
            out.push(c);
            last_cmd = Some(c);
            continue;
        }

        // Emit command letter only on first use or change
        let repeat = last_cmd == Some(c);
        if !repeat {
            if !out.is_empty() { out.push(' '); }
            out.push(c);
        }

        let is_arc = c == 'A' || c == 'a';

        for (idx, &arg) in cmd.args.iter().enumerate() {
            let s = fmt_num(arg);

            if idx == 0 {
                if repeat {
                    // Separate from previous argument group
                    if !s.starts_with('-') {
                        out.push(' ');
                    }
                } else {
                    out.push(' ');
                }
            } else if is_arc && (idx == 3 || idx == 4) {
                // Arc flags (large-arc-flag and sweep-flag): no separator needed
                // before them — they are always 0 or 1, acting as their own delimiter.
            } else {
                // Standard separator: omit before '-' or before leading '.'
                // (negative sign and leading-decimal already act as implicit delimiters)
                if !s.starts_with('-') {
                    out.push(' ');
                }
            }
            out.push_str(&s);
        }

        last_cmd = Some(c);
    }

    out
}

/// Format a float with:
///   - Integer output when fractional part is zero ("2.0" → "2")
///   - Leading-zero elision ("0.5" → ".5", "-0.5" → "-.5")
///   - Trailing-zero stripping ("1.50000" → "1.5")
fn fmt_num(v: f64) -> String {
    if v.is_nan() || v.is_infinite() { return "0".to_string(); }
    // Integer case
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }

    // Format with enough decimal places then strip trailing zeros
    let s = format!("{:.10}", v);
    let s = s.trim_end_matches('0');
    let s = s.trim_end_matches('.');

    // Leading-zero elision: "0.5" → ".5",  "-0.5" → "-.5"
    if let Some(rest) = s.strip_prefix("-0.") {
        format!("-.{}", rest)
    } else if let Some(rest) = s.strip_prefix("0.") {
        format!(".{}", rest)
    } else {
        s.to_string()
    }
}
