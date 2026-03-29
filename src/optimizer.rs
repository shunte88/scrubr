/// Core SVG optimizer for scrubr.
///
/// Uses the `subst` module for substitution-variable protection:
///   - Phase 1: Replace {{var}} with XML-safe context-appropriate placeholders
///   - Phase 2: Parse the now-valid XML
///   - Phases 3–6: Analyse, deduplicate, rename, serialize
///   - Phase 7: Restore all {{var}} tokens from the placeholder map

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
    GradientDef, make_gradient_key, make_stop_key, find_duplicate_gradients,
};
use crate::style_block::optimize_style_block;
use crate::groups::{ElementFragment, group_runs};
use crate::subst::{
    protect_subst_vars, restore_subst_vars,
    value_has_subst, CapturedVar,
};
use crate::path_simplify::{
    simplify_path_d, combine_path_d, paths_are_combinable,
};

// ─── Public Types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Indent { None, Space, Tab }

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
    pub simplify_paths: bool,
    pub combine_paths: bool,
    pub quiet: bool,
    pub verbose: bool,
}

#[derive(Debug, Default)]
pub struct OptimizeStats {
    pub has_flowtext: bool,
    pub subst_vars_preserved: usize,
    pub gradients_deduplicated: usize,
    pub paths_simplified: usize,
    pub paths_combined: usize,
    pub empty_defs_removed: usize,
}

// ─── Entry Point ──────────────────────────────────────────────────────────────

pub fn optimize_svg(input: &str, opts: &ScourOptions) -> (String, OptimizeStats) {
    let mut stats = OptimizeStats::default();

    // Phase 1 – replace {{var}} with XML-safe context-aware placeholders.
    // The protected string is valid XML; each placeholder embeds a neutral
    // value appropriate for its attribute type so optimizer passes work
    // correctly on the synthetic value rather than corrupting it.
    let (protected, vars) = protect_subst_vars(input);
    stats.subst_vars_preserved = vars.len();

    // Phase 2 – parse
    let doc = match Document::parse(&protected) {
        Ok(d) => d,
        Err(e) => {
            if !opts.quiet {
                eprintln!("scrubr: XML parse error: {}. Returning input unchanged.", e);
            }
            return (input.to_string(), stats);
        }
    };

    // Phase 3 – collect IDs, references, gradient defs, flowtext flag
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
        &vars,
    );
    stats.has_flowtext = has_flowtext;

    // Phase 4 – gradient deduplication
    let grad_renames = find_duplicate_gradients(&gradient_defs, &vars);
    stats.gradients_deduplicated = grad_renames.len();
    let grad_dup_ids: HashSet<String> = grad_renames.keys().cloned().collect();
    for dup_id in &grad_dup_ids {
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
                            pi.target, pi.value.unwrap_or("")
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
                serialize_element(
                    &child, &mut output, opts, &id_map, &grad_dup_ids, &vars, &mut stats, 0,
                );
            }
            _ => {}
        }
    }

    // Phase 7 – restore {{var}} tokens
    let final_output = restore_subst_vars(&output, &vars);
    (final_output, stats)
}

// ─── ID / Reference / Gradient Collection ─────────────────────────────────────

fn collect_ids_and_refs(
    node: Node,
    all_ids: &mut Vec<String>,
    referenced: &mut HashSet<String>,
    has_flowtext: &mut bool,
    gradient_defs: &mut Vec<GradientDef>,
    vars: &[CapturedVar],
) {
    for child in node.children() {
        if child.node_type() != NodeType::Element { continue; }
        let ename = child.tag_name().name();
        if matches!(ename, "flowRoot" | "flowPara" | "flowSpan" | "flowRegion") {
            *has_flowtext = true;
        }
        if let Some(id) = child.attribute("id") {
            // Skip IDs that are entirely a placeholder — they'll be restored
            if !value_has_subst(id, vars) && !id.is_empty() {
                all_ids.push(id.to_string());
            }
        }
        for attr in child.attributes() {
            extract_id_refs(attr.value(), referenced);
        }
        let parent_is_defs = child.parent()
            .map(|p| p.tag_name().name() == "defs").unwrap_or(false);
        if parent_is_defs && matches!(ename, "linearGradient" | "radialGradient" | "pattern") {
            if let Some(grad) = extract_gradient_def(&child) {
                gradient_defs.push(grad);
            }
        }
        collect_ids_and_refs(child, all_ids, referenced, has_flowtext, gradient_defs, vars);
    }
}

