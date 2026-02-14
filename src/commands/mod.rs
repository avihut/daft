/// Command modules for daft
///
/// Each module represents a Git extension command that can be invoked
/// either directly or via symlink detection in the multicall binary.
pub mod branch;
pub mod carry;
pub mod checkout;
pub mod checkout_branch;
pub mod clone;
pub mod complete;
pub mod completions;
pub mod docs;
pub mod doctor;
pub mod fetch;
pub mod flow_adopt;
pub mod flow_eject;
pub mod hooks;
pub mod init;
pub mod multi_remote;
pub mod prune;
pub mod release_notes;
pub mod setup;
pub mod shell_init;
pub mod shortcuts;
