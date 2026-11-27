//! Environment variables with the pre-rename fallback.
//!
//! The application's variables are `BEAST_STUDIO_*`. Before the rename (2026-11-27) they were
//! `PATTERNHAUS_*`; scripts that still set the old name keep working: the new name wins when
//! it is set at all, the old name is read otherwise.

use std::ffi::OsString;

/// The prefix of the application's variables.
pub const PREFIX: &str = "BEAST_STUDIO_";
/// The prefix they had before the rename; still read as a fallback.
pub const LEGACY_PREFIX: &str = "PATTERNHAUS_";

/// The pre-rename spelling of `name`, when `name` is one of the application's variables
/// (`BEAST_STUDIO_ASSETS` gives `PATTERNHAUS_ASSETS`).
pub fn legacy_name(name: &str) -> Option<String> {
    name.strip_prefix(PREFIX)
        .map(|rest| format!("{LEGACY_PREFIX}{rest}"))
}

/// [`var_os`] over any lookup (tests pass a map instead of touching the process environment).
pub fn var_os_with(name: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
    get(name).or_else(|| legacy_name(name).and_then(|old| get(&old)))
}

/// The value of `name` (a `BEAST_STUDIO_*` variable), else of its `PATTERNHAUS_*` spelling.
pub fn var_os(name: &str) -> Option<OsString> {
    var_os_with(name, |k| std::env::var_os(k))
}

/// [`var_os`] as a string; `None` when unset or not Unicode.
pub fn var(name: &str) -> Option<String> {
    var_os(name).and_then(|v| v.into_string().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn the_new_name_is_read() {
        let get = lookup(&[("BEAST_STUDIO_ASSETS", "new")]);
        assert_eq!(var_os_with("BEAST_STUDIO_ASSETS", get), Some("new".into()));
    }

    #[test]
    fn the_old_name_is_a_fallback() {
        let get = lookup(&[("PATTERNHAUS_ASSETS", "old")]);
        assert_eq!(var_os_with("BEAST_STUDIO_ASSETS", get), Some("old".into()));
    }

    #[test]
    fn the_new_name_wins_even_when_empty() {
        let get = lookup(&[("BEAST_STUDIO_ASSETS", ""), ("PATTERNHAUS_ASSETS", "old")]);
        assert_eq!(var_os_with("BEAST_STUDIO_ASSETS", get), Some("".into()));
    }

    #[test]
    fn unset_is_none_and_other_names_have_no_alias() {
        assert_eq!(var_os_with("BEAST_STUDIO_ASSETS", lookup(&[])), None);
        assert_eq!(legacy_name("HOME"), None);
        assert_eq!(
            legacy_name("BEAST_STUDIO_PLUGIN_SETTLE_SECONDS").as_deref(),
            Some("PATTERNHAUS_PLUGIN_SETTLE_SECONDS")
        );
    }
}
