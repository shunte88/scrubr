/// Core SVG optimizer for scrubr.
///
/// Implements all scour-equivalent optimizations plus the three previously-planned
/// features now fully operational:
///   1. Gradient deduplication (`<linearGradient>` / `<radialGradient>` / `<pattern>`)
///   2. `<style>` block CSS optimization (color simplification, default stripping, ID remaps)
///   3. `--create-groups`: group sibling elements with identical presentation attributes
///
/// Substitution variables (`{{keyword}}`, `{{key-word}}`, `{{key_word}}`) are protected
/// end-to-end via a null-byte placeholder scheme applied before XML parsing and reversed
/// after serialization.

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
use crate::gradient::{
    GradientDef, StopKey, make_gradient_key, make_stop_key, find_duplicate_gradients,
};
use crate::style_block::optimize_style_block;
use crate::groups::{ElementFragment, group_runs};

//  Public Types

#[derive(Debug, Clone, PartialEq)]
pub enum Indent {
    None,
    Space,
    Tab,
}

#[derive(Debug, Clone)]
pub struct ScourOptions {
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
    pub gradients_deduplicated: usize,
}

//  Substitution Variable Protection

const SUBST_PREFIX: &str = "\x00SUBST";
const SUBST_SUFFIX: &str = "\x00END";

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

fn restore_subst_vars(output: &str, var_map: &HashMap<String, String>) -> String {
    if var_map.is_empty() {
        return output.to_string();
    }
    let mut entries: Vec<(&String, &String)> = var_map.iter().collect();
    entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    let mut result = output.to_string();
    for (placeholder, original) in entries {
        result = result.replace(placeholder.as_str(), original.as_str());
    }
    result
}

//  Entry Point