fn extract_gradient_def(node: &Node) -> Option<GradientDef> {
    let id = node.attribute("id")?.to_string();
    let tag = node.tag_name().name().to_string();
    let raw_attrs: Vec<(String, String)> = node.attributes()
        .filter(|a| a.name() != "id")
        .map(|a| (a.name().to_string(), a.value().to_string()))
        .collect();
    let inherits: Option<String> = raw_attrs.iter()
        .find(|(k, _)| k == "href" || k == "xlink:href")
        .map(|(_, v)| v.trim_start_matches('#').to_string());
    let stops = node.children()
        .filter(|c| c.node_type() == NodeType::Element && c.tag_name().name() == "stop")
        .map(|s| {
            let sa: Vec<(String, String)> = s.attributes()
                .map(|a| (a.name().to_string(), a.value().to_string()))
                .collect();
            make_stop_key(&sa)
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
            if !id.is_empty() { referenced.insert(id.to_string()); }
            s = &rest[end + 1..];
        } else { break; }
    }
    let mut s = val;
    while let Some(pos) = s.find('#') {
        let rest = &s[pos + 1..];
        let end = rest.find(|c: char| {
            c.is_whitespace() || c == '"' || c == '\'' || c == ')' || c == '.'
        }).unwrap_or(rest.len());
        if end > 0 { referenced.insert(rest[..end].to_string()); }
        if end >= rest.len() { break; }
        s = &rest[end..];
    }
}

// ─── Editor Namespaces ────────────────────────────────────────────────────────

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
fn is_editor_ns(ns: &str) -> bool { EDITOR_NAMESPACES.contains(&ns) }

// ─── Element Serialization ────────────────────────────────────────────────────

