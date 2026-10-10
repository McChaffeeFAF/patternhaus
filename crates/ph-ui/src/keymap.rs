//! The keymap: a TOML file of `key -> command` bindings (DESIGN §9).
//!
//! Defaults follow FL Studio's functional shortcuts. Users override them with a file in their
//! config folder; every binding is remappable. Parsing and lookup are pure and tested without a
//! window.

use egui::{Key, Modifiers};
use serde::Deserialize;
use serde_json::Value;
use std::fmt;

pub const DEFAULT_TOML: &str = include_str!("../keymap.default.toml");

/// Where a binding applies. Panel contexts win over `Global` while the pointer is over them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Context {
    #[default]
    Global,
    PianoRoll,
    Playlist,
    ChannelRack,
    Mixer,
}

/// A key plus modifiers. `ctrl` means the platform command key (Cmd on macOS).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Chord {
    pub fn matches(&self, key: Key, m: &Modifiers) -> bool {
        self.key == key && self.ctrl == m.command && self.shift == m.shift && self.alt == m.alt
    }

    pub fn parse(text: &str) -> Result<Chord, String> {
        let mut chord = Chord {
            key: Key::A,
            ctrl: false,
            shift: false,
            alt: false,
        };
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" | "command" | "control" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" | "option" => chord.alt = true,
                _ => {
                    if key.is_some() {
                        return Err(format!("{text:?}: more than one key"));
                    }
                    key = Some(
                        key_from_name(part)
                            .ok_or_else(|| format!("{text:?}: unknown key {part:?}"))?,
                    );
                }
            }
        }
        chord.key = key.ok_or_else(|| format!("{text:?}: no key"))?;
        Ok(chord)
    }
}

fn key_from_name(name: &str) -> Option<Key> {
    match name.to_ascii_lowercase().as_str() {
        "esc" => Some(Key::Escape),
        "del" => Some(Key::Delete),
        "return" => Some(Key::Enter),
        "left" => Some(Key::ArrowLeft),
        "right" => Some(Key::ArrowRight),
        "up" => Some(Key::ArrowUp),
        "down" => Some(Key::ArrowDown),
        "pgup" => Some(Key::PageUp),
        "pgdn" => Some(Key::PageDown),
        _ => Key::from_name(name).or_else(|| {
            // case-insensitive match on egui's own names ("space" -> Space, "f5" -> F5)
            Key::ALL
                .iter()
                .copied()
                .find(|k| format!("{k:?}").eq_ignore_ascii_case(name))
        }),
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            write!(f, "Ctrl+")?;
        }
        if self.shift {
            write!(f, "Shift+")?;
        }
        if self.alt {
            write!(f, "Alt+")?;
        }
        write!(f, "{:?}", self.key)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub chord: Chord,
    pub context: Context,
    /// A registry command id (`transport.toggle`) or an `app.*` UI action.
    pub command: String,
    pub params: Value,
}

impl Binding {
    pub fn is_app_action(&self) -> bool {
        self.command.starts_with("app.")
    }
}

#[derive(Deserialize)]
struct RawBind {
    key: String,
    command: String,
    #[serde(default)]
    params: Option<toml::Value>,
    #[serde(default)]
    context: Context,
}

#[derive(Deserialize)]
struct RawFile {
    #[serde(default)]
    bind: Vec<RawBind>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
}

fn toml_to_json(v: toml::Value) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

impl Keymap {
    pub fn parse(text: &str) -> Result<Keymap, String> {
        let raw: RawFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut bindings = Vec::new();
        for b in raw.bind {
            bindings.push(Binding {
                chord: Chord::parse(&b.key)?,
                context: b.context,
                command: b.command,
                params: b.params.map(toml_to_json).unwrap_or(Value::Null),
            });
        }
        Ok(Keymap { bindings })
    }

    pub fn defaults() -> Keymap {
        Self::parse(DEFAULT_TOML).expect("built-in keymap parses")
    }

