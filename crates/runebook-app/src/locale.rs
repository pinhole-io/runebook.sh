//! Window text for each locale.
//!
//! Each language is one JSON file under `locales/`.
//! A new language is a new file plus one entry in [`CATALOGS`].
//! The active code lives in a process lock so render code can read it.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

struct Catalog {
    code: &'static str,
    raw: &'static str,
}

const CATALOGS: &[Catalog] = &[Catalog {
    code: "en",
    raw: include_str!("../../../locales/en.json"),
}];

static CURRENT: RwLock<&'static str> = RwLock::new("en");

struct Tables {
    by_code: HashMap<&'static str, HashMap<String, String>>,
}

/// One language the Settings control can show.
pub(crate) struct Choice {
    pub(crate) code: &'static str,
    pub(crate) name: &'static str,
}

/// Use `code` when that catalog exists. An unknown code uses English.
///
/// Returns the code that is now active.
pub(crate) fn activate(code: &str) -> &'static str {
    let code = known(code);
    *CURRENT.write().unwrap_or_else(|err| err.into_inner()) = code;
    code
}

/// Text for `key` in the active locale.
///
/// A missing key uses the English value. A key missing from English returns `key`.
pub(crate) fn text(key: &'static str) -> &'static str {
    resolve(current(), key)
}

/// Replace each `{name}` in the active string for `key`.
///
/// A value is inserted as text. The inserted text is not scanned.
pub(crate) fn format(key: &'static str, parts: &[(&str, &str)]) -> String {
    fill(text(key), parts)
}

/// Languages shipped with the app, in catalog order.
pub(crate) fn choices() -> Vec<Choice> {
    CATALOGS
        .iter()
        .map(|catalog| Choice {
            code: catalog.code,
            name: resolve(catalog.code, "locale.name"),
        })
        .collect()
}

fn current() -> &'static str {
    *CURRENT.read().unwrap_or_else(|err| err.into_inner())
}

fn known(code: &str) -> &'static str {
    CATALOGS
        .iter()
        .find(|catalog| catalog.code == code)
        .map(|catalog| catalog.code)
        .unwrap_or("en")
}

fn resolve(code: &str, key: &'static str) -> &'static str {
    resolve_in(tables(), code, key)
}

fn resolve_in<'a>(tables: &'a Tables, code: &str, key: &'a str) -> &'a str {
    if let Some(value) = lookup(tables, code, key) {
        return value;
    }
    if code != "en"
        && let Some(value) = lookup(tables, "en", key)
    {
        return value;
    }
    key
}

fn lookup<'a>(tables: &'a Tables, code: &str, key: &str) -> Option<&'a str> {
    tables
        .by_code
        .get(code)
        .and_then(|table| table.get(key))
        .map(String::as_str)
}

fn fill(template: &str, parts: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push('{');
            out.push_str(after);
            return out;
        };
        let name = &after[..end];
        if let Some((_, value)) = parts.iter().find(|(key, _)| *key == name) {
            out.push_str(value);
        } else {
            out.push('{');
            out.push_str(name);
            out.push('}');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut by_code = HashMap::new();
        for catalog in CATALOGS {
            let parsed: HashMap<String, String> = serde_json::from_str(catalog.raw)
                .unwrap_or_else(|err| panic!("locale {} is invalid JSON: {err}", catalog.code));
            by_code.insert(catalog.code, parsed);
        }
        Tables { by_code }
    })
}

#[cfg(test)]
mod tests {
    use super::{CATALOGS, Tables, activate, current, fill, known, resolve, tables};

    #[test]
    fn shipped_catalogs_share_keys_with_english() {
        let english = tables().by_code.get("en").expect("en");
        assert!(english.contains_key("locale.name"));
        assert!(!english.is_empty());
        for catalog in CATALOGS {
            let table = tables().by_code.get(catalog.code).expect(catalog.code);
            assert_eq!(table.len(), english.len(), "{}", catalog.code);
            for key in english.keys() {
                assert!(table.contains_key(key), "{} missing {key}", catalog.code);
            }
        }
    }

    #[test]
    fn a_missing_key_uses_english() {
        let mut english = std::collections::HashMap::new();
        english.insert("settings.done".to_string(), "Done".to_string());
        let mut other = std::collections::HashMap::new();
        other.insert("locale.name".to_string(), "Test".to_string());
        let mut by_code = std::collections::HashMap::new();
        by_code.insert("en", english);
        by_code.insert("xx", other);
        let tables = Tables { by_code };
        assert_eq!(super::resolve_in(&tables, "xx", "settings.done"), "Done");
        assert_eq!(super::resolve_in(&tables, "xx", "locale.name"), "Test");
        assert_eq!(super::resolve_in(&tables, "xx", "missing"), "missing");
    }

    #[test]
    fn format_inserts_a_value_without_scanning_it() {
        let text = fill("Delete note {id}?", &[("id", "a {id}")]);
        assert_eq!(text, "Delete note a {id}?");
    }

    #[test]
    fn an_unknown_code_uses_english() {
        assert_eq!(known("zz"), "en");
        assert_eq!(activate("zz"), "en");
        assert_eq!(current(), "en");
        assert_eq!(resolve("zz", "locale.name"), "English");
    }
}
