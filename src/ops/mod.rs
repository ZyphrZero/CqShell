pub mod ai;
pub mod clash;
pub mod docker;
pub mod ip_quality;
pub mod kernel;
pub mod memory;
pub mod mirrors;
pub mod network;
pub mod packages;
pub mod service;
pub mod ssh;
pub mod system;
pub mod terminal;
pub mod tools;

/// An existing item a parameter can name, such as a saved subscription or an installed
/// kernel. Listing these lets the TUI offer a picker instead of free-text entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The exact parameter value.
    pub value: String,
    /// Short context shown beside the value; empty when there is nothing to add.
    pub detail: String,
    /// The item currently in effect, such as the selected subscription.
    pub current: bool,
    /// Values a dependent parameter may take once this one is chosen, such as the members
    /// of a proxy group.
    pub children: Vec<Choice>,
}