    /// Defaults with a user file layered on top: same chord and context replaces; an empty
    /// command unbinds.
    pub fn with_user(mut self, user: Keymap) -> Keymap {
        for u in user.bindings {
            self.bindings
                .retain(|b| !(b.chord == u.chord && b.context == u.context));
            if !u.command.is_empty() {
                self.bindings.push(u);
            }
        }
        self
    }

    /// The binding for a key press: the panel's own context first, then global.
    pub fn lookup(&self, key: Key, mods: &Modifiers, panel: Context) -> Option<&Binding> {
        let find = |ctx| {
            self.bindings
                .iter()
                .find(|b| b.context == ctx && b.chord.matches(key, mods))
        };
        if panel != Context::Global
            && let Some(b) = find(panel)
        {
            return Some(b);
        }
        find(Context::Global)
    }

    /// Rebinds `command` (with `params`) in `context` to a new chord, replacing whatever the
    /// chord did there.
    pub fn rebind(&mut self, old: &Binding, new_chord: Chord) {
        self.bindings
            .retain(|b| !(b.chord == new_chord && b.context == old.context));
        if let Some(b) = self
            .bindings
            .iter_mut()
            .find(|b| b.chord == old.chord && b.context == old.context && b.command == old.command)
        {
            b.chord = new_chord;
        }
    }

