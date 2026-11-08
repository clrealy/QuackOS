//! Carbs' configuration file.
//!
//! `$XDG_CONFIG_HOME/carbs/carbs.conf` (or `~/.config/carbs/carbs.conf`).
//! Deliberately boring: `key = value`, one per line. A line whose first non-blank
//! character is `#` is a comment; `#` anywhere else is part of the value, so
//! `background = #f5c740` means what it looks like.
//!
//! Anything missing or unparseable falls back to the default and logs a warning -
//! a typo in a config file must never stop the desktop from starting.

use std::{env, path::PathBuf};

/// Everything Carbs reads from disk at startup.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Command for Super+Return. Empty means "probe for one at startup".
    pub terminal: String,
    /// Command for Super+D.
    pub launcher: String,
    /// XKB layout, e.g. "us" or "gb" or "de,us".
    pub keyboard_layout: String,
    pub keyboard_variant: String,
    /// XKB options, e.g. "ctrl:nocaps".
    pub keyboard_options: Option<String>,
    /// Key repeat, in milliseconds before repeating and repeats per second.
    pub repeat_delay: i32,
    pub repeat_rate: i32,
    /// Cursor height in pixels at scale 1.
    pub cursor_size: u32,
    /// Desktop background, as premultiplied-ready linear RGBA.
    pub background: [f32; 4],
    /// Commands to run once Carbs is up (the Bread panel, a wallpaper setter, ...).
    pub autostart: Vec<String>,
}

/// QuackOS duck yellow.
pub const DEFAULT_BACKGROUND: [f32; 4] = [0.96, 0.78, 0.25, 1.0];

impl Default for Config {
    fn default() -> Self {
        Self {
            terminal: String::new(),
            launcher: String::new(),
            keyboard_layout: "us".into(),
            keyboard_variant: String::new(),
            keyboard_options: None,
            repeat_delay: 400,
            repeat_rate: 30,
            cursor_size: 24,
            background: DEFAULT_BACKGROUND,
            autostart: Vec::new(),
        }
    }
}

/// Terminals to try for Super+Return when the config doesn't name one.
pub const TERMINAL_CANDIDATES: &[&str] =
    &["foot", "alacritty", "kitty", "wezterm", "gnome-terminal", "xterm", "weston-terminal"];

/// Launchers to try for Super+D when the config doesn't name one.
/// `bread-launcher` is QuackOS' own; the rest are what people already have.
pub const LAUNCHER_CANDIDATES: &[&str] = &["bread-launcher", "fuzzel", "wofi", "rofi", "bemenu-run"];

