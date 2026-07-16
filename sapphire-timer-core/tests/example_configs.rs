//! Verify that the example config files under `docs/config/` stay in sync
//! with the `UserConfig` / `TimerConfig` schemas.

use sapphire_timer_core::timer::TimerConfig;
use sapphire_timer_core::user_config::UserConfig;

#[test]
fn user_config_example_parses() {
    let raw = include_str!("../../docs/config/user-config.toml");
    toml::from_str::<UserConfig>(raw)
        .expect("docs/config/user-config.toml must parse as UserConfig");
}

#[test]
fn timer_config_example_parses() {
    let raw = include_str!("../../docs/config/timer-config.toml");
    toml::from_str::<TimerConfig>(raw)
        .expect("docs/config/timer-config.toml must parse as TimerConfig");
}
