/// Convert any CSS color expression to shortest #RRGGBB or #RGB form.
/// Returns None if the input should not be changed (e.g. contains a substitution variable).

use std::collections::HashMap;

/// Well-known CSS color keywords → RGB
fn color_keywords() -> HashMap<&'static str, (u8, u8, u8)> {
    let mut m = HashMap::new();
    m.insert("aliceblue",            (240,248,255));
    m.insert("antiquewhite",         (250,235,215));
    m.insert("aqua",                 (0,255,255));
    m.insert("aquamarine",           (127,255,212));
    m.insert("azure",                (240,255,255));
    m.insert("beige",                (245,245,220));
    m.insert("bisque",               (255,228,196));
    m.insert("black",                (0,0,0));
    m.insert("blanchedalmond",       (255,235,205));
    m.insert("blue",                 (0,0,255));
    m.insert("blueviolet",           (138,43,226));
    m.insert("brown",                (165,42,42));
    m.insert("burlywood",            (222,184,135));
    m.insert("cadetblue",            (95,158,160));
    m.insert("chartreuse",           (127,255,0));
    m.insert("chocolate",            (210,105,30));
    m.insert("coral",                (255,127,80));
    m.insert("cornflowerblue",       (100,149,237));
    m.insert("cornsilk",             (255,248,220));
    m.insert("crimson",              (220,20,60));
    m.insert("cyan",                 (0,255,255));
    m.insert("darkblue",             (0,0,139));
    m.insert("darkcyan",             (0,139,139));
    m.insert("darkgoldenrod",        (184,134,11));
    m.insert("darkgray",             (169,169,169));
    m.insert("darkgreen",            (0,100,0));
    m.insert("darkgrey",             (169,169,169));
    m.insert("darkkhaki",            (189,183,107));
    m.insert("darkmagenta",          (139,0,139));
    m.insert("darkolivegreen",       (85,107,47));
    m.insert("darkorange",           (255,140,0));
    m.insert("darkorchid",           (153,50,204));
    m.insert("darkred",              (139,0,0));
    m.insert("darksalmon",           (233,150,122));
    m.insert("darkseagreen",         (143,188,143));
    m.insert("darkslateblue",        (72,61,139));
    m.insert("darkslategray",        (47,79,79));
    m.insert("darkslategrey",        (47,79,79));
    m.insert("darkturquoise",        (0,206,209));
    m.insert("darkviolet",           (148,0,211));
    m.insert("deeppink",             (255,20,147));
    m.insert("deepskyblue",          (0,191,255));
    m.insert("dimgray",              (105,105,105));
    m.insert("dimgrey",              (105,105,105));
    m.insert("dodgerblue",           (30,144,255));
    m.insert("firebrick",            (178,34,34));
    m.insert("floralwhite",          (255,250,240));
    m.insert("forestgreen",          (34,139,34));
    m.insert("fuchsia",              (255,0,255));
    m.insert("gainsboro",            (220,220,220));
    m.insert("ghostwhite",           (248,248,255));
    m.insert("gold",                 (255,215,0));
    m.insert("goldenrod",            (218,165,32));
    m.insert("gray",                 (128,128,128));
    m.insert("green",                (0,128,0));
    m.insert("greenyellow",          (173,255,47));
    m.insert("grey",                 (128,128,128));
    m.insert("honeydew",             (240,255,240));
    m.insert("hotpink",              (255,105,180));
    m.insert("indianred",            (205,92,92));
    m.insert("indigo",               (75,0,130));
    m.insert("ivory",                (255,255,240));
    m.insert("khaki",                (240,230,140));
    m.insert("lavender",             (230,230,250));
    m.insert("lavenderblush",        (255,240,245));
    m.insert("lawngreen",            (124,252,0));
    m.insert("lemonchiffon",         (255,250,205));
    m.insert("lightblue",            (173,216,230));
    m.insert("lightcoral",           (240,128,128));
    m.insert("lightcyan",            (224,255,255));
    m.insert("lightgoldenrodyellow", (250,250,210));
    m.insert("lightgray",            (211,211,211));
    m.insert("lightgreen",           (144,238,144));
    m.insert("lightgrey",            (211,211,211));
    m.insert("lightpink",            (255,182,193));
    m.insert("lightsalmon",          (255,160,122));
    m.insert("lightseagreen",        (32,178,170));
    m.insert("lightskyblue",         (135,206,250));
    m.insert("lightslategray",       (119,136,153));
    m.insert("lightslategrey",       (119,136,153));
    m.insert("lightsteelblue",       (176,196,222));
    m.insert("lightyellow",          (255,255,224));
    m.insert("lime",                 (0,255,0));
    m.insert("limegreen",            (50,205,50));
    m.insert("linen",                (250,240,230));
    m.insert("magenta",              (255,0,255));
    m.insert("maroon",               (128,0,0));
    m.insert("mediumaquamarine",     (102,205,170));
    m.insert("mediumblue",           (0,0,205));
    m.insert("mediumorchid",         (186,85,211));
    m.insert("mediumpurple",         (147,112,219));
    m.insert("mediumseagreen",       (60,179,113));
    m.insert("mediumslateblue",      (123,104,238));
    m.insert("mediumspringgreen",    (0,250,154));
    m.insert("mediumturquoise",      (72,209,204));
    m.insert("mediumvioletred",      (199,21,133));
    m.insert("midnightblue",         (25,25,112));
    m.insert("mintcream",            (245,255,250));
    m.insert("mistyrose",            (255,228,225));
    m.insert("moccasin",             (255,228,181));
    m.insert("navajowhite",          (255,222,173));
    m.insert("navy",                 (0,0,128));
    m.insert("oldlace",              (253,245,230));
    m.insert("olive",                (128,128,0));
    m.insert("olivedrab",            (107,142,35));
    m.insert("orange",               (255,165,0));
    m.insert("orangered",            (255,69,0));
    m.insert("orchid",               (218,112,214));
    m.insert("palegoldenrod",        (238,232,170));
    m.insert("palegreen",            (152,251,152));
    m.insert("paleturquoise",        (175,238,238));
    m.insert("palevioletred",        (219,112,147));
    m.insert("papayawhip",           (255,239,213));
    m.insert("peachpuff",            (255,218,185));
    m.insert("peru",                 (205,133,63));
    m.insert("pink",                 (255,192,203));
    m.insert("plum",                 (221,160,221));
    m.insert("powderblue",           (176,224,230));
    m.insert("purple",               (128,0,128));
    m.insert("rebeccapurple",        (102,51,153));
    m.insert("red",                  (255,0,0));
    m.insert("rosybrown",            (188,143,143));
    m.insert("royalblue",            (65,105,225));
    m.insert("saddlebrown",          (139,69,19));
    m.insert("salmon",               (250,128,114));
    m.insert("sandybrown",           (244,164,96));
    m.insert("seagreen",             (46,139,87));
    m.insert("seashell",             (255,245,238));
    m.insert("sienna",               (160,82,45));
    m.insert("silver",               (192,192,192));
    m.insert("skyblue",              (135,206,235));
    m.insert("slateblue",            (106,90,205));
    m.insert("slategray",            (112,128,144));
    m.insert("slategrey",            (112,128,144));
    m.insert("snow",                 (255,250,250));
    m.insert("springgreen",          (0,255,127));
    m.insert("steelblue",            (70,130,180));
    m.insert("tan",                  (210,180,140));
    m.insert("teal",                 (0,128,128));
    m.insert("thistle",              (216,191,216));
    m.insert("tomato",               (255,99,71));
    m.insert("turquoise",            (64,224,208));
    m.insert("violet",               (238,130,238));
    m.insert("wheat",                (245,222,179));
    m.insert("white",                (255,255,255));
    m.insert("whitesmoke",           (245,245,245));
    m.insert("yellow",               (255,255,0));
    m.insert("yellowgreen",          (154,205,50));
    m
}

