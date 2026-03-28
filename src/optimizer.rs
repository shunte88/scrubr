/// Core SVG optimizer.
/// This module implements all optimizations matching the Python scour utility,
/// with special handling to preserve substitution variables of the form
/// {{keyword}}, {{key-word}}, or {{key_word}} throughout the entire SVG tree.

use std::collections::{HashMap, HashSet};
use roxmltree::{Document, Node, NodeType};

use crate::css::{
    parse_style, serialize_style, simplify_style_colors,
    extract_presentation_attrs, is_default_value,
};
use crate::color::simplify_color;
use crate::path::optimize_path;
use crate::transform::optimize_transform;
use crate::ids::build_id_map;


#[derive(Debug, Clone, PartialEq)]
pub enum Indent {
    None,
    Space,
    Tab,
}

#[derive(Debug, Clone)]
pub struct ScrubrOptions {
    pub precision: u8,
    pub c_precision: u8,
    pub simplify_colors: bool,
    pub style_to_xml: bool,
    pub group_collapsing: bool,
    pub create_groups: bool,
    pub keep_editor_data: bool,
    pub keep_unreferenced_defs: bool,
    pub renderer_workaround: bool,
    pub strip_xml_prolog: bool,
    pub remove_titles: bool,
    pub remove_descriptions: bool,
    pub remove_metadata: bool,
    pub strip_comments: bool,
    pub embed_rasters: bool,
    pub enable_viewboxing: bool,
    pub indent: Indent,
    pub nindent: u8,
    pub no_line_breaks: bool,
    pub strip_xml_space: bool,
    pub strip_ids: bool,
    pub shorten_ids: bool,
    pub shorten_ids_prefix: Option<String>,
    pub protect_ids_noninkscape: bool,
    pub protect_ids_list: Vec<String>,
    pub protect_ids_prefix: Option<String>,
    pub error_on_flowtext: bool,
    pub quiet: bool,
    pub verbose: bool,
}

#[derive(Debug, Default)]
pub struct OptimizeStats {
    pub has_flowtext: bool,
    pub subst_vars_preserved: usize,
}


/// Null-byte-delimited placeholder that cannot appear in valid SVG/XML.
const SUBST_PREFIX: &str = "\x00SUBST";
const SUBST_SUFFIX: &str = "\x00END";