fn serialize_element(
    node: &Node,
    out: &mut String,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    grad_dup_ids: &HashSet<String>,
    vars: &[CapturedVar],
    stats: &mut OptimizeStats,
    depth: usize,
) {
    let tag   = node.tag_name();
    let local = tag.name();
    let ns    = tag.namespace().unwrap_or("");

    if !opts.keep_editor_data && !ns.is_empty() && is_editor_ns(ns) { return; }
    match local {
        "title"    if opts.remove_titles       => return,
        "desc"     if opts.remove_descriptions => return,
        "metadata" if opts.remove_metadata     => return,
        _ => {}
    }

    // Suppress deduplicated gradients
    if matches!(local, "linearGradient" | "radialGradient" | "pattern") {
        if let Some(id) = node.attribute("id") {
            if grad_dup_ids.contains(id) { return; }
        }
    }

    // Remove empty <defs/> — a <defs> with no visible children is useless
    if local == "defs" && !opts.keep_unreferenced_defs {
        let has_content = node.children().any(|c| match c.node_type() {
            NodeType::Element => should_emit_element(&c, opts) && {
                // check it's not itself a suppressed gradient dup
                if matches!(c.tag_name().name(), "linearGradient" | "radialGradient" | "pattern") {
                    c.attribute("id").map(|id| !grad_dup_ids.contains(id)).unwrap_or(true)
                } else { true }
            },
            NodeType::Text => !c.text().unwrap_or("").trim().is_empty(),
            _ => false,
        });
        if !has_content {
            stats.empty_defs_removed += 1;
            return;
        }
    }

    // Group collapsing
    if opts.group_collapsing && local == "g" && can_collapse_group(node, opts) {
        for child in node.children() {
            match child.node_type() {
                NodeType::Element => {
                    serialize_element(&child, out, opts, id_map, grad_dup_ids, vars, stats, depth);
                }
                NodeType::Text => {
                    let t = child.text().unwrap_or("");
                    if !t.trim().is_empty() { out.push_str(&escape_xml_text(t)); }
                }
                NodeType::Comment if !opts.strip_comments => {
                    write_comment(child.text().unwrap_or(""), out);
                }
                _ => {}
            }
        }
        return;
    }

    // ── Resolve id ──────────────────────────────────────────────────────
    let resolved_id = resolve_id(node, opts, id_map, vars);

    // ── Process style="" ─────────────────────────────────────────────────
    let (style_as_xml, remaining_style) = process_style(node, opts, vars);

    // ── Gather and merge attributes ───────────────────────────────────────
    let mut xml_attrs = gather_attrs(node, opts, id_map, vars);
    for (k, v) in &style_as_xml {
        if !xml_attrs.iter().any(|(n, _)| n == k) {
            xml_attrs.push((k.clone(), v.clone()));
        }
    }
    xml_attrs.retain(|(k, v)| {
        // Never strip an attribute that contains a placeholder
        value_has_subst(v, vars) || !is_default_value(k, v)
    });

    // Path simplification stat tracking: count paths where d changed
    if opts.simplify_paths && local == "path" {
        for (k, v) in &mut xml_attrs {
            if k == "d" && !value_has_subst(v, vars) {
                let simplified = simplify_path_d(v, opts.precision);
                if simplified != *v {
                    stats.paths_simplified += 1;
                    *v = simplified;
                }
            }
        }
    }

    xml_attrs.sort_by(|a, b| a.0.cmp(&b.0));
    apply_viewboxing(node, local, opts, &mut xml_attrs, vars);

    // ── Serialize opening tag ─────────────────────────────────────────────
    let tag_name = qualified_name(local, ns, node);
    indent(out, opts, depth);
    out.push('<');
    out.push_str(&tag_name);
    emit_ns_decls(node, out, opts);

    if let Some(id) = &resolved_id {
        write_attr(out, "id", id, opts, id_map, vars);
    }
    for (k, v) in &xml_attrs {
        write_attr(out, k, v, opts, id_map, vars);
    }
    if let Some(s) = &remaining_style {
        write_attr(out, "style", s, opts, id_map, vars);
    }

    // ── Children ──────────────────────────────────────────────────────────
    let has_visible = node.children().any(|c| match c.node_type() {
        NodeType::Element => {
            if !should_emit_element(&c, opts) { return false; }
            let cl = c.tag_name().name();
            if matches!(cl, "linearGradient" | "radialGradient" | "pattern") {
                if let Some(cid) = c.attribute("id") {
                    if grad_dup_ids.contains(cid) { return false; }
                }
            }
            true
        }
        NodeType::Text    => !c.text().unwrap_or("").trim().is_empty(),
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

    if local == "style" {
        serialize_style_element(node, out, opts, id_map, grad_dup_ids, vars, stats, depth);
    } else if opts.create_groups {
        serialize_children_with_grouping(node, out, opts, id_map, grad_dup_ids, vars, stats, depth);
    } else {
        serialize_children(node, out, opts, id_map, grad_dup_ids, vars, stats, depth);
    }

    indent(out, opts, depth);
    out.push_str("</");
    out.push_str(&tag_name);
    out.push('>');
    maybe_newline(out, opts);
}

// ─── <style> Element ─────────────────────────────────────────────────────────

fn serialize_style_element(
    node: &Node,
    out: &mut String,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    grad_dup_ids: &HashSet<String>,
    vars: &[CapturedVar],
    stats: &mut OptimizeStats,
    depth: usize,
) {
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
    let optimized = optimize_style_block(&css_text, id_map, opts.simplify_colors, vars);
    if !optimized.trim().is_empty() {
        indent(out, opts, depth + 1);
        out.push_str(&optimized);
        maybe_newline(out, opts);
    }
}

// ─── Child Serialization ─────────────────────────────────────────────────────

fn serialize_children(
    node: &Node,
    out: &mut String,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    grad_dup_ids: &HashSet<String>,
    vars: &[CapturedVar],
    stats: &mut OptimizeStats,
    depth: usize,
) {
    // Collect element children so we can do a combination pass
    // Non-element nodes (text, comments) are emitted immediately and break runs.
    let children: Vec<roxmltree::Node> = node.children().collect();
    let n = children.len();
    let mut skip_next = false;

    for idx in 0..n {
        if skip_next { skip_next = false; continue; }
        let child = children[idx];

        match child.node_type() {
            NodeType::Element => {
                // Path combination: if this and the next sibling are both <path>
                // elements with no id, identical presentation attrs, and no subst
                // placeholder in their d values — merge them.
                if opts.combine_paths
                    && child.tag_name().name() == "path"
                    && child.attribute("id").is_none()
                    && idx + 1 < n
                {
                    let next = children[idx + 1];
                    if next.node_type() == NodeType::Element
                        && next.tag_name().name() == "path"
                        && next.attribute("id").is_none()
                    {
                        let d1 = child.attribute("d").unwrap_or("");
                        let d2 = next.attribute("d").unwrap_or("");
                        let attrs1 = collect_all_attrs(&child, opts, id_map, vars);
                        let attrs2 = collect_all_attrs(&next, opts, id_map, vars);
                        // Compare non-d presentation attrs
                        let attrs1_nd: Vec<_> = attrs1.iter().filter(|(k,_)| k != "d").cloned().collect();
                        let attrs2_nd: Vec<_> = attrs2.iter().filter(|(k,_)| k != "d").cloned().collect();
                        if crate::path_simplify::paths_are_combinable(&attrs1_nd, &attrs2_nd, vars) {
                            if let Some(combined) = combine_path_d(d1, d2, vars) {
                                // Emit merged path: child's attrs + combined d
                                let mut merged_attrs = attrs1_nd.clone();
                                merged_attrs.push(("d".to_string(), combined));
                                emit_synthetic_path(
                                    &merged_attrs, out, opts, id_map, vars, depth + 1,
                                );
                                stats.paths_combined += 1;
                                skip_next = true;
                                continue;
                            }
                        }
                    }
                }
                serialize_element(&child, out, opts, id_map, grad_dup_ids, vars, stats, depth + 1);
            }
            NodeType::Text => emit_text(child.text().unwrap_or(""), out, opts, depth + 1),
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
                    out.push_str(&format!("<?{} {}?>", pi.target, pi.value.unwrap_or("")));
                    maybe_newline(out, opts);
                }
            }
            _ => {}
        }
    }
}

