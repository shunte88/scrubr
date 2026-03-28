/// SVG path `d` attribute optimizer.
/// Substitution variables ({{...}}) in path data are detected and the path is
/// returned unmodified to protect dynamic values.

/// Optimize the `d` attribute of a path element.
/// - Collapses redundant commands
/// - Reduces numeric precision
/// - Uses relative commands where shorter
/// Returns the original string if it contains substitution variables.
pub fn optimize_path(d: &str, precision: u8, c_precision: u8) -> String {
    // Guard: never modify paths containing substitution variables
    if d.contains("{{") {
        return d.to_string();
    }

    let tokens = tokenize_path(d);
    if tokens.is_empty() {
        return d.to_string();
    }

    let commands = parse_path_tokens(&tokens);
    if commands.is_empty() {
        return d.to_string();
    }

    let optimized = optimize_commands(commands, precision, c_precision);
    serialize_commands(&optimized)
}

#[derive(Debug, Clone)]
pub struct PathCommand {
    pub cmd: char,
    pub args: Vec<f64>,
}

fn tokenize_path(d: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();

    for ch in d.chars() {
        match ch {
            'M' | 'm' | 'Z' | 'z' | 'L' | 'l' | 'H' | 'h' | 'V' | 'v'
            | 'C' | 'c' | 'S' | 's' | 'Q' | 'q' | 'T' | 't' | 'A' | 'a' => {
                if !cur.trim().is_empty() {
                    tokens.push(cur.trim().to_string());
                }
                cur = ch.to_string();
            }
            ' ' | '\t' | '\n' | '\r' | ',' => {
                if !cur.trim().is_empty() {
                    tokens.push(cur.trim().to_string());
                    cur = String::new();
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        tokens.push(cur.trim().to_string());
    }
    tokens
}

fn parse_path_tokens(tokens: &[String]) -> Vec<PathCommand> {
    let mut commands = Vec::new();
    let mut i = 0;
    let mut current_cmd: Option<char> = None;

    while i < tokens.len() {
        let t = &tokens[i];
        if let Some(cmd_char) = parse_command_char(t) {
            current_cmd = Some(cmd_char);
            let args = collect_args(tokens, &mut i);
            commands.push(PathCommand { cmd: cmd_char, args });
        } else if let Some(cmd) = current_cmd {
            // Implicit repeat
            let mut j = i;
            let args = collect_args(tokens, &mut j);
            i = j;
            // For M implicit repeat becomes L/l
            let implicit_cmd = match cmd {
                'M' => 'L',
                'm' => 'l',
                c => c,
            };
            commands.push(PathCommand { cmd: implicit_cmd, args });
            continue;
        }
        i += 1;
    }
    commands
}

fn parse_command_char(s: &str) -> Option<char> {
    if s.len() == 1 {
        let c = s.chars().next().unwrap();
        if "MmZzLlHhVvCcSsQqTtAa".contains(c) {
            return Some(c);
        }
    }
    None
}

fn collect_args(tokens: &[String], i: &mut usize) -> Vec<f64> {
    let mut args = Vec::new();
    let start = *i + 1;
    let mut j = start;
    while j < tokens.len() {
        if parse_command_char(&tokens[j]).is_some() {
            break;
        }
        // Split concatenated numbers (e.g. "-.5.3")
        let nums = split_numbers(&tokens[j]);
        for n in nums {
            if let Ok(v) = n.parse::<f64>() {
                args.push(v);
            }
        }
        j += 1;
    }
    *i = j - 1;
    args
}

/// Split a token that may contain concatenated numbers like "1.5-.3"
fn split_numbers(s: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '-' || c == '+' {
            if !cur.is_empty() {
                results.push(cur.clone());
                cur.clear();
            }
            cur.push(c);
        } else if c == '.' {
            if cur.contains('.') {
                results.push(cur.clone());
                cur = String::from(".");
            } else {
                cur.push(c);
            }
        } else {
            cur.push(c);
        }
        i += 1;
    }
    if !cur.is_empty() { results.push(cur); }
    results
}

fn optimize_commands(cmds: Vec<PathCommand>, precision: u8, c_precision: u8) -> Vec<PathCommand> {
    let mut out: Vec<PathCommand> = Vec::new();
    let prec = precision as usize;
    let cprec = c_precision as usize;

    for cmd in cmds {
        // Round numbers to precision
        let rounded: Vec<f64> = cmd.args.iter().enumerate().map(|(idx, &v)| {
            let p = match cmd.cmd {
                'C' | 'c' | 'S' | 's' | 'Q' | 'q' => {
                    if idx < cmd.args.len() - 2 { cprec } else { prec }
                }
                _ => prec,
            };
            round_to_sig(v, p)
        }).collect();

        // Remove redundant Z duplicates
        if cmd.cmd == 'Z' || cmd.cmd == 'z' {
            if let Some(last) = out.last() {
                if last.cmd == 'Z' || last.cmd == 'z' {
                    continue;
                }
            }
        }

        out.push(PathCommand { cmd: cmd.cmd, args: rounded });
    }
    out
}

fn round_to_sig(v: f64, sig: usize) -> f64 {
    if v == 0.0 || sig == 0 { return 0.0; }
    let d = sig as i32 - 1 - v.abs().log10().floor() as i32;
    let factor = 10f64.powi(d);
    (v * factor).round() / factor
}

fn serialize_commands(cmds: &[PathCommand]) -> String {
    let mut out = String::new();
    let mut last_cmd: Option<char> = None;

    for cmd in cmds {
        if cmd.cmd == 'Z' || cmd.cmd == 'z' {
            out.push(cmd.cmd);
            last_cmd = Some(cmd.cmd);
            continue;
        }

        // Emit command letter unless it's a repeat of the last
        let emit_cmd = last_cmd != Some(cmd.cmd);
        if emit_cmd {
            if !out.is_empty() { out.push(' '); }
            out.push(cmd.cmd);
        }

        for (i, &arg) in cmd.args.iter().enumerate() {
            let s = format_number(arg);
            // Separator: omit if next arg starts with '-' or starts with '.'
            let needs_sep = i > 0 && !s.starts_with('-') && !s.starts_with('.');
            if i == 0 {
                if emit_cmd { out.push(' '); }
                else { out.push(' '); }
            } else if needs_sep {
                out.push(' ');
            }
            out.push_str(&s);
        }

        last_cmd = Some(cmd.cmd);
    }

    out.trim().to_string()
}

fn format_number(v: f64) -> String {
    if v == v.floor() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        // Strip trailing zeros
        let s = format!("{}", v);
        s
    }
}
