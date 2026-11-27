//! The per-user folders (settings, remembered agent grants, the plugin catalogue, the plugin
//! scan cache, the projects folder) and their one-time migration from the names they had
//! before the rename to Beast Studio (2026-11-27).
//!
//! | what | Windows | macOS | Linux |
//! |---|---|---|---|
//! | settings, grants, catalogue | `%APPDATA%\Beast Studio` | `~/Library/Application Support/Beast Studio` | `$XDG_CONFIG_HOME/beast-studio`, catalogue in `$XDG_DATA_HOME/Beast Studio/catalogue` |
//! | plugin cache | `%LOCALAPPDATA%\Beast Studio` | `~/Library/Caches/Beast Studio` | `$XDG_CACHE_HOME/beast-studio` |
//!
//! Before the rename the same places were called `Patternhaus` / `patternhaus` (Windows and
//! macOS file systems do not tell the two spellings apart, so settings and catalogue shared one
//! folder there; on macOS the remembered grants lived in `~/.config/patternhaus`).
//!
//! # Migration
//!
//! [`migrate_user_dirs`] runs once at the start of a front end. For every pair of an old and a
//! new folder, [`migrate_dir`] **copies** the old folder recursively into the new place when
//! the new one does not exist yet and the old one does. It never moves or deletes anything: an
//! older build may still be running against the old folder. Nothing is overwritten, and a copy
//! that fails half way leaves no new folder behind (it is built under a temporary name and
//! renamed into place at the end).

use std::path::{Path, PathBuf};

/// The product name as it spells a folder (capitalised, with a space).
pub const NAME: &str = "Beast Studio";
/// The product name as it spells a folder on Linux config and cache paths, a socket or a pipe.
pub const SLUG: &str = "beast-studio";
/// The names before the rename.
pub const LEGACY_NAME: &str = "Patternhaus";
pub const LEGACY_SLUG: &str = "patternhaus";

/// An environment lookup: the process environment in production, a map in tests. Empty values
/// count as unset.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<PathBuf>;

/// The process environment, empty values filtered out.
pub fn process_env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Which spelling of the product name a folder is built with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Era {
    /// Beast Studio.
    Current,
    /// Patternhaus, before the rename.
    Legacy,
}

impl Era {
    fn name(self) -> &'static str {
        match self {
            Era::Current => NAME,
            Era::Legacy => LEGACY_NAME,
        }
    }
    fn slug(self) -> &'static str {
        match self {
            Era::Current => SLUG,
            Era::Legacy => LEGACY_SLUG,
        }
    }
}

