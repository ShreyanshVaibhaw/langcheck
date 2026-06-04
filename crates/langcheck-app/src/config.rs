//! Configuration: defaults, validation, migration, and (de)serialization.
//!
//! Loaded from `%LOCALAPPDATA%\LangCheck\config.toml`. A missing or corrupt file
//! falls back to safe defaults, and every load is validated and clamped — in
//! particular `retain_typing_history` is a documented invariant that is *forced*
//! to `false` and is never a user-configurable feature (`blueprint.md`
//! Sections 8.13, 12.1). Atomic persistence lives in [`crate::persistence`].
//!
//! Implemented in delivery Step 08 (Native Tray, Settings, and Persistence).

use serde::{Deserialize, Serialize};

/// Current configuration schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// The correction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionMode {
    /// Observe nothing; perform no correction.
    Off,
    /// Apply only high-confidence corrections (default).
    #[default]
    Conservative,
    /// Detect but never replace automatically (post-MVP UI).
    Suggest,
}

/// Performance bounds (`blueprint.md` Section 8.13 `[performance]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceConfig {
    pub decision_deadline_ms: u64,
    pub max_token_chars: usize,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            decision_deadline_ms: 15,
            max_token_chars: 32,
        }
    }
}

/// Privacy settings. `retain_typing_history` is a documented invariant, not a
/// feature: it must always be `false` (`blueprint.md` Sections 8.13, 12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PrivacyConfig {
    pub context_mode: bool,
    pub retain_typing_history: bool,
}

/// The full configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub enabled: bool,
    pub mode: CorrectionMode,
    pub language: String,
    pub start_at_login: bool,
    pub launch_in_background: bool,
    pub undo_window_ms: u64,
    pub diagnostics: bool,
    pub performance: PerformanceConfig,
    pub privacy: PrivacyConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            enabled: true,
            mode: CorrectionMode::Conservative,
            language: "en-US".to_owned(),
            start_at_login: false,
            launch_in_background: true,
            undo_window_ms: 2000,
            diagnostics: false,
            performance: PerformanceConfig::default(),
            privacy: PrivacyConfig::default(),
        }
    }
}

impl Config {
    /// Parse TOML, falling back to defaults on any parse error (corrupt config
    /// must never block startup), then validate/clamp/migrate.
    pub fn load_str(toml_text: &str) -> Self {
        let mut config = toml::from_str::<Config>(toml_text).unwrap_or_default();
        config.validate();
        config
    }

    /// Serialize to TOML for atomic persistence.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Clamp ranges, migrate the schema version, and enforce invariants.
    pub fn validate(&mut self) {
        // Enforce the privacy invariants regardless of file contents.
        self.privacy.retain_typing_history = false;
        // Context mode stays off in the MVP (no privacy/quality gate yet).
        self.privacy.context_mode = false;

        if self.language.is_empty() {
            self.language = "en-US".to_owned();
        }
        self.undo_window_ms = self.undo_window_ms.clamp(500, 10_000);
        self.performance.decision_deadline_ms = self.performance.decision_deadline_ms.clamp(5, 50);
        self.performance.max_token_chars = self.performance.max_token_chars.clamp(8, 64);

        // Forward-migrate unknown/older schema versions to the current one.
        if self.schema_version != SCHEMA_VERSION {
            self.schema_version = SCHEMA_VERSION;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe() {
        let c = Config::default();
        assert!(c.enabled);
        assert_eq!(c.mode, CorrectionMode::Conservative);
        assert!(!c.start_at_login);
        assert!(!c.privacy.retain_typing_history);
        assert!(!c.privacy.context_mode);
    }

    #[test]
    fn corrupt_toml_falls_back_to_defaults() {
        let c = Config::load_str("this is not valid toml {{{");
        assert_eq!(c, Config::default());
    }

    #[test]
    fn retain_typing_history_is_always_forced_false() {
        let c = Config::load_str("[privacy]\nretain_typing_history = true\ncontext_mode = true\n");
        assert!(!c.privacy.retain_typing_history, "invariant must hold");
        assert!(!c.privacy.context_mode);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let c = Config::load_str(
            "undo_window_ms = 99999999\n[performance]\ndecision_deadline_ms = 0\nmax_token_chars = 9999\n",
        );
        assert_eq!(c.undo_window_ms, 10_000);
        assert_eq!(c.performance.decision_deadline_ms, 5);
        assert_eq!(c.performance.max_token_chars, 64);
    }

    #[test]
    fn partial_config_keeps_other_defaults() {
        let c = Config::load_str("enabled = false\nmode = \"off\"\n");
        assert!(!c.enabled);
        assert_eq!(c.mode, CorrectionMode::Off);
        assert_eq!(c.language, "en-US"); // defaulted
    }

    #[test]
    fn round_trips_through_toml() {
        let original = Config {
            enabled: false,
            start_at_login: true,
            ..Config::default()
        };
        let reparsed = Config::load_str(&original.to_toml());
        assert_eq!(reparsed, original);
    }

    #[test]
    fn unknown_schema_version_is_migrated() {
        let c = Config::load_str("schema_version = 999\n");
        assert_eq!(c.schema_version, SCHEMA_VERSION);
    }
}
