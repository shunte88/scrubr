/// ID management — shortening, stripping, and protection logic.

use std::collections::{HashMap, HashSet};

// ─── ID Generation ────────────────────────────────────────────────────────────

/// Generate the n-th short ID in the sequence a, b, …, z, A, B, …, Z, aa, ab, …
/// An optional prefix is prepended.
pub fn short_id(n: usize, prefix: Option<&str>) -> String {
    const ALPHA: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let base = ALPHA.len();
    let mut result = Vec::new();
    let mut n = n;
    loop {
        result.push(ALPHA[n % base] as char);
        n /= base;
        if n == 0 { break; }
        n -= 1; // convert from zero-indexed base to "no zero digit" base
    }
    result.reverse();
    let s: String = result.into_iter().collect();
    match prefix {
        Some(p) => format!("{}{}", p, s),
        None => s,
    }
}

// ─── Protection Check ─────────────────────────────────────────────────────────

/// Return true if this ID should never be removed or renamed.
fn should_protect(
    id: &str,
    protect_noninkscape: bool,
    protect_list: &HashSet<String>,
    protect_prefix: Option<&str>,
) -> bool {
    if protect_list.contains(id) {
        return true;
    }
    if let Some(pfx) = protect_prefix {
        if id.starts_with(pfx) {
            return true;
        }
    }
    // Inkscape convention: IDs not ending in a digit are "semantic" names
    if protect_noninkscape {
        if !id.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            return true;
        }
    }
    false
}

// ─── Build Rename Map ─────────────────────────────────────────────────────────

/// Build a map from every defined ID to its new value:
///   - `Some(same_id)` — keep as-is
///   - `Some(new_id)`  — rename to new_id
///   - `None`          — remove (strip)
///
/// Returns `(map, count_removed)`.
pub fn build_id_map(
    all_ids: &[String],
    referenced_ids: &HashSet<String>,
    strip_unreferenced: bool,
    shorten: bool,
    shorten_prefix: Option<&str>,
    protect_noninkscape: bool,
    protect_list: &[String],
    protect_prefix: Option<&str>,
) -> (HashMap<String, Option<String>>, usize) {
    // Build a fast-lookup set of protect_list
    let protect_set: HashSet<String> = protect_list.iter().cloned().collect();

    // Set of all currently-defined IDs for collision avoidance
    let existing: HashSet<&str> = all_ids.iter().map(|s| s.as_str()).collect();

    let mut map: HashMap<String, Option<String>> = HashMap::new();
    let mut counter = 0usize;
    let mut removed = 0usize;

    for id in all_ids {
        let protected = should_protect(
            id,
            protect_noninkscape,
            &protect_set,
            protect_prefix,
        );

        if protected {
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
            // Find next short ID not already used in the document
            let new_id = loop {
                let candidate = short_id(counter, shorten_prefix);
                counter += 1;
                if !existing.contains(candidate.as_str()) {
                    break candidate;
                }
            };
            map.insert(id.clone(), Some(new_id));
        } else {
            // No shortening — keep as-is
            map.insert(id.clone(), Some(id.clone()));
        }
    }

    (map, removed)
}