/// Emit a synthetic `<path>` element from a flat attribute list.
/// Used when two paths have been combined into one.
fn emit_synthetic_path(
    attrs: &[(String, String)],
    out: &mut String,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    vars: &[CapturedVar],
    depth: usize,
) {
    indent(out, opts, depth);
    out.push_str("<path");
    // Sort for determinism, d last for readability
    let mut sorted = attrs.to_vec();
    sorted.sort_by(|a, b| {
        if a.0 == "d" { std::cmp::Ordering::Greater }
        else if b.0 == "d" { std::cmp::Ordering::Less }
        else { a.0.cmp(&b.0) }
    });
    for (k, v) in &sorted {
        let v_mapped = if (opts.strip_ids || opts.shorten_ids) && v.contains('#') {
            remap_id_refs(v, id_map)
        } else {
            v.clone()
        };
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&escape_xml_attr(&v_mapped));
        out.push('"');
    }
    out.push_str("/>");
    maybe_newline(out, opts);
    let _ = vars;
}

fn serialize_children_with_grouping(
    node: &Node,
    out: &mut String,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    grad_dup_ids: &HashSet<String>,
    vars: &[CapturedVar],
    stats: &mut OptimizeStats,
    depth: usize,
) {
    let mut fragments: Vec<ElementFragment> = Vec::new();

    let flush = |frags: &mut Vec<ElementFragment>, out: &mut String| {
        if !frags.is_empty() {
            let unit = make_indent_unit(opts);
            out.push_str(&group_runs(frags, &unit, depth + 1, opts.no_line_breaks));
            frags.clear();
        }
    };

    for child in node.children() {
        match child.node_type() {
            NodeType::Element => {
                let all_attrs = collect_all_attrs(&child, opts, id_map, vars);
                let mut frag_text = String::new();
                serialize_element(&child, &mut frag_text, opts, id_map, grad_dup_ids, vars, stats, depth + 1);
                fragments.push(ElementFragment::new(
                    child.tag_name().name(), frag_text, &all_attrs, vars,
                ));
            }
            NodeType::Text => {
                flush(&mut fragments, out);
                emit_text(child.text().unwrap_or(""), out, opts, depth + 1);
            }
            NodeType::Comment => {
                if !opts.strip_comments {
                    flush(&mut fragments, out);
                    indent(out, opts, depth + 1);
                    write_comment(child.text().unwrap_or(""), out);
                    maybe_newline(out, opts);
                }
            }
            _ => {}
        }
    }
    flush(&mut fragments, out);
}