pub fn optimize_svg(input: &str, opts: &ScourOptions) -> (String, OptimizeStats) {
    let mut stats = OptimizeStats::default();

    // Phase 1 – protect substitution variables
    let (protected, var_map) = protect_subst_vars(input);
    stats.subst_vars_preserved = var_map.len();

    // Phase 2 – parse XML
    let doc = match Document::parse(&protected) {
        Ok(d) => d,
        Err(e) => {
            if !opts.quiet {
                eprintln!("scrubr: XML parse error: {}. Returning input unchanged.", e);
            }
            return (input.to_string(), stats);
        }
    };

    // Phase 3 – collect IDs, references, gradients, flow-text flag
    let mut all_ids: Vec<String> = Vec::new();
    let mut referenced_ids: HashSet<String> = HashSet::new();
    let mut has_flowtext = false;
    let mut gradient_defs: Vec<GradientDef> = Vec::new();

    collect_ids_and_refs(
        doc.root(),
        &mut all_ids,
        &mut referenced_ids,
        &mut has_flowtext,
        &mut gradient_defs,
    );
    stats.has_flowtext = has_flowtext;

    // Phase 4 – gradient deduplication
    let grad_renames = find_duplicate_gradients(&gradient_defs);
    stats.gradients_deduplicated = grad_renames.len();
    // Merge gradient renames into referenced_ids so duplicates get stripped later
    for (dup_id, _) in &grad_renames {
        referenced_ids.remove(dup_id.as_str());
    }

    // Phase 5 – build ID rename map
    let mut id_map: HashMap<String, Option<String>> = if opts.strip_ids || opts.shorten_ids {
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

    // Insert gradient duplicate renames into the id_map (map dup → canonical)
    for (dup_id, canonical_id) in &grad_renames {
        id_map.insert(dup_id.clone(), Some(canonical_id.clone()));
    }

    // Phase 6 – serialize
    let mut output = String::with_capacity(protected.len());
    if !opts.strip_xml_prolog {
        output.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    }

    for child in doc.root().children() {
        match child.node_type() {
            NodeType::ProcessingInstruction => {
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
            NodeType::Comment => {
                if !opts.strip_comments {
                    write_comment(child.text().unwrap_or(""), &mut output);
                    maybe_newline(&mut output, opts);
                }
            }
            NodeType::Element => {
                serialize_element(&child, &mut output, opts, &id_map, 0);
            }
            _ => {}
        }
    }

    // Phase 7 – restore substitution variables
    let final_output = restore_subst_vars(&output, &var_map);
    (final_output, stats)
}

//  ID / Reference / Gradient Collection

fn collect_ids_and_refs(
    node: Node,
    all_ids: &mut Vec<String>,
    referenced: &mut HashSet<String>,
    has_flowtext: &mut bool,
    gradient_defs: &mut Vec<GradientDef>,
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

        // Collect gradient definitions (inside <defs>)
        let parent_is_defs = child
            .parent()
            .map(|p| p.tag_name().name() == "defs")
            .unwrap_or(false);
        if parent_is_defs
            && matches!(ename, "linearGradient" | "radialGradient" | "pattern")
        {
            if let Some(grad) = extract_gradient_def(&child) {
                gradient_defs.push(grad);
            }
        }

        collect_ids_and_refs(child, all_ids, referenced, has_flowtext, gradient_defs);
    }
}

fn extract_gradient_def(node: &Node) -> Option<GradientDef> {
    let id = node.attribute("id")?.to_string();
    if id.contains(SUBST_PREFIX) {
        return None;
    }
    let tag = node.tag_name().name().to_string();

    let raw_attrs: Vec<(String, String)> = node
        .attributes()
        .filter(|a| a.name() != "id")
        .map(|a| (a.name().to_string(), a.value().to_string()))
        .collect();

    let inherits: Option<String> = raw_attrs
        .iter()
        .find(|(k, _)| k == "href" || k == "xlink:href")
        .map(|(_, v)| v.trim_start_matches('#').to_string());

    // Collect child <stop> elements
    let stops: Vec<StopKey> = node
        .children()
        .filter(|c| c.node_type() == NodeType::Element && c.tag_name().name() == "stop")
        .map(|stop| {
            let stop_attrs: Vec<(String, String)> = stop
                .attributes()
                .map(|a| (a.name().to_string(), a.value().to_string()))
                .collect();
            make_stop_key(&stop_attrs)
        })
        .collect();

    let key = make_gradient_key(&tag, &raw_attrs, stops);
    Some(GradientDef { id, key, inherits })
}

fn extract_id_refs(val: &str, referenced: &mut HashSet<String>) {
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

//  Editor Namespace List

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

//  Element Serialization

fn serialize_element(
    node: &Node,
    out: &mut String,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    depth: usize,
) {
    let tag = node.tag_name();
    let local = tag.name();
    let ns = tag.namespace().unwrap_or("");

    if !opts.keep_editor_data && !ns.is_empty() && is_editor_ns(ns) {
        return;
    }

    match local {
        "title" if opts.remove_titles => return,
        "desc" if opts.remove_descriptions => return,
        "metadata" if opts.remove_metadata => return,
        _ => {}
    }

    // Gradient dedup: skip duplicate gradient elements entirely
    if matches!(local, "linearGradient" | "radialGradient" | "pattern") {
        if let Some(id) = node.attribute("id") {
            if let Some(Some(_canonical)) = id_map.get(id) {
                // This ID maps to something else — it was a duplicate gradient, drop it
                // But only if it's genuinely a gradient rename (not a normal ID shorten)
                // We detect this by checking if the canonical != id
                if id_map.get(id).and_then(|v| v.as_deref()) != Some(id) {
                    return;
                }
            }
        }
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

    // Resolve id─
    let resolved_id = resolve_id(node, opts, id_map);

    // Process style=""─
    let (style_as_xml, remaining_style) = process_style(node, opts);

    // Gather attributes─
    let mut xml_attrs = gather_attrs(node, opts, id_map);

    // Merge presentation attrs from style= (don't override explicit XML attrs)
    for (k, v) in &style_as_xml {
        if !xml_attrs.iter().any(|(n, _)| n == k) {
            xml_attrs.push((k.clone(), v.clone()));
        }
    }

    xml_attrs.retain(|(k, v)| !is_default_value(k, v));
    xml_attrs.sort_by(|a, b| a.0.cmp(&b.0));

    // Viewboxing on root <svg>
    apply_viewboxing(node, local, opts, &mut xml_attrs);

    // Tag name─
    let tag_name = qualified_name(local, ns, node);

    // Opening tag─
    indent(out, opts, depth);
    out.push('<');
    out.push_str(&tag_name);
    emit_ns_decls(node, out, opts);

    if let Some(ref id) = resolved_id {
        write_attr(out, "id", id, opts, id_map);
    }
    for (k, v) in &xml_attrs {
        write_attr(out, k, v, opts, id_map);
    }
    if let Some(ref s) = remaining_style {
        write_attr(out, "style", s, opts, id_map);
    }

    // Children─
    let has_visible = node.children().any(|c| match c.node_type() {
        NodeType::Element => should_emit_element(&c, opts),
        NodeType::Text => !c.text().unwrap_or("").trim().is_empty(),
        NodeType::Comment => !opts.strip_comments,
        _ => false,
    });

    if !has_visible {
        out.push_str("/>");
        maybe_newline(out, opts);
        return;
    }

    out.push('>');
    maybe_newline(out, opts);

    // Special handling for <style> elements — optimize the CSS block
    if local == "style" {
        serialize_style_element(node, out, opts, id_map, depth);
    } else if opts.create_groups {
        serialize_children_with_grouping(node, out, opts, id_map, depth);
    } else {
        serialize_children(node, out, opts, id_map, depth);
    }

    indent(out, opts, depth);
    out.push_str("</");
    out.push_str(&tag_name);
    out.push('>');
    maybe_newline(out, opts);
}

//  <style> Element Handler

fn serialize_style_element(
    node: &Node,
    out: &mut String,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    depth: usize,
) {
    // Collect raw text content of the <style> element
    let mut css_text = String::new();
    for child in node.children() {
        match child.node_type() {
            NodeType::Text => css_text.push_str(child.text().unwrap_or("")),
            NodeType::Comment if !opts.strip_comments => {
                css_text.push_str("/*");
                css_text.push_str(child.text().unwrap_or(""));
                css_text.push_str("*/");
            }
            _ => {}
        }
    }

    let optimized_css = optimize_style_block(&css_text, id_map, opts.simplify_colors);

    if !optimized_css.trim().is_empty() {
        indent(out, opts, depth + 1);
        out.push_str(&optimized_css);
        maybe_newline(out, opts);
    }
}

//  Child Serialization (plain)

fn serialize_children(
    node: &Node,
    out: &mut String,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    depth: usize,
) {
    for child in node.children() {
        match child.node_type() {
            NodeType::Element => serialize_element(&child, out, opts, id_map, depth + 1),
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
            NodeType::ProcessingInstruction => {
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
            _ => {}
        }
    }
}

//  Child Serialization (with create-groups)─

fn serialize_children_with_grouping(
    node: &Node,
    out: &mut String,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    depth: usize,
) {
    // Serialize each child element into a fragment, then apply group_runs
    let mut fragments: Vec<ElementFragment> = Vec::new();
    let mut non_element_prefix = String::new();

    for child in node.children() {
        match child.node_type() {
            NodeType::Element => {
                // Flush any accumulated non-element content as a Raw fragment
                if !non_element_prefix.is_empty() {
                    out.push_str(&non_element_prefix);
                    non_element_prefix.clear();
                }
                let mut frag_text = String::new();
                let all_attrs = collect_all_attrs(&child, opts, id_map);
                serialize_element(&child, &mut frag_text, opts, id_map, depth + 1);
                let frag = ElementFragment::new(
                    child.tag_name().name(),
                    frag_text,
                    &all_attrs,
                );
                fragments.push(frag);
            }
            NodeType::Text => {
                // Flush fragments first, then emit text
                if !fragments.is_empty() {
                    let indent_unit = make_indent_unit(opts);
                    let grouped =
                        group_runs(&fragments, &indent_unit, depth + 1, opts.no_line_breaks);
                    out.push_str(&grouped);
                    fragments.clear();
                }
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
                    if !fragments.is_empty() {
                        let indent_unit = make_indent_unit(opts);
                        let grouped = group_runs(
                            &fragments,
                            &indent_unit,
                            depth + 1,
                            opts.no_line_breaks,
                        );
                        out.push_str(&grouped);
                        fragments.clear();
                    }
                    indent(out, opts, depth + 1);
                    write_comment(child.text().unwrap_or(""), out);
                    maybe_newline(out, opts);
                }
            }
            _ => {}
        }
    }

    // Flush remaining fragments
    if !fragments.is_empty() {
        let indent_unit = make_indent_unit(opts);
        let grouped =
            group_runs(&fragments, &indent_unit, depth + 1, opts.no_line_breaks);
        out.push_str(&grouped);
    }
}

/// Collect all non-id, non-style attributes plus style-derived presentation attrs
/// for a node, as flat (name, value) pairs — used to determine groupability.
fn collect_all_attrs(
    node: &Node,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
) -> Vec<(String, String)> {
    let mut attrs: Vec<(String, String)> = gather_attrs(node, opts, id_map);
    let (style_xml, _) = process_style(node, opts);
    for (k, v) in style_xml {
        if !attrs.iter().any(|(n, _)| n == &k) {
            attrs.push((k, v));
        }
    }
    attrs
}

fn make_indent_unit(opts: &ScourOptions) -> String {
    if opts.no_line_breaks || opts.indent == Indent::None {
        return String::new();
    }
    let unit = match opts.indent {
        Indent::Tab => "\t",
        _ => " ",
    };
    unit.repeat(opts.nindent as usize)
}

//  Group Collapsing─

fn can_collapse_group(node: &Node, opts: &ScourOptions) -> bool {
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

//  ID Resolution─

fn resolve_id(
    node: &Node,
    opts: &ScourOptions,
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

//  Style Processing─

fn process_style(
    node: &Node,
    opts: &ScourOptions,
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

//  Attribute Gathering─

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

fn gather_attrs(
    node: &Node,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
) -> Vec<(String, String)> {
    let local_name = node.tag_name().name();
    let mut attrs: Vec<(String, String)> = Vec::new();
    for attr in node.attributes() {
        let aname = attr.name();
        let ans = attr.namespace().unwrap_or("");
        if aname == "id" && ans.is_empty() { continue; }
        if aname == "style" && ans.is_empty() { continue; }
        if ans == XMLNS_NS || aname == "xmlns" || aname.starts_with("xmlns:") { continue; }
        if !opts.keep_editor_data && !ans.is_empty() && is_editor_ns(ans) { continue; }
        if aname == "space" && ans == XML_NS && opts.strip_xml_space { continue; }
        let val = attr.value();
        let qname: String = if ans.is_empty() {
            aname.to_string()
        } else {
            match find_ns_prefix(node, ans) {
                Some(ref pfx) if !pfx.is_empty() => format!("{}:{}", pfx, aname),
                _ => aname.to_string(),
            }
        };
        let optimized = optimize_attr(aname, val, local_name, opts, id_map);
        attrs.push((qname, optimized));
    }
    attrs
}

fn optimize_attr(
    name: &str,
    val: &str,
    element: &str,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
) -> String {
    let remapped = remap_id_refs(val, id_map);
    if remapped.contains(SUBST_PREFIX) {
        return remapped;
    }
    match name {
        "d" if element == "path" => optimize_path(&remapped, opts.precision, opts.c_precision),
        "transform" | "patternTransform" | "gradientTransform" => {
            optimize_transform(&remapped, opts.precision)
        }
        "fill" | "stroke" | "stop-color" | "flood-color" | "lighting-color" | "color"
            if opts.simplify_colors => simplify_color(&remapped),
        "x" | "y" | "x1" | "y1" | "x2" | "y2"
        | "cx" | "cy" | "r" | "rx" | "ry"
        | "fx" | "fy" | "offset"
        | "stroke-width" | "stroke-miterlimit" | "stroke-dashoffset"
        | "font-size" | "letter-spacing" | "word-spacing" | "kerning"
        | "opacity" | "fill-opacity" | "stroke-opacity" | "stop-opacity"
        | "flood-opacity" | "k" | "k1" | "k2" | "k3" | "k4"
        | "amplitude" | "exponent" | "intercept" | "slope"
        | "specularConstant" | "specularExponent" | "diffuseConstant"
        | "surfaceScale" | "seed" | "numOctaves" => optimize_number(&remapped, opts.precision),
        "width" | "height" if element != "svg" => optimize_number(&remapped, opts.precision),
        "viewBox" | "points" | "stdDeviation" | "baseFrequency"
        | "kernelMatrix" | "tableValues" | "values" | "keyTimes"
        | "keySplines" | "order" => optimize_number_list(&remapped, opts.precision),
        _ => remapped,
    }
}

//  Viewboxing─

fn apply_viewboxing(
    node: &Node,
    local: &str,
    opts: &ScourOptions,
    xml_attrs: &mut Vec<(String, String)>,
) {
    if !opts.enable_viewboxing || local != "svg" || node.attribute("viewBox").is_some() {
        return;
    }
    let w = node.attribute("width");
    let h = node.attribute("height");
    if let (Some(w), Some(h)) = (w, h) {
        if let (Some(wv), Some(hv)) = (strip_units(w), strip_units(h)) {
            let viewbox = format!("0 0 {} {}", wv, hv);
            for a in xml_attrs.iter_mut() {
                if a.0 == "width" { a.1 = "100%".to_string(); }
                if a.0 == "height" { a.1 = "100%".to_string(); }
            }
            xml_attrs.push(("viewBox".to_string(), viewbox));
        }
    }
}

//  Namespace Utilities─

fn find_ns_prefix(node: &Node, ns_uri: &str) -> Option<String> {
    let mut current = *node;
    loop {
        for attr in current.attributes() {
            if attr.value() == ns_uri {
                let aname = attr.name();
                if aname == "xmlns" { return Some(String::new()); }
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

fn emit_ns_decls(node: &Node, out: &mut String, opts: &ScourOptions) {
    for attr in node.attributes() {
        let aname = attr.name();
        let attr_ns = attr.namespace().unwrap_or("");
        let is_xmlns = attr_ns == XMLNS_NS || aname == "xmlns" || aname.starts_with("xmlns:");
        if !is_xmlns { continue; }
        let ns_val = attr.value();
        if !opts.keep_editor_data && !ns_val.is_empty() && is_editor_ns(ns_val) { continue; }
        out.push(' ');
        out.push_str(aname);
        out.push_str("=\"");
        out.push_str(&escape_xml_attr(ns_val));
        out.push('"');
    }
}

fn should_emit_element(node: &Node, opts: &ScourOptions) -> bool {
    let local = node.tag_name().name();
    let ns = node.tag_name().namespace().unwrap_or("");
    if !opts.keep_editor_data && !ns.is_empty() && is_editor_ns(ns) { return false; }
    match local {
        "title" if opts.remove_titles => false,
        "desc" if opts.remove_descriptions => false,
        "metadata" if opts.remove_metadata => false,
        _ => true,
    }
}

//  ID Reference Remapping─

fn remap_id_refs(val: &str, id_map: &HashMap<String, Option<String>>) -> String {
    if id_map.is_empty() || !val.contains('#') { return val.to_string(); }
    let stage1 = remap_url_refs(val, id_map);
    remap_href_refs(&stage1, id_map)
}

fn remap_url_refs(val: &str, id_map: &HashMap<String, Option<String>>) -> String {
    if !val.contains("url(#") { return val.to_string(); }
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
            None => { out.push_str("url(#"); out.push_str(rest); s = ""; break; }
        }
    }
    out.push_str(s);
    out
}

fn remap_href_refs(val: &str, id_map: &HashMap<String, Option<String>>) -> String {
    if !val.contains("=\"#") && !val.contains("='#") { return val.to_string(); }
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
            while i < n && chars[i] != quote { id.push(chars[i]); i += 1; }
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

//  Numeric Helpers

fn optimize_number(val: &str, precision: u8) -> String {
    if val.contains(SUBST_PREFIX) || val.contains("{{") { return val.to_string(); }
    let trimmed = val.trim();
    let (num_part, unit) = split_number_unit(trimmed);
    match num_part.parse::<f64>() {
        Ok(n) => format!("{}{}", fmt_f64(round_to_sig(n, precision as usize)), unit),
        Err(_) => val.to_string(),
    }
}

fn optimize_number_list(val: &str, precision: u8) -> String {
    if val.contains(SUBST_PREFIX) || val.contains("{{") { return val.to_string(); }
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
            if num.parse::<f64>().is_ok() { return (num, u); }
        }
    }
    (s, "")
}

pub fn round_to_sig(v: f64, sig: usize) -> f64 {
    if v == 0.0 || sig == 0 { return 0.0; }
    let magnitude = v.abs().log10().floor() as i32;
    let factor = 10f64.powi(sig as i32 - 1 - magnitude);
    (v * factor).round() / factor
}

fn fmt_f64(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { return format!("{}", v as i64); }
    let s = format!("{:.10}", v);
    let s = s.trim_end_matches('0');
    let s = s.trim_end_matches('.');
    s.to_string()
}

//  Output Helpers

fn write_attr(
    out: &mut String,
    name: &str,
    val: &str,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
) {
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

fn maybe_newline(out: &mut String, opts: &ScourOptions) {
    if !opts.no_line_breaks { out.push('\n'); }
}

fn indent(out: &mut String, opts: &ScourOptions, depth: usize) {
    if opts.no_line_breaks || opts.indent == Indent::None || depth == 0 { return; }
    let unit = match opts.indent { Indent::Tab => "\t", _ => " " };
    let count = depth * opts.nindent as usize;
    for _ in 0..count { out.push_str(unit); }
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
            if candidate.parse::<f64>().is_ok() { v = candidate; break; }
        }
    }
    if v.parse::<f64>().is_ok() { Some(v.to_string()) } else { None }
}
