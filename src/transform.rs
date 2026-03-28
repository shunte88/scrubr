/// SVG transform attribute optimizer.
/// Substitution variables in transform strings are preserved verbatim.

/// Optimize a transform attribute value.
/// Returns original string if it contains substitution variables.
pub fn optimize_transform(t: &str, precision: u8) -> String {
    if t.contains("{{") {
        return t.to_string();
    }

    let t = t.trim();
    if t.is_empty() || t == "none" {
        return t.to_string();
    }

    // Parse transform list
    let transforms = parse_transforms(t);
    if transforms.is_empty() {
        return t.to_string();
    }

    // Simplify each transform
    let simplified: Vec<String> = transforms.iter()
        .filter_map(|(name, args)| simplify_transform(name, args, precision))
        .collect();

    simplified.join(" ")
}

fn parse_transforms(s: &str) -> Vec<(String, Vec<f64>)> {
    let mut result = Vec::new();
    let mut s = s.trim();

    while !s.is_empty() {
        s = s.trim_start();
        // Find function name
        let paren = match s.find('(') {
            Some(p) => p,
            None => break,
        };
        let name = s[..paren].trim().to_lowercase();
        let rest = &s[paren+1..];
        let close = match rest.find(')') {
            Some(c) => c,
            None => break,
        };
        let args_str = &rest[..close];
        let args: Vec<f64> = args_str
            .split(|c: char| c == ',' || c == ' ' || c == '\t')
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        result.push((name, args));
        s = rest[close+1..].trim_start_matches(|c: char| c == ',' || c.is_whitespace());
    }
    result
}

fn round_to_prec(v: f64, prec: u8) -> f64 {
    if v == 0.0 { return 0.0; }
    let p = prec as i32;
    let d = p - 1 - v.abs().log10().floor() as i32;
    let factor = 10f64.powi(d);
    (v * factor).round() / factor
}

fn fmt(v: f64, prec: u8) -> String {
    let r = round_to_prec(v, prec);
    if r == r.floor() && r.abs() < 1e15 {
        format!("{}", r as i64)
    } else {
        format!("{}", r)
    }
}

fn simplify_transform(name: &str, args: &[f64], precision: u8) -> Option<String> {
    match name {
        "matrix" => {
            if args.len() < 6 { return None; }
            let (a, b, c, d, e, f) = (args[0], args[1], args[2], args[3], args[4], args[5]);
            // Identity?
            if approx_eq(a, 1.0) && approx_eq(b, 0.0) && approx_eq(c, 0.0)
                && approx_eq(d, 1.0) && approx_eq(e, 0.0) && approx_eq(f, 0.0) {
                return None; // drop identity
            }
            // Pure translation?
            if approx_eq(a, 1.0) && approx_eq(b, 0.0) && approx_eq(c, 0.0) && approx_eq(d, 1.0) {
                if approx_eq(f, 0.0) {
                    return Some(format!("translate({})", fmt(e, precision)));
                }
                return Some(format!("translate({},{})", fmt(e, precision), fmt(f, precision)));
            }
            // Pure scale?
            if approx_eq(b, 0.0) && approx_eq(c, 0.0) && approx_eq(e, 0.0) && approx_eq(f, 0.0) {
                if approx_eq(a, d) {
                    return Some(format!("scale({})", fmt(a, precision)));
                }
                return Some(format!("scale({},{})", fmt(a, precision), fmt(d, precision)));
            }
            Some(format!("matrix({},{},{},{},{},{})",
                fmt(a, precision), fmt(b, precision), fmt(c, precision),
                fmt(d, precision), fmt(e, precision), fmt(f, precision)))
        }
        "translate" => {
            if args.is_empty() { return None; }
            let x = args[0];
            let y = if args.len() > 1 { args[1] } else { 0.0 };
            if approx_eq(x, 0.0) && approx_eq(y, 0.0) {
                return None; // identity translate
            }
            if approx_eq(y, 0.0) {
                Some(format!("translate({})", fmt(x, precision)))
            } else {
                Some(format!("translate({},{})", fmt(x, precision), fmt(y, precision)))
            }
        }
        "scale" => {
            if args.is_empty() { return None; }
            let sx = args[0];
            let sy = if args.len() > 1 { args[1] } else { sx };
            if approx_eq(sx, 1.0) && approx_eq(sy, 1.0) {
                return None; // identity scale
            }
            if approx_eq(sx, sy) {
                Some(format!("scale({})", fmt(sx, precision)))
            } else {
                Some(format!("scale({},{})", fmt(sx, precision), fmt(sy, precision)))
            }
        }
        "rotate" => {
            if args.is_empty() { return None; }
            let angle = round_to_prec(args[0], precision);
            if approx_eq(angle, 0.0) { return None; }
            if args.len() >= 3 && (!approx_eq(args[1], 0.0) || !approx_eq(args[2], 0.0)) {
                Some(format!("rotate({},{},{})", fmt(angle, precision),
                    fmt(args[1], precision), fmt(args[2], precision)))
            } else {
                Some(format!("rotate({})", fmt(angle, precision)))
            }
        }
        "skewx" => {
            if args.is_empty() { return None; }
            let a = round_to_prec(args[0], precision);
            if approx_eq(a, 0.0) { return None; }
            Some(format!("skewX({})", fmt(a, precision)))
        }
        "skewy" => {
            if args.is_empty() { return None; }
            let a = round_to_prec(args[0], precision);
            if approx_eq(a, 0.0) { return None; }
            Some(format!("skewY({})", fmt(a, precision)))
        }
        _ => {
            // Unknown transform — preserve
            Some(format!("{}({})", name,
                args.iter().map(|v| fmt(*v, precision)).collect::<Vec<_>>().join(",")))
        }
    }
}

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-10
}