/// Simplify an RGB triple to the shortest hex representation
pub fn rgb_to_hex(r: u8, g: u8, b: u8) -> String {
    if r & 0x0f == r >> 4 && g & 0x0f == g >> 4 && b & 0x0f == b >> 4 {
        format!("#{:x}{:x}{:x}", r >> 4, g >> 4, b >> 4)
    } else {
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    }
}

/// Parse and simplify a color value. Returns the simplified form or original if not simplifiable.
/// Substitution variables ({{...}}) are returned as-is.
pub fn simplify_color(value: &str) -> String {
    let v = value.trim();

    // Already a short hex - normalize case
    if let Some(hex) = try_parse_hex(v) {
        return hex;
    }

    // Named color
    let lower = v.to_lowercase();
    let kw = color_keywords();
    if let Some(&(r, g, b)) = kw.get(lower.as_str()) {
        let hex = rgb_to_hex(r, g, b);
        // Only use hex if it's shorter or equal
        if hex.len() <= v.len() {
            return hex;
        }
        return lower;
    }

    // rgb(r,g,b) or rgb(r%, g%, b%)
    if let Some(inner) = strip_func(v, "rgb") {
        if let Some((r, g, b)) = parse_rgb_func(inner) {
            return rgb_to_hex(r, g, b);
        }
    }

    v.to_string()
}

fn strip_func<'a>(v: &'a str, name: &str) -> Option<&'a str> {
    let v = v.trim();
    let prefix = format!("{}(", name);
    if v.to_lowercase().starts_with(&prefix) && v.ends_with(')') {
        Some(&v[prefix.len()..v.len()-1])
    } else {
        None
    }
}

fn parse_rgb_func(inner: &str) -> Option<(u8, u8, u8)> {
    let parts: Vec<&str> = inner.split(',').collect();
    if parts.len() != 3 { return None; }
    let parse_channel = |s: &str| -> Option<u8> {
        let s = s.trim();
        if s.ends_with('%') {
            let pct: f64 = s[..s.len()-1].trim().parse().ok()?;
            Some((pct.clamp(0.0, 100.0) / 100.0 * 255.0).round() as u8)
        } else {
            let n: i32 = s.parse().ok()?;
            Some(n.clamp(0, 255) as u8)
        }
    };
    Some((parse_channel(parts[0])?, parse_channel(parts[1])?, parse_channel(parts[2])?))
}

fn try_parse_hex(v: &str) -> Option<String> {
    if !v.starts_with('#') { return None; }
    let hex = &v[1..];
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            Some(rgb_to_hex(r, g, b))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(rgb_to_hex(r, g, b))
        }
        _ => None,
    }
}
