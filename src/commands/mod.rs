/// Command modules for daft
///
/// Each module represents a Git extension command that can be invoked
/// either directly or via symlink detection in the multicall binary.
pub mod carry;
pub mod checkout;
pub mod checkout_branch;
pub mod checkout_branch_from_default;
pub mod clone;
pub mod complete;
pub mod completions;
pub mod docs;
pub mod hooks;
pub mod init;
pub mod prune;
pub mod setup;
pub mod shell_init;