/// The settings folder (layout, keymap, settings, learned MIDI mappings).
pub fn config_dir_in(env: Env<'_>, era: Era) -> Option<PathBuf> {
    if cfg!(windows) {
        env("APPDATA").map(|d| d.join(era.name()))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support").join(era.name()))
    } else {
        env("XDG_CONFIG_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".config")))
            .map(|d| d.join(era.slug()))
    }
}

/// The folder remembered per-client agent grants live in (`agents.json`). Like the settings
/// folder, except on macOS, where it has always been under `~/.config`.
pub fn grants_dir_in(env: Env<'_>, era: Era) -> Option<PathBuf> {
    if cfg!(windows) {
        config_dir_in(env, era)
    } else {
        env("XDG_CONFIG_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".config")))
            .map(|d| d.join(era.slug()))
    }
}

/// The data folder: the plugin catalogue lives in its `catalogue` subfolder.
pub fn data_dir_in(env: Env<'_>, era: Era) -> Option<PathBuf> {
    let base = if cfg!(windows) {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share")))
    };
    base.map(|d| d.join(era.name()))
}

/// The plugin catalogue folder: `$BEAST_STUDIO_CATALOGUE` (or the older `$PATTERNHAUS_CATALOGUE`)
/// when set and not empty, else `catalogue` in the data folder.
pub fn catalogue_dir_in(env: Env<'_>) -> Option<PathBuf> {
    let from_env = ph_node::env::var_os_with("BEAST_STUDIO_CATALOGUE", |k| {
        env(k).map(PathBuf::into_os_string)
    })
    .map(PathBuf::from)
    .filter(|p| !p.as_os_str().is_empty());
    from_env.or_else(|| data_dir_in(env, Era::Current).map(|d| d.join("catalogue")))
}

/// The plugin scan cache folder.
pub fn cache_dir_in(env: Env<'_>, era: Era) -> Option<PathBuf> {
    if cfg!(windows) {
        env("LOCALAPPDATA").map(|d| d.join(era.name()))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Caches").join(era.name()))
    } else {
        env("XDG_CACHE_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".cache")))
            .map(|d| d.join(era.slug()))
    }
}

/// The folders to migrate: (old, new), without repeats, in the order settings, grants, data,
/// cache. A folder pair is left out when either side is unknown.
pub fn legacy_pairs_in(env: Env<'_>) -> Vec<(PathBuf, PathBuf)> {
    let kinds: [fn(Env<'_>, Era) -> Option<PathBuf>; 4] =
        [config_dir_in, grants_dir_in, data_dir_in, cache_dir_in];
    let mut out: Vec<(PathBuf, PathBuf)> = Vec::new();
    for dir in kinds {
        if let (Some(old), Some(new)) = (dir(env, Era::Legacy), dir(env, Era::Current))
            && !out.iter().any(|(_, n)| *n == new)
        {
            out.push((old, new));
        }
    }
    out
}

/// What [`migrate_dir`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Migration {
    /// Nothing to do: there is no old folder, or the new one already exists.
    NotNeeded,
    /// The old folder was copied; this many files.
    Copied { files: usize },
    /// The copy failed; the new folder was not created.
    Failed(String),
}

/// Copies `old` recursively to `new`, once: only when `old` is a folder and `new` does not
/// exist. `old` is never changed, moved or deleted, and nothing in `new` is ever overwritten.
/// Links are not followed or copied. A failed copy leaves no `new` behind.
pub fn migrate_dir(old: &Path, new: &Path) -> Migration {
    if !old.is_dir() || new.exists() || new.starts_with(old) {
        return Migration::NotNeeded;
    }
    let Some(parent) = new.parent() else {
        return Migration::NotNeeded;
    };
    let leaf = new.file_name().map(|n| n.to_string_lossy().into_owned());
    let tmp = parent.join(format!(
        ".{}-migrating-{}",
        leaf.unwrap_or_default().replace(' ', "-"),
        std::process::id()
    ));
    let result = (|| -> std::io::Result<usize> {
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::remove_dir_all(&tmp); // a leftover of our own earlier failed attempt
        let n = copy_tree(old, &tmp)?;
        std::fs::rename(&tmp, new)?;
        Ok(n)
    })();
    match result {
        Ok(files) => Migration::Copied { files },
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Migration::Failed(format!(
                "could not copy {} to {}: {e}",
                old.display(),
                new.display()
            ))
        }
    }
}

/// Copies the files and folders of `from` into the new folder `to`; returns the file count.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(to)?;
    let mut files = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            files += copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
            files += 1;
        }
    }
    Ok(files)
}

/// Runs the migration for the real per-user folders. Front ends call it once at the start,
/// before anything reads a settings file. Failures are returned, never fatal.
pub fn migrate_user_dirs() -> Vec<Migration> {
    migrate_pairs(&legacy_pairs_in(&process_env))
}

/// [`migrate_dir`] for each pair.
pub fn migrate_pairs(pairs: &[(PathBuf, PathBuf)]) -> Vec<Migration> {
    pairs
        .iter()
        .map(|(old, new)| migrate_dir(old, new))
        .filter(|m| *m != Migration::NotNeeded)
        .collect()
}