/// Replace `{{keyword}}` patterns with opaque placeholders so the XML parser
/// never sees brace-pairs in attribute values or text nodes.
/// Returns the sanitised string and a map placeholder→original.
fn protect_subst_vars(input: &str) -> (String, HashMap<String, String>) {
    let mut var_map: HashMap<String, String> = HashMap::new();
    let mut result = String::with_capacity(input.len() + 64);
    let mut counter = 0usize;
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        if i + 1 < n && chars[i] == '{' && chars[i + 1] == '{' {
            let start = i;
            let mut j = i + 2;
            let mut found = false;
            while j + 1 < n {
                if chars[j] == '}' && chars[j + 1] == '}' {
                    found = true;
                    j += 2;
                    break;
                }
                j += 1;
            }
            if found {
                let inner: String = chars[start + 2..j - 2].iter().collect();
                if is_valid_subst_inner(&inner) {
                    let original: String = chars[start..j].iter().collect();
                    let placeholder =
                        format!("{}{}{}", SUBST_PREFIX, counter, SUBST_SUFFIX);
                    var_map.insert(placeholder.clone(), original);
                    result.push_str(&placeholder);
                    counter += 1;
                    i = j;
                    continue;
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    (result, var_map)
}

fn is_valid_subst_inner(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Restore all placeholders back to their original `{{...}}` tokens.
fn restore_subst_vars(output: &str, var_map: &HashMap<String, String>) -> String {
    if var_map.is_empty() {
        return output.to_string();
    }
    let mut entries: Vec<(&String, &String)> = var_map.iter().collect();
    // Sort longer placeholder first to prevent prefix ambiguity
    entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    let mut result = output.to_string();
    for (placeholder, original) in entries {
        result = result.replace(placeholder.as_str(), original.as_str());
    }
    result
}


pub fn optimize_svg(input: &str, opts: &ScrubrOptions) -> (String, OptimizeStats) {
    let mut stats = OptimizeStats::default();

    // Phase 1 – protect substitution variables from the XML parser
    let (protected, var_map) = protect_subst_vars(input);
    stats.subst_vars_preserved = var_map.len();

    // Phase 2 – parse
    let doc = match Document::parse(&protected) {
        Ok(d) => d,
        Err(e) => {
            if !opts.quiet {
                eprintln!(
                    "scrubr: XML parse error: {}. Returning input unchanged.",
                    e
                );
            }
            return (input.to_string(), stats);
        }
    };

    // Phase 3 – collect IDs and references
    let mut all_ids: Vec<String> = Vec::new();
    let mut referenced_ids: HashSet<String> = HashSet::new();
    let mut has_flowtext = false;
    collect_ids_and_refs(
        doc.root(),
        &mut all_ids,
        &mut referenced_ids,
        &mut has_flowtext,
    );
    stats.has_flowtext = has_flowtext;

    // Phase 4 – build ID rename map
    let id_map: HashMap<String, Option<String>> = if opts.strip_ids || opts.shorten_ids {
        let (m, _) = build_id_map(
            &all_ids,
            &referenced_ids,
            opts.strip_ids,
            opts.shorten_ids,
            opts.shorten_ids_prefix.as_deref(),
            opts.protect_ids_noninkscape,
            &opts.protect_ids_list,
            opts.protect_ids_prefix.as_deref(),
        );
        m
    } else {
        HashMap::new()
    };

    // Phase 5 – serialize
    let mut output = String::with_capacity(protected.len());

    if !opts.strip_xml_prolog {
        output.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    }

    for child in doc.root().children() {
        match child.node_type() {
            NodeType::Comment => {
                if !opts.strip_comments {
                    write_comment(child.text().unwrap_or(""), &mut output);
                    maybe_newline(&mut output, opts);
                }
            }
            NodeType::Element => {
                serialize_element(&child, &mut output, opts, &id_map, 0);
            }
            _ => {
                if child.pi().map(|p| p.target) != Some("xml") {
                    if let Some(pi) = child.pi() {
                        output.push_str(&format!(
                            "<?{} {}?>",
                            pi.target,
                            pi.value.unwrap_or("")
                        ));
                         maybe_newline(&mut output, opts);
                    }
                }
            }
        }
    }

    // Phase 6 – restore
    let final_output = restore_subst_vars(&output, &var_map);
    (final_output, stats)
}


fn collect_ids_and_refs(
    node: Node,
    all_ids: &mut Vec<String>,
    referenced: &mut HashSet<String>,
    has_flowtext: &mut bool,
) {
    for child in node.children() {
        if child.node_type() != NodeType::Element {
            continue;
        }
        let ename = child.tag_name().name();
        if matches!(ename, "flowRoot" | "flowPara" | "flowSpan" | "flowRegion") {
            *has_flowtext = true;
        }
        if let Some(id) = child.attribute("id") {
            if !id.contains(SUBST_PREFIX) && !id.is_empty() {
                all_ids.push(id.to_string());
            }
        }
        for attr in child.attributes() {
            extract_id_refs(attr.value(), referenced);
        }
        collect_ids_and_refs(child, all_ids, referenced, has_flowtext);
    }
}

fn extract_id_refs(val: &str, referenced: &mut HashSet<String>) {
    // url(#id)
    let mut s = val;
    while let Some(pos) = s.find("url(#") {
        let rest = &s[pos + 5..];
        if let Some(end) = rest.find(')') {
            let id = rest[..end].trim();
            if !id.is_empty() {
                referenced.insert(id.to_string());
            }
            s = &rest[end + 1..];
        } else {
            break;
        }
    }
    // bare #ref
    let mut s = val;
    while let Some(pos) = s.find('#') {
        let rest = &s[pos + 1..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace() || c == '"' || c == '\'' || c == ')' || c == '.'
            })
            .unwrap_or(rest.len());
        if end > 0 {
            referenced.insert(rest[..end].to_string());
        }
        if end >= rest.len() {
            break;
        }
        s = &rest[end..];
    }
}


const EDITOR_NAMESPACES: &[&str] = &[
    "http://www.inkscape.org/namespaces/inkscape",
    "http://sodipodi.sourceforge.net/DTD/sodipodi-0.0.dtd",
    "http://ns.adobe.com/AdobeIllustrator/10.0/",
    "http://ns.adobe.com/AdobeSVGViewerExtensions/3.0/",
    "http://ns.adobe.com/Variables/1.0/",
    "http://ns.adobe.com/SaveForWeb/1.0/",
    "http://ns.adobe.com/Extensibility/1.0/",
    "http://ns.adobe.com/Flows/1.0/",
    "http://ns.adobe.com/ImageReplacement/1.0/",
    "http://ns.adobe.com/GenericCustomNamespace/1.0/",
    "http://ns.adobe.com/XPath/1.0/",
    "http://sketchtool.com",
    "http://ns.adobe.com/pdf/1.3/",
    "http://ns.adobe.com/illustrator/1.0/",
];

fn is_editor_ns(ns: &str) -> bool {
    EDITOR_NAMESPACES.contains(&ns)
}


fn serialize_element(
    node: &Node,
    out: &mut String,
    opts: &ScrubrOptions,
    id_map: &HashMap<String, Option<String>>,
    depth: usize,
) {
    let tag = node.tag_name();
    let local = tag.name();
    let ns = tag.namespace().unwrap_or("");

    // Drop entire editor-namespace elements
    if !opts.keep_editor_data && !ns.is_empty() && is_editor_ns(ns) {
        return;
    }

    // Drop descriptive elements per options
    match local {
        "title" if opts.remove_titles => return,
        "desc" if opts.remove_descriptions => return,
        "metadata" if opts.remove_metadata => return,
        _ => {}
    }

    // Group collapsing
    if opts.group_collapsing && local == "g" && can_collapse_group(node, opts) {
        for child in node.children() {
            match child.node_type() {
                NodeType::Element => {
                    serialize_element(&child, out, opts, id_map, depth);
                }
                NodeType::Text => {
                    let t = child.text().unwrap_or("");
                    if !t.trim().is_empty() {
                        out.push_str(&escape_xml_text(t));
                    }
                }
                NodeType::Comment if !opts.strip_comments => {
                    write_comment(child.text().unwrap_or(""), out);
                }
                _ => {}
            }
        }
        return;
    }

    // Resolve id 
    let resolved_id: Option<String> = resolve_id(node, opts, id_map);

    // Process style attr
    let (style_as_xml, remaining_style) = process_style(node, opts);

    // Gather remaining attrs
    let mut xml_attrs: Vec<(String, String)> = gather_attrs(node, opts, id_map);

    // Merge presentation attrs extracted from style=, without overriding explicit XML attrs
    for (k, v) in &style_as_xml {
        if !xml_attrs.iter().any(|(n, _)| n == k) {
            xml_attrs.push((k.clone(), v.clone()));
        }
    }

    // Strip defaults and sort
    xml_attrs.retain(|(k, v)| !is_default_value(k, v));
    xml_attrs.sort_by(|a, b| a.0.cmp(&b.0));

    // Viewboxing on root <svg>
    apply_viewboxing(node, local, opts, &mut xml_attrs);

    // Tag name with namespace prefix
    let tag_name = qualified_name(local, ns, node);

    // Emit opening tag
    indent(out, opts, depth);
    out.push('<');
    out.push_str(&tag_name);

    // Namespace declarations
    emit_ns_decls(node, out, opts);

    // id first
    if let Some(ref id) = resolved_id {
        write_attr(out, "id", id, opts, id_map);
    }

    // Sorted attributes
    for (k, v) in &xml_attrs {
        write_attr(out, k, v, opts, id_map);
    }

    // Remaining style
    if let Some(ref s) = remaining_style {
        write_attr(out, "style", s, opts, id_map);
    }

    // Children
    let has_visible = node.children().any(|c| match c.node_type() {
        NodeType::Element => should_emit_element(&c, opts),
        NodeType::Text => !c.text().unwrap_or("").trim().is_empty(),
        NodeType::Comment => !opts.strip_comments,
        _ => false,
    });

    if !has_visible {
        out.push_str("/>");
        maybe_newline(out, opts);
    } else {
        out.push('>');
        maybe_newline(out, opts);

        for child in node.children() {
            match child.node_type() {
                NodeType::Element => {
                    serialize_element(&child, out, opts, id_map, depth + 1);
                }
                NodeType::Text => {
                    let t = child.text().unwrap_or("");
                    let t2 = if opts.no_line_breaks {
                        collapse_whitespace(t)
                    } else {
                        t.to_string()
                    };
                    if !t2.trim().is_empty() {
                        indent(out, opts, depth + 1);
                        out.push_str(&escape_xml_text(&t2));
                        maybe_newline(out, opts);
                    }
                }
                NodeType::Comment => {
                    if !opts.strip_comments {
                        indent(out, opts, depth + 1);
                        write_comment(child.text().unwrap_or(""), out);
                        maybe_newline(out, opts);
                    }
                }
                _ => {
                    if let Some(pi) = child.pi() {
                        indent(out, opts, depth + 1);
                        out.push_str(&format!(
                            "<?{} {}?>",
                            pi.target,
                            pi.value.unwrap_or("")
                        ));
                        maybe_newline(out, opts);
                    }
                }
            }
        }

        indent(out, opts, depth);
        out.push_str("</");
        out.push_str(&tag_name);
        out.push('>');
        maybe_newline(out, opts);
    }
}

fn should_emit_element(node: &Node, opts: &ScrubrOptions) -> bool {
    let local = node.tag_name().name();
    let ns = node.tag_name().namespace().unwrap_or("");
    if !opts.keep_editor_data && !ns.is_empty() && is_editor_ns(ns) {
        return false;
    }
    match local {
        "title" if opts.remove_titles => false,
        "desc" if opts.remove_descriptions => false,
        "metadata" if opts.remove_metadata => false,
        _ => true,
    }
}


fn resolve_id(
    node: &Node,
    opts: &ScrubrOptions,
    id_map: &HashMap<String, Option<String>>,
) -> Option<String> {
    let raw = node.attribute("id")?;
    if raw.contains(SUBST_PREFIX) {
        return Some(raw.to_string());
    }
    if opts.strip_ids || opts.shorten_ids {
        match id_map.get(raw) {
            Some(Some(new_id)) => Some(new_id.clone()),
            Some(None) => None,
            None => Some(raw.to_string()),
        }
    } else {
        Some(raw.to_string())
    }
}


fn process_style(
    node: &Node,
    opts: &ScrubrOptions,
) -> (Vec<(String, String)>, Option<String>) {
    let style_val = match node.attribute("style") {
        Some(s) if !s.trim().is_empty() => s,
        _ => return (Vec::new(), None),
    };

    let mut decls = parse_style(style_val);
    simplify_style_colors(&mut decls, opts.simplify_colors);

    let presentation_xml: Vec<(String, String)> = if opts.style_to_xml {
        extract_presentation_attrs(&mut decls)
            .into_iter()
            .filter(|(k, v)| !is_default_value(k, v))
            .collect()
    } else {
        Vec::new()
    };

    decls.retain(|(k, v)| !is_default_value(k, v));

    let remaining = if decls.is_empty() {
        None
    } else {
        Some(serialize_style(&decls))
    };

    (presentation_xml, remaining)
}


const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

fn gather_attrs(
    node: &Node,
    opts: &ScrubrOptions,
    id_map: &HashMap<String, Option<String>>,
) -> Vec<(String, String)> {
    let local_name = node.tag_name().name();
    let mut attrs: Vec<(String, String)> = Vec::new();

    for attr in node.attributes() {
        let aname = attr.name();
        let ans = attr.namespace().unwrap_or("");

        if aname == "id" && ans.is_empty() {
            continue;
        }
        if aname == "style" && ans.is_empty() {
            continue;
        }
        // xmlns declarations — handled by emit_ns_decls
        if ans == XMLNS_NS || aname == "xmlns" || aname.starts_with("xmlns:") {
            continue;
        }
        // Editor-namespace attributes
        if !opts.keep_editor_data && !ans.is_empty() && is_editor_ns(ans) {
            continue;
        }
        // xml:space stripping
        if aname == "space" && ans == XML_NS && opts.strip_xml_space {
            continue;
        }

        let val = attr.value();
        let qname: String = if ans.is_empty() {
            aname.to_string()
        } else {
            match find_ns_prefix(node, ans) {
                Some(ref pfx) if !pfx.is_empty() => {
                    format!("{}:{}", pfx, aname)
                }
                _ => aname.to_string(),
            }
        };

        let optimized = optimize_attr(aname, ans, val, local_name, opts, id_map);
        attrs.push((qname, optimized));
    }

    attrs
}

fn optimize_attr(
    name: &str,
    _ns: &str,
    val: &str,
    element: &str,
    opts: &ScrubrOptions,
    id_map: &HashMap<String, Option<String>>,
) -> String {
    // Remap ID references first (handles both normal and placeholder-containing values)
    let remapped = remap_id_refs(val, id_map);

    // Never further process placeholder-containing values
    if remapped.contains(SUBST_PREFIX) {
        return remapped;
    }

    match name {
        "d" if element == "path" => {
            optimize_path(&remapped, opts.precision, opts.c_precision)
        }
        "transform" | "patternTransform" | "gradientTransform" => {
            optimize_transform(&remapped, opts.precision)
        }
        "fill" | "stroke" | "stop-color" | "flood-color" | "lighting-color" | "color"
            if opts.simplify_colors =>
        {
            simplify_color(&remapped)
        }
        "x" | "y" | "x1" | "y1" | "x2" | "y2"
        | "cx" | "cy" | "r" | "rx" | "ry"
        | "fx" | "fy"
        | "offset"
        | "stroke-width" | "stroke-miterlimit" | "stroke-dashoffset"
        | "font-size" | "letter-spacing" | "word-spacing" | "kerning"
        | "opacity" | "fill-opacity" | "stroke-opacity" | "stop-opacity"
        | "flood-opacity" | "k" | "k1" | "k2" | "k3" | "k4"
        | "amplitude" | "exponent" | "intercept" | "slope"
        | "specularConstant" | "specularExponent" | "diffuseConstant"
        | "surfaceScale" | "seed" | "numOctaves" => {
            optimize_number(&remapped, opts.precision)
        }
        "width" | "height" if element != "svg" => {
            optimize_number(&remapped, opts.precision)
        }
        "viewBox" | "points" | "stdDeviation" | "baseFrequency"
        | "kernelMatrix" | "tableValues" | "values" | "keyTimes"
        | "keySplines" | "order" => {
            optimize_number_list(&remapped, opts.precision)
        }
        _ => remapped,
    }
}


fn apply_viewboxing(
    node: &Node,
    local: &str,
    opts: &ScrubrOptions,
    xml_attrs: &mut Vec<(String, String)>,
) {
    if !opts.enable_viewboxing || local != "svg" {
        return;
    }
    if node.attribute("viewBox").is_some() {
        return;
    }
    let w = node.attribute("width");
    let h = node.attribute("height");
    if let (Some(w), Some(h)) = (w, h) {
        if let (Some(wv), Some(hv)) = (strip_units(w), strip_units(h)) {
            let viewbox = format!("0 0 {} {}", wv, hv);
            for a in xml_attrs.iter_mut() {
                if a.0 == "width" {
                    a.1 = "100%".to_string();
                }
                if a.0 == "height" {
                    a.1 = "100%".to_string();
                }
            }
            xml_attrs.push(("viewBox".to_string(), viewbox));
        }
    }
}


fn can_collapse_group(node: &Node, opts: &ScrubrOptions) -> bool {
    if node.tag_name().name() != "g" {
        return false;
    }
    if node.attribute("id").is_some() {
        return false;
    }
    if node.attribute("class").is_some() {
        return false;
    }
    if node.attribute("transform").is_some() {
        return false;
    }
    if let Some(style) = node.attribute("style") {
        let decls = parse_style(style);
        if decls.iter().any(|(k, v)| !is_default_value(k, v)) {
            return false;
        }
    }
    let has_meaningful = node.attributes().any(|a| {
        let n = a.name();
        let ns = a.namespace().unwrap_or("");
        !matches!(n, "id" | "style" | "class")
            && (ns.is_empty() || (!opts.keep_editor_data && !is_editor_ns(ns)))
    });
    if has_meaningful {
        return false;
    }
    if node
        .parent()
        .map(|p| p.tag_name().name() == "defs")
        .unwrap_or(false)
    {
        return false;
    }
    true
}


fn optimize_number(val: &str, precision: u8) -> String {
    if val.contains(SUBST_PREFIX) || val.contains("{{") {
        return val.to_string();
    }
    let trimmed = val.trim();
    let (num_part, unit) = split_number_unit(trimmed);
    match num_part.parse::<f64>() {
        Ok(n) => {
            let r = round_to_sig(n, precision as usize);
            format!("{}{}", fmt_f64(r), unit)
        }
        Err(_) => val.to_string(),
    }
}

fn optimize_number_list(val: &str, precision: u8) -> String {
    if val.contains(SUBST_PREFIX) || val.contains("{{") {
        return val.to_string();
    }
    let sep = if val.contains(',') { "," } else { " " };
    val.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| optimize_number(s, precision))
        .collect::<Vec<_>>()
        .join(sep)
}

fn split_number_unit(s: &str) -> (&str, &str) {
    const UNITS: &[&str] = &["px", "pt", "pc", "mm", "cm", "in", "em", "ex", "%"];
    for u in UNITS {
        if s.ends_with(u) {
            let num = &s[..s.len() - u.len()];
            if num.parse::<f64>().is_ok() {
                return (num, u);
            }
        }
    }
    (s, "")
}

pub fn round_to_sig(v: f64, sig: usize) -> f64 {
    if v == 0.0 || sig == 0 {
        return 0.0;
    }
    let magnitude = v.abs().log10().floor() as i32;
    let factor = 10f64.powi(sig as i32 - 1 - magnitude);
    (v * factor).round() / factor
}

fn fmt_f64(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{:.10}", v);
    let s = s.trim_end_matches('0');
    let s = s.trim_end_matches('.');
    s.to_string()
}


fn find_ns_prefix(node: &Node, ns_uri: &str) -> Option<String> {
    let mut current = *node;
    loop {
        for attr in current.attributes() {
            if attr.value() == ns_uri {
                let aname = attr.name();
                if aname == "xmlns" {
                    return Some(String::new());
                }
                if let Some(pfx) = aname.strip_prefix("xmlns:") {
                    return Some(pfx.to_string());
                }
            }
        }
        match current.parent() {
            Some(p) => current = p,
            None => break,
        }
    }
    None
}

fn qualified_name(local: &str, ns: &str, node: &Node) -> String {
    if ns.is_empty() || ns == "http://www.w3.org/2000/svg" {
        return local.to_string();
    }
    match find_ns_prefix(node, ns) {
        Some(pfx) if !pfx.is_empty() => format!("{}:{}", pfx, local),
        _ => local.to_string(),
    }
}

fn emit_ns_decls(node: &Node, out: &mut String, opts: &ScrubrOptions) {
    for attr in node.attributes() {
        let aname = attr.name();
        let attr_ns = attr.namespace().unwrap_or("");
        let is_xmlns = attr_ns == XMLNS_NS
            || aname == "xmlns"
            || aname.starts_with("xmlns:");
        if !is_xmlns {
            continue;
        }
        let ns_val = attr.value();
        if !opts.keep_editor_data && !ns_val.is_empty() && is_editor_ns(ns_val) {
            continue;
        }
        out.push(' ');
        out.push_str(aname);
        out.push_str("=\"");
        out.push_str(&escape_xml_attr(ns_val));
        out.push('"');
    }
}


fn remap_id_refs(val: &str, id_map: &HashMap<String, Option<String>>) -> String {
    if id_map.is_empty() || !val.contains('#') {
        return val.to_string();
    }
    let stage1 = remap_url_refs(val, id_map);
    remap_href_refs(&stage1, id_map)
}

fn remap_url_refs(val: &str, id_map: &HashMap<String, Option<String>>) -> String {
    if !val.contains("url(#") {
        return val.to_string();
    }
    let mut out = String::with_capacity(val.len());
    let mut s = val;
    while let Some(pos) = s.find("url(#") {
        out.push_str(&s[..pos]);
        let rest = &s[pos + 5..];
        match rest.find(')') {
            Some(end) => {
                let id = rest[..end].trim();
                match id_map.get(id) {
                    Some(Some(new_id)) => out.push_str(&format!("url(#{})", new_id)),
                    _ => out.push_str(&format!("url(#{})", id)),
                }
                s = &rest[end + 1..];
            }
            None => {
                out.push_str("url(#");
                out.push_str(rest);
                s = "";
                break;
            }
        }
    }
    out.push_str(s);
    out
}

fn remap_href_refs(val: &str, id_map: &HashMap<String, Option<String>>) -> String {
    if !val.contains("=\"#") && !val.contains("='#") {
        return val.to_string();
    }
    let chars: Vec<char> = val.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(val.len());
    let mut i = 0;
    while i < n {
        if i + 2 < n
            && chars[i] == '='
            && (chars[i + 1] == '"' || chars[i + 1] == '\'')
            && chars[i + 2] == '#'
        {
            let quote = chars[i + 1];
            out.push('=');
            out.push(quote);
            out.push('#');
            i += 3;
            let mut id = String::new();
            while i < n && chars[i] != quote {
                id.push(chars[i]);
                i += 1;
            }
            match id_map.get(id.as_str()) {
                Some(Some(new_id)) => out.push_str(new_id),
                _ => out.push_str(&id),
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}


fn write_attr(
    out: &mut String,
    name: &str,
    val: &str,
    opts: &ScrubrOptions,
    id_map: &HashMap<String, Option<String>>,
) {
    // Remap id references that may have slipped through (e.g. in style=remaining)
    let final_val = if (opts.strip_ids || opts.shorten_ids) && val.contains('#') {
        remap_id_refs(val, id_map)
    } else {
        val.to_string()
    };
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    out.push_str(&escape_xml_attr(&final_val));
    out.push('"');
}

fn write_comment(text: &str, out: &mut String) {
    out.push_str("<!--");
    out.push_str(text);
    out.push_str("-->");
}

fn maybe_newline(out: &mut String, opts: &ScrubrOptions) {
    if !opts.no_line_breaks {
        out.push('\n');
    }
}

fn indent(out: &mut String, opts: &ScrubrOptions, depth: usize) {
    if opts.no_line_breaks || opts.indent == Indent::None || depth == 0 {
        return;
    }
    let unit = match opts.indent {
        Indent::Tab => "\t",
        _ => " ",
    };
    let count = depth * opts.nindent as usize;
    for _ in 0..count {
        out.push_str(unit);
    }
}

fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_units(val: &str) -> Option<String> {
    const UNITS: &[&str] = &["px", "pt", "pc", "mm", "cm", "in", "em", "ex"];
    let mut v = val.trim();
    for u in UNITS {
        if v.ends_with(u) {
            let candidate = &v[..v.len() - u.len()];
            if candidate.parse::<f64>().is_ok() {
                v = candidate;
                break;
            }
        }
    }
    if v.parse::<f64>().is_ok() {
        Some(v.to_string())
    } else {
        None
    }
}
