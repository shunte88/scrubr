/// ID management utilities for the SVG optimizer.
/// IDs that appear inside substitution variables ({{id}}) are never touched.

use std::collections::{HashMap, HashSet};

/// Generate a shortened ID from a counter. Uses base-52 (a-z, A-Z) then extends.
pub fn short_id(n: usize, prefix: Option<&str>) -> String {
    let alphabet: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".chars().collect();
    let base = alphabet.len();
    let mut result = Vec::new();
    let mut n = n;
    loop {
        result.push(alphabet[n % base]);
        n /= base;
        if n == 0 { break; }
        n -= 1;
    }
    result.reverse();
    let s: String = result.into_iter().collect();
    match prefix {
        Some(p) => format!("{}{}", p, s),
        None => s,
    }
}

/// Check whether an ID should be protected based on options
pub fn should_protect_id(
    id: &str,
    protect_noninkscape: bool,
    protect_list: &[String],
    protect_prefix: Option<&str>,
) -> bool {
    // Protect IDs within substitution variables (handled at a higher level,
    // but also guard here)
    if id.contains("{{") || id.contains("}}") {
        return true;
    }

    // Protect if in explicit list
    if protect_list.iter().any(|p| p == id) {
        return true;
    }

    // Protect if matching prefix
    if let Some(pfx) = protect_prefix {
        if id.starts_with(pfx) {
            return true;
        }
    }

    // Protect IDs not ending in a digit (Inkscape convention flag)
    if protect_noninkscape {
        if !id.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            return true;
        }
    }

    false
}

/// Build a rename map for all IDs that should be shortened.
/// `ids_in_use` = all IDs defined in the SVG.
/// `referenced_ids` = IDs that are actually referenced (url(#...), xlink:href, etc.)
pub fn build_id_map(
    ids_in_use: &[String],
    referenced_ids: &HashSet<String>,
    strip_unreferenced: bool,
    shorten: bool,
    shorten_prefix: Option<&str>,
    protect_noninkscape: bool,
    protect_list: &[String],
    protect_prefix: Option<&str>,
) -> (HashMap<String, Option<String>>, usize) {
    // Returns: map of old_id -> Some(new_id) | None (= remove)
    // Also returns count of IDs removed

    let mut map: HashMap<String, Option<String>> = HashMap::new();
    let mut counter = 0usize;
    let mut removed = 0usize;

    for id in ids_in_use {
        if should_protect_id(id, protect_noninkscape, protect_list, protect_prefix) {
            // Keep as-is
            map.insert(id.clone(), Some(id.clone()));
            continue;
        }

        let is_referenced = referenced_ids.contains(id.as_str());

        if strip_unreferenced && !is_referenced {
            map.insert(id.clone(), None);
            removed += 1;
            continue;
        }

        if shorten {
            let new_id = loop {
                let candidate = short_id(counter, shorten_prefix);
                counter += 1;
                // Ensure not colliding with existing IDs
                if !ids_in_use.contains(&candidate) {
                    break candidate;
                }
            };
            map.insert(id.clone(), Some(new_id));
        } else {
            map.insert(id.clone(), Some(id.clone()));
        }
    }

    (map, removed)
}