/// Reports what [`migrate_user_dirs`] did on stderr: a line per copy, and per failure.
pub fn migrate_user_dirs_and_report() {
    for m in migrate_user_dirs() {
        match m {
            Migration::Copied { files } => {
                eprintln!("beast-studio: copied {files} files of your Patternhaus settings")
            }
            Migration::Failed(e) => eprintln!("beast-studio: settings not migrated: {e}"),
            Migration::NotNeeded => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn the_old_folder_is_copied_once_and_left_alone() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Patternhaus");
        let new = t.path().join("Beast Studio");
        write(&old.join("settings.toml"), "a = 1");
        write(&old.join("catalogue/synth.toml"), "id = 'x'");
        write(&old.join("deep/er/file.bin"), "zz");

        assert_eq!(migrate_dir(&old, &new), Migration::Copied { files: 3 });
        assert_eq!(
            std::fs::read_to_string(new.join("settings.toml")).unwrap(),
            "a = 1"
        );
        assert!(new.join("catalogue/synth.toml").is_file());
        assert!(new.join("deep/er/file.bin").is_file());
        // the old folder is untouched
        assert_eq!(
            std::fs::read_to_string(old.join("settings.toml")).unwrap(),
            "a = 1"
        );
        assert!(old.join("deep/er/file.bin").is_file());
        // no temporary folder is left beside it
        let names: Vec<_> = std::fs::read_dir(t.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn a_second_start_does_not_copy_again_or_overwrite_edits() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Patternhaus");
        let new = t.path().join("Beast Studio");
        write(&old.join("settings.toml"), "a = 1");
        assert_eq!(migrate_dir(&old, &new), Migration::Copied { files: 1 });
        write(&new.join("settings.toml"), "a = 2"); // edited in the new place
        write(&old.join("later.toml"), "b = 1"); // an old build keeps writing
        assert_eq!(migrate_dir(&old, &new), Migration::NotNeeded);
        assert_eq!(
            std::fs::read_to_string(new.join("settings.toml")).unwrap(),
            "a = 2"
        );
        assert!(!new.join("later.toml").exists());
    }

    #[test]
    fn an_existing_new_folder_is_never_touched() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Patternhaus");
        let new = t.path().join("Beast Studio");
        write(&old.join("settings.toml"), "old");
        std::fs::create_dir_all(&new).unwrap(); // even an empty one
        assert_eq!(migrate_dir(&old, &new), Migration::NotNeeded);
        assert_eq!(std::fs::read_dir(&new).unwrap().count(), 0);
    }

    #[test]
    fn no_old_folder_means_nothing_happens() {
        let t = tempfile::tempdir().unwrap();
        let new = t.path().join("Beast Studio");
        assert_eq!(
            migrate_dir(&t.path().join("Patternhaus"), &new),
            Migration::NotNeeded
        );
        assert!(!new.exists());
        // an old *file* of that name is not a folder to copy
        write(&t.path().join("patternhaus"), "x");
        assert_eq!(
            migrate_dir(&t.path().join("patternhaus"), &new),
            Migration::NotNeeded
        );
    }

    #[test]
    fn a_new_folder_inside_the_old_one_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Patternhaus");
        write(&old.join("a.toml"), "x");
        assert_eq!(
            migrate_dir(&old, &old.join("Beast Studio")),
            Migration::NotNeeded
        );
        assert!(!old.join("Beast Studio").exists());
    }

    #[test]
    fn a_failed_copy_leaves_no_new_folder() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Patternhaus");
        write(&old.join("a.toml"), "x");
        // the new folder's parent is a file: it cannot be created
        write(&t.path().join("blocker"), "i am a file");
        let m = migrate_dir(&old, &t.path().join("blocker/Beast Studio"));
        assert!(matches!(m, Migration::Failed(_)), "{m:?}");
        assert!(old.join("a.toml").is_file());
    }

    fn fake_env(root: &Path) -> impl Fn(&str) -> Option<PathBuf> {
        let m: HashMap<&str, PathBuf> = [
            ("APPDATA", root.join("appdata")),
            ("LOCALAPPDATA", root.join("local")),
            ("HOME", root.join("home")),
            ("XDG_CONFIG_HOME", root.join("xdg-config")),
            ("XDG_DATA_HOME", root.join("xdg-data")),
            ("XDG_CACHE_HOME", root.join("xdg-cache")),
        ]
        .into_iter()
        .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn the_pairs_name_the_old_and_the_new_folders_of_this_platform() {
        let root = Path::new("/r");
        let env = fake_env(root);
        let pairs = legacy_pairs_in(&env);
        let (old_cfg, new_cfg) = pairs[0].clone();
        assert!(
            old_cfg.to_string_lossy().contains("atternhaus"),
            "{old_cfg:?}"
        );
        assert!(new_cfg.to_string_lossy().contains("east"), "{new_cfg:?}");
        for (old, new) in &pairs {
            assert_ne!(old, new);
            assert!(!new.to_string_lossy().contains("atternhaus"), "{new:?}");
            assert!(!old.to_string_lossy().contains("east"), "{old:?}");
        }
        // every new folder is listed once
        let mut news: Vec<_> = pairs.iter().map(|(_, n)| n.clone()).collect();
        news.sort();
        news.dedup();
        assert_eq!(news.len(), pairs.len());
        if cfg!(windows) {
            // settings, grants and catalogue were one folder and stay one folder
            assert_eq!(pairs.len(), 2);
            assert_eq!(new_cfg, root.join("appdata").join("Beast Studio"));
            assert_eq!(old_cfg, root.join("appdata").join("Patternhaus"));
            assert_eq!(
                cache_dir_in(&env, Era::Current),
                Some(root.join("local").join("Beast Studio"))
            );
        } else if cfg!(target_os = "macos") {
            // settings and catalogue, the grants in ~/.config, the cache
            assert_eq!(pairs.len(), 3);
        } else {
            assert_eq!(pairs.len(), 3); // settings and grants, data, cache
            assert_eq!(new_cfg, root.join("xdg-config").join("beast-studio"));
            assert_eq!(old_cfg, root.join("xdg-config").join("patternhaus"));
            assert_eq!(
                data_dir_in(&env, Era::Current),
                Some(root.join("xdg-data").join("Beast Studio"))
            );
            assert_eq!(
                cache_dir_in(&env, Era::Legacy),
                Some(root.join("xdg-cache").join("patternhaus"))
            );
        }
    }

    #[test]
    fn the_catalogue_variable_wins_and_the_old_spelling_still_works() {
        let root = Path::new("/r");
        let base = fake_env(root);
        let with = |extra: &'static [(&'static str, &'static str)]| {
            let base = fake_env(root);
            move |k: &str| {
                extra
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| PathBuf::from(v))
                    .or_else(|| base(k))
            }
        };
        let default = catalogue_dir_in(&base).unwrap();
        assert!(default.ends_with("Beast Studio/catalogue"), "{default:?}");
        let new = with(&[("BEAST_STUDIO_CATALOGUE", "/new")]);
        assert_eq!(catalogue_dir_in(&new), Some(PathBuf::from("/new")));
        let old = with(&[("PATTERNHAUS_CATALOGUE", "/old")]);
        assert_eq!(catalogue_dir_in(&old), Some(PathBuf::from("/old")));
        let both = with(&[
            ("BEAST_STUDIO_CATALOGUE", "/new"),
            ("PATTERNHAUS_CATALOGUE", "/old"),
        ]);
        assert_eq!(catalogue_dir_in(&both), Some(PathBuf::from("/new")));
    }

    #[test]
    fn migrating_the_pairs_copies_every_old_folder_it_finds() {
        let t = tempfile::tempdir().unwrap();
        let env = fake_env(t.path());
        let pairs = legacy_pairs_in(&env);
        for (old, _) in &pairs {
            write(&old.join("marker.txt"), &old.to_string_lossy());
        }
        let done = migrate_pairs(&pairs);
        assert_eq!(done.len(), pairs.len(), "{done:?}");
        for (old, new) in &pairs {
            assert!(new.join("marker.txt").is_file());
            assert!(old.join("marker.txt").is_file(), "the old folder stays");
        }
        assert!(
            migrate_pairs(&pairs).is_empty(),
            "the second start copies nothing"
        );
    }
}