fn collect_all_attrs(
    node: &Node,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    vars: &[CapturedVar],
) -> Vec<(String, String)> {
    let mut attrs = gather_attrs(node, opts, id_map, vars);
    let (style_xml, _) = process_style(node, opts, vars);
    for (k, v) in style_xml {
        if !attrs.iter().any(|(n, _)| n == &k) { attrs.push((k, v)); }
    }
    attrs
}

fn make_indent_unit(opts: &ScourOptions) -> String {
    if opts.no_line_breaks || opts.indent == Indent::None { return String::new(); }
    let unit = match opts.indent { Indent::Tab => "\t", _ => " " };
    unit.repeat(opts.nindent as usize)
}

// ─── Group Collapsing ─────────────────────────────────────────────────────────

fn can_collapse_group(node: &Node, opts: &ScourOptions) -> bool {
    if node.tag_name().name() != "g" { return false; }
    if node.attribute("id").is_some() { return false; }
    if node.attribute("class").is_some() { return false; }
    if node.attribute("transform").is_some() { return false; }
    if let Some(style) = node.attribute("style") {
        let decls = parse_style(style);
        if decls.iter().any(|(k, v)| !is_default_value(k, v)) { return false; }
    }
    let has_meaningful = node.attributes().any(|a| {
        let n = a.name();
        let ns = a.namespace().unwrap_or("");
        !matches!(n, "id" | "style" | "class")
            && (ns.is_empty() || (!opts.keep_editor_data && !is_editor_ns(ns)))
    });
    if has_meaningful { return false; }
    if node.parent().map(|p| p.tag_name().name() == "defs").unwrap_or(false) {
        return false;
    }
    true
}

// ─── ID Resolution ────────────────────────────────────────────────────────────

fn resolve_id(
    node: &Node,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    vars: &[CapturedVar],
) -> Option<String> {
    let raw = node.attribute("id")?;
    // If the id value contains a placeholder, preserve it verbatim
    if value_has_subst(raw, vars) { return Some(raw.to_string()); }
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

// ─── Style Processing ─────────────────────────────────────────────────────────

fn process_style(
    node: &Node,
    opts: &ScourOptions,
    vars: &[CapturedVar],
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
            .filter(|(k, v)| value_has_subst(v, vars) || !is_default_value(k, v))
            .collect()
    } else {
        Vec::new()
    };

    decls.retain(|(k, v)| value_has_subst(v, vars) || !is_default_value(k, v));
    let remaining = if decls.is_empty() { None } else { Some(serialize_style(&decls)) };
    (presentation_xml, remaining)
}

// ─── Attribute Gathering & Optimization ──────────────────────────────────────

const XML_NS:   &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