impl Config {
    /// Where the config file lives, honouring XDG.
    pub fn path() -> Option<PathBuf> {
        if let Some(dir) = env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(dir).join("carbs/carbs.conf"));
        }
        env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(|home| PathBuf::from(home).join(".config/carbs/carbs.conf"))
    }

    /// Reads the config file if there is one. Never fails: a missing or broken
    /// file just means defaults.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let (config, problems) = Self::parse(&text);
                for problem in &problems {
                    tracing::warn!("{}: {problem}", path.display());
                }
                tracing::info!("read config from {}", path.display());
                config
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                tracing::warn!("couldn't read {}: {err}", path.display());
                Self::default()
            }
        }
    }

    /// Parses config text. Returns the config plus a list of complaints about
    /// lines that didn't make sense, so the caller can log them.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut config = Self::default();
        let mut problems = Vec::new();

        for (number, raw) in text.lines().enumerate() {
            let line = raw.trim();
            // Whole-line comments only: a value may legitimately contain '#'.
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                problems.push(format!("line {}: expected 'key = value', got {raw:?}", number + 1));
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let complain = |problems: &mut Vec<String>, what: &str| {
                problems.push(format!("line {}: {key} needs {what}, got {value:?}", number + 1));
            };

            match key {
                "terminal" => config.terminal = value.to_string(),
                "launcher" => config.launcher = value.to_string(),
                "keyboard_layout" => config.keyboard_layout = value.to_string(),
                "keyboard_variant" => config.keyboard_variant = value.to_string(),
                "keyboard_options" => {
                    config.keyboard_options = (!value.is_empty()).then(|| value.to_string())
                }
                "repeat_delay" => match value.parse::<i32>() {
                    Ok(v) if v >= 0 => config.repeat_delay = v,
                    _ => complain(&mut problems, "a whole number of milliseconds"),
                },
                "repeat_rate" => match value.parse::<i32>() {
                    Ok(v) if v >= 0 => config.repeat_rate = v,
                    _ => complain(&mut problems, "a whole number of repeats per second"),
                },
                "cursor_size" => match value.parse::<u32>() {
                    Ok(v) if (8..=512).contains(&v) => config.cursor_size = v,
                    _ => complain(&mut problems, "a size between 8 and 512"),
                },
                "background" => match parse_colour(value) {
                    Some(colour) => config.background = colour,
                    None => complain(&mut problems, "a colour like #f5c740"),
                },
                "autostart" => {
                    if !value.is_empty() {
                        config.autostart.push(value.to_string());
                    }
                }
                _ => problems.push(format!("line {}: don't know the setting {key:?}", number + 1)),
            }
        }

        (config, problems)
    }
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa`. Returns straight (not premultiplied) RGBA.
pub fn parse_colour(text: &str) -> Option<[f32; 4]> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    let nibble = |i: usize| {
        u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| (v * 17) as f32 / 255.0)
    };
    match hex.len() {
        3 => Some([nibble(0)?, nibble(1)?, nibble(2)?, 1.0]),
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 1.0]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

/// First of `candidates` that exists on PATH.
pub fn first_on_path(candidates: &[&str]) -> Option<String> {
    let path = env::var_os("PATH")?;
    for candidate in candidates {
        for dir in env::split_paths(&path) {
            let full = dir.join(candidate);
            if full.is_file() {
                return Some((*candidate).to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_the_default() {
        let (config, problems) = Config::parse("");
        assert_eq!(config, Config::default());
        assert!(problems.is_empty());
    }

    #[test]
    fn reads_every_setting() {
        let (config, problems) = Config::parse(
            r#"
            # QuackOS
            terminal = foot
            launcher = bread-launcher --duck
            keyboard_layout = gb
            keyboard_variant = dvorak
            keyboard_options = ctrl:nocaps
            repeat_delay = 250
            repeat_rate = 45
            cursor_size = 32
            background = #112233
            autostart = bread-bar
            autostart = bread-dock
            "#,
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.terminal, "foot");
        assert_eq!(config.launcher, "bread-launcher --duck");
        assert_eq!(config.keyboard_layout, "gb");
        assert_eq!(config.keyboard_variant, "dvorak");
        assert_eq!(config.keyboard_options.as_deref(), Some("ctrl:nocaps"));
        assert_eq!(config.repeat_delay, 250);
        assert_eq!(config.repeat_rate, 45);
        assert_eq!(config.cursor_size, 32);
        assert_eq!(config.autostart, vec!["bread-bar", "bread-dock"]);
        assert!((config.background[0] - 0x11 as f32 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let (config, problems) = Config::parse("# all comment\n\n   \n  # indented\nterminal = kitty\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.terminal, "kitty");
    }

    #[test]
    fn hash_inside_a_value_is_not_a_comment() {
        // The whole reason comments are whole-line: colours start with '#'.
        let (config, problems) = Config::parse("background = #f5c740\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config.background, [245.0 / 255.0, 199.0 / 255.0, 64.0 / 255.0, 1.0]);
    }

    #[test]
    fn a_bad_line_is_reported_but_keeps_the_rest() {
        let (config, problems) =
            Config::parse("cursor_size = enormous\nterminal = foot\nnonsense\nwhat = 3\n");
        assert_eq!(config.terminal, "foot", "a bad line must not lose good ones");
        assert_eq!(config.cursor_size, Config::default().cursor_size);
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    #[test]
    fn colours() {
        assert_eq!(parse_colour("#000000"), Some([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_colour("ffffff"), Some([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(parse_colour("#fff"), Some([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(parse_colour("#00000000"), Some([0.0, 0.0, 0.0, 0.0]));
        assert_eq!(parse_colour("#12345"), None);
        assert_eq!(parse_colour("duck"), None);
        assert_eq!(parse_colour(""), None);
    }

    #[test]
    fn out_of_range_values_are_rejected_not_clamped_silently() {
        let (config, problems) = Config::parse("cursor_size = 4\nrepeat_rate = -1\n");
        assert_eq!(config.cursor_size, Config::default().cursor_size);
        assert_eq!(config.repeat_rate, Config::default().repeat_rate);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }
}