    /// Serialises to the TOML file format (what a user keymap file looks like).
    pub fn to_toml(&self) -> String {
        let mut s = String::from(
            "# Patternhaus keymap (generated). See keymap.default.toml for the format.\n",
        );
        for b in &self.bindings {
            s.push_str("\n[[bind]]\n");
            s.push_str(&format!(
                "key = \"{}\"\ncommand = \"{}\"\n",
                b.chord, b.command
            ));
            if !b.params.is_null()
                && let Ok(t) = toml::to_string(&b.params)
            {
                let inline = t.trim().replace('\n', ", ");
                s.push_str(&format!("params = {{ {inline} }}\n"));
            }
            if b.context != Context::Global {
                let c = match b.context {
                    Context::PianoRoll => "piano_roll",
                    Context::Playlist => "playlist",
                    Context::ChannelRack => "channel_rack",
                    Context::Mixer => "mixer",
                    Context::Global => "global",
                };
                s.push_str(&format!("context = \"{c}\"\n"));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, shift: bool, alt: bool) -> Modifiers {
        Modifiers {
            alt,
            ctrl,
            shift,
            mac_cmd: false,
            command: ctrl,
        }
    }

    #[test]
    fn chords_parse_and_print() {
        let c = Chord::parse("ctrl+shift+z").unwrap();
        assert_eq!(
            c,
            Chord {
                key: Key::Z,
                ctrl: true,
                shift: true,
                alt: false
            }
        );
        assert_eq!(Chord::parse("F5").unwrap().key, Key::F5);
        assert_eq!(Chord::parse("Space").unwrap().key, Key::Space);
        assert_eq!(Chord::parse("Up").unwrap().key, Key::ArrowUp);
        assert!(Chord::parse("Ctrl+").is_err());
        assert!(Chord::parse("Ctrl+Nonsense").is_err());
        assert!(Chord::parse("A+B").is_err());
        assert_eq!(
            Chord::parse("Ctrl+Shift+Z").unwrap().to_string(),
            "Ctrl+Shift+Z"
        );
    }

    #[test]
    fn default_keymap_has_the_fl_shortcuts() {
        let k = Keymap::defaults();
        let look = |key, m: Modifiers, ctx| {
            k.lookup(key, &m, ctx)
                .map(|b| (b.command.clone(), b.params.clone()))
        };
        let none = mods(false, false, false);
        assert_eq!(
            look(Key::Space, none, Context::Global).unwrap().0,
            "transport.toggle"
        );
        for (key, panel) in [
            (Key::F5, "playlist"),
            (Key::F6, "channel_rack"),
            (Key::F7, "piano_roll"),
            (Key::F8, "browser"),
            (Key::F9, "mixer"),
        ] {
            let (cmd, params) = look(key, none, Context::Global).unwrap();
            assert_eq!(cmd, "app.toggle_panel");
            assert_eq!(params["panel"], panel);
        }
        assert_eq!(
            look(Key::Z, mods(true, false, false), Context::Global)
                .unwrap()
                .0,
            "history.undo"
        );
        assert_eq!(
            look(Key::Y, mods(true, false, false), Context::Global)
                .unwrap()
                .0,
            "history.redo"
        );
        assert_eq!(
            look(Key::S, mods(true, false, false), Context::Global)
                .unwrap()
                .0,
            "file.save"
        );
        assert_eq!(
            look(Key::L, mods(true, false, false), Context::Global)
                .unwrap()
                .0,
            "app.clone_pattern"
        );
        // Ctrl+B and Ctrl+Q mean something in the piano roll, nothing globally
        assert_eq!(
            look(Key::B, mods(true, false, false), Context::PianoRoll)
                .unwrap()
                .0,
            "app.roll.duplicate"
        );
        assert_eq!(
            look(Key::Q, mods(true, false, false), Context::PianoRoll)
                .unwrap()
                .0,
            "app.roll.quantize"
        );
        assert!(look(Key::B, mods(true, false, false), Context::Global).is_none());
        // the same chord in the playlist is the playlist's duplicate
        assert_eq!(
            look(Key::B, mods(true, false, false), Context::Playlist)
                .unwrap()
                .0,
            "app.playlist.duplicate"
        );
        // exact modifier match: Ctrl+Shift+Z is redo, Ctrl+Z is not
        assert_eq!(
            look(Key::Z, mods(true, true, false), Context::Global)
                .unwrap()
                .0,
            "history.redo"
        );
        assert!(look(Key::Z, mods(false, false, false), Context::Global).is_none());
    }

    #[test]
    fn every_default_command_is_a_registry_command_or_app_action() {
        let reg = ph_commands::Registry::with_builtins();
        let ids: Vec<String> = reg.list("").iter().map(|s| s.id.to_string()).collect();
        for b in &Keymap::defaults().bindings {
            assert!(
                b.is_app_action() || ids.contains(&b.command),
                "{} is neither an app action nor a registry command",
                b.command
            );
        }
    }

    #[test]
    fn user_file_overrides_and_unbinds() {
        let user = Keymap::parse(
            r#"
[[bind]]
key = "F5"
command = "app.toggle_panel"
params = { panel = "mixer" }

[[bind]]
key = "Space"
command = ""

[[bind]]
key = "Ctrl+Shift+K"
command = "transport.stop"
"#,
        )
        .unwrap();
        let k = Keymap::defaults().with_user(user);
        let none = mods(false, false, false);
        assert_eq!(
            k.lookup(Key::F5, &none, Context::Global).unwrap().params["panel"],
            "mixer"
        );
        assert!(k.lookup(Key::Space, &none, Context::Global).is_none());
        assert_eq!(
            k.lookup(Key::K, &mods(true, true, false), Context::Global)
                .unwrap()
                .command,
            "transport.stop"
        );
        // untouched defaults survive
        assert!(k.lookup(Key::F9, &none, Context::Global).is_some());
    }

    #[test]
    fn rebinding_moves_a_binding() {
        let mut k = Keymap::defaults();
        let old = k
            .bindings
            .iter()
            .find(|b| b.command == "transport.toggle")
            .unwrap()
            .clone();
        k.rebind(&old, Chord::parse("Enter").unwrap());
        let none = mods(false, false, false);
        assert!(k.lookup(Key::Space, &none, Context::Global).is_none());
        assert_eq!(
            k.lookup(Key::Enter, &none, Context::Global)
                .unwrap()
                .command,
            "transport.toggle"
        );
    }

    #[test]
    fn serialised_keymap_round_trips() {
        let k = Keymap::defaults();
        let text = k.to_toml();
        let back = Keymap::parse(&text).unwrap();
        assert_eq!(k, back);
    }

    #[test]
    fn bad_files_are_reported() {
        assert!(Keymap::parse("[[bind]]\nkey = \"Ctrl+Bogus\"\ncommand = \"x\"\n").is_err());
        assert!(Keymap::parse("not toml [").is_err());
    }
}