fn gather_attrs(
    node: &Node,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    vars: &[CapturedVar],
) -> Vec<(String, String)> {
    let local_name = node.tag_name().name();
    let mut attrs: Vec<(String, String)> = Vec::new();
    for attr in node.attributes() {
        let aname = attr.name();
        let ans   = attr.namespace().unwrap_or("");
        if aname == "id" && ans.is_empty() { continue; }
        if aname == "style" && ans.is_empty() { continue; }
        if ans == XMLNS_NS || aname == "xmlns" || aname.starts_with("xmlns:") { continue; }
        if !opts.keep_editor_data && !ans.is_empty() && is_editor_ns(ans) { continue; }
        if aname == "space" && ans == XML_NS && opts.strip_xml_space { continue; }

        let val = attr.value();
        let qname = if ans.is_empty() {
            aname.to_string()
        } else {
            match find_ns_prefix(node, ans) {
                Some(p) if !p.is_empty() => format!("{}:{}", p, aname),
                _ => aname.to_string(),
            }
        };
        let optimized = optimize_attr(aname, val, local_name, opts, id_map, vars);
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
    vars: &[CapturedVar],
) -> String {
    // Always remap ID references first (works on placeholders too — only the
    // embedded normal-ID portions are remapped, placeholders are inert strings)
    let remapped = remap_id_refs(val, id_map);

    // If the value (after ID remapping) still contains a placeholder,
    // don't apply any further transformation — the placeholder encodes
    // a neutral value but we must preserve it for restoration.
    if value_has_subst(&remapped, vars) {
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
        | "cx" | "cy" | "r" | "rx" | "ry" | "fx" | "fy" | "offset"
        | "stroke-width" | "stroke-miterlimit" | "stroke-dashoffset"
        | "font-size" | "letter-spacing" | "word-spacing" | "kerning"
        | "opacity" | "fill-opacity" | "stroke-opacity" | "stop-opacity" | "flood-opacity"
        | "k" | "k1" | "k2" | "k3" | "k4"
        | "amplitude" | "exponent" | "intercept" | "slope"
        | "specularConstant" | "specularExponent" | "diffuseConstant"
        | "surfaceScale" | "seed" | "numOctaves" => optimize_number(&remapped, opts.precision),
        "width" | "height" if element != "svg" => optimize_number(&remapped, opts.precision),
        "viewBox" | "points" | "stdDeviation" | "baseFrequency"
        | "kernelMatrix" | "tableValues" | "values" | "keyTimes" | "keySplines" | "order" => {
            optimize_number_list(&remapped, opts.precision)
        }
        _ => remapped,
    }
}

// ─── Viewboxing ───────────────────────────────────────────────────────────────

fn apply_viewboxing(
    node: &Node,
    local: &str,
    opts: &ScourOptions,
    xml_attrs: &mut Vec<(String, String)>,
    vars: &[CapturedVar],
) {
    if !opts.enable_viewboxing || local != "svg" || node.attribute("viewBox").is_some() {
        return;
    }
    let w = node.attribute("width");
    let h = node.attribute("height");
    if let (Some(w), Some(h)) = (w, h) {
        // Don't viewbox if width/height contain placeholders
        if value_has_subst(w, vars) || value_has_subst(h, vars) { return; }
        if let (Some(wv), Some(hv)) = (strip_units(w), strip_units(h)) {
            let viewbox = format!("0 0 {} {}", wv, hv);
            for a in xml_attrs.iter_mut() {
                if a.0 == "width"  { a.1 = "100%".to_string(); }
                if a.0 == "height" { a.1 = "100%".to_string(); }
            }
            xml_attrs.push(("viewBox".to_string(), viewbox));
        }
    }
}

// ─── Namespace Helpers ────────────────────────────────────────────────────────

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
        match current.parent() { Some(p) => current = p, None => break }
    }
    None
}

fn qualified_name(local: &str, ns: &str, node: &Node) -> String {
    if ns.is_empty() || ns == "http://www.w3.org/2000/svg" { return local.to_string(); }
    match find_ns_prefix(node, ns) {
        Some(pfx) if !pfx.is_empty() => format!("{}:{}", pfx, local),
        _ => local.to_string(),
    }
}

fn emit_ns_decls(node: &Node, out: &mut String, opts: &ScourOptions) {
    for attr in node.attributes() {
        let aname   = attr.name();
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
    let ns    = node.tag_name().namespace().unwrap_or("");
    if !opts.keep_editor_data && !ns.is_empty() && is_editor_ns(ns) { return false; }
    match local {
        "title"    if opts.remove_titles       => false,
        "desc"     if opts.remove_descriptions => false,
        "metadata" if opts.remove_metadata     => false,
        _ => true,
    }
}

// ─── ID Reference Remapping ───────────────────────────────────────────────────

fn remap_id_refs(val: &str, id_map: &HashMap<String, Option<String>>) -> String {
    if id_map.is_empty() || !val.contains('#') { return val.to_string(); }
    let s1 = remap_url_refs(val, id_map);
    remap_href_refs(&s1, id_map)
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
            && (chars[i+1] == '"' || chars[i+1] == '\'')
            && chars[i+2] == '#'
        {
            let q = chars[i+1];
            out.push('='); out.push(q); out.push('#');
            i += 3;
            let mut id = String::new();
            while i < n && chars[i] != q { id.push(chars[i]); i += 1; }
            match id_map.get(id.as_str()) {
                Some(Some(new_id)) => out.push_str(new_id),
                _ => out.push_str(&id),
            }
        } else { out.push(chars[i]); i += 1; }
    }
    out
}

