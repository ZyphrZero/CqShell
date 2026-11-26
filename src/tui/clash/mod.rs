//! Dedicated Mihomo (Clash Meta) visual TUI management workspace integrated into CqShell's
//! Sidebar-Shell (FlClash-inspired 6-tab cockpit aligned with the Docker workspace).

pub(super) mod events;
#[cfg(test)]
pub(super) mod fixture;
pub(super) mod layout;
pub(super) mod render;
pub(super) mod state;