// ─── Numeric Helpers ──────────────────────────────────────────────────────────

fn optimize_number(val: &str, precision: u8) -> String {
    let trimmed = val.trim();
    let (num, unit) = split_number_unit(trimmed);
    match num.parse::<f64>() {
        Ok(n) => format!("{}{}", fmt_f64(round_to_sig(n, precision as usize)), unit),
        Err(_) => val.to_string(),
    }
}

fn optimize_number_list(val: &str, precision: u8) -> String {
    let sep = if val.contains(',') { "," } else { " " };
    val.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| optimize_number(s, precision))
        .collect::<Vec<_>>()
        .join(sep)
}

fn split_number_unit(s: &str) -> (&str, &str) {
    const UNITS: &[&str] = &["px","pt","pc","mm","cm","in","em","ex","%"];
    for u in UNITS {
        if s.ends_with(u) {
            let num = &s[..s.len()-u.len()];
            if num.parse::<f64>().is_ok() { return (num, u); }
        }
    }
    (s, "")
}

pub fn round_to_sig(v: f64, sig: usize) -> f64 {
    if v == 0.0 || sig == 0 { return 0.0; }
    let mag = v.abs().log10().floor() as i32;
    let factor = 10f64.powi(sig as i32 - 1 - mag);
    (v * factor).round() / factor
}

fn fmt_f64(v: f64) -> String {
    if v.is_nan() || v.is_infinite() { return "0".to_string(); }
    if v.fract() == 0.0 && v.abs() < 1e15 { return format!("{}", v as i64); }
    let s = format!("{:.10}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if let Some(r) = s.strip_prefix("-0.") { format!("-.{}", r) }
    else if let Some(r) = s.strip_prefix("0.") { format!(".{}", r) }
    else { s.to_string() }
}

// ─── Output Helpers ───────────────────────────────────────────────────────────

fn write_attr(
    out: &mut String,
    name: &str,
    val: &str,
    opts: &ScourOptions,
    id_map: &HashMap<String, Option<String>>,
    _vars: &[CapturedVar],
) {
    let v = if (opts.strip_ids || opts.shorten_ids) && val.contains('#') {
        remap_id_refs(val, id_map)
    } else {
        val.to_string()
    };
    // Don't escape inside placeholders — but since placeholders use only
    // alphanumeric + `_`, escape_xml_attr is safe to run over the whole string.
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    out.push_str(&escape_xml_attr(&v));
    out.push('"');
}

fn emit_text(text: &str, out: &mut String, opts: &ScourOptions, depth: usize) {
    let t = if opts.no_line_breaks { collapse_ws(text) } else { text.to_string() };
    if !t.trim().is_empty() {
        indent(out, opts, depth);
        out.push_str(&escape_xml_text(&t));
        maybe_newline(out, opts);
    }
}

fn write_comment(text: &str, out: &mut String) {
    out.push_str("<!--"); out.push_str(text); out.push_str("-->");
}

fn maybe_newline(out: &mut String, opts: &ScourOptions) {
    if !opts.no_line_breaks { out.push('\n'); }
}

fn indent(out: &mut String, opts: &ScourOptions, depth: usize) {
    if opts.no_line_breaks || opts.indent == Indent::None || depth == 0 { return; }
    let unit = match opts.indent { Indent::Tab => "\t", _ => " " };
    for _ in 0..depth * opts.nindent as usize { out.push_str(unit); }
}

fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;")
     .replace('>', "&gt;").replace('"', "&quot;")
}

fn escape_xml_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_units(val: &str) -> Option<String> {
    const UNITS: &[&str] = &["px","pt","pc","mm","cm","in","em","ex"];
    let mut v = val.trim();
    for u in UNITS {
        if v.ends_with(u) {
            let c = &v[..v.len()-u.len()];
            if c.parse::<f64>().is_ok() { v = c; break; }
        }
    }
    if v.parse::<f64>().is_ok() { Some(v.to_string()) } else { None }
}
