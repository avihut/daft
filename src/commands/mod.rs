/// Command modules for daft
///
/// Each module represents a Git extension command that can be invoked
/// either directly or via symlink detection in the multicall binary.
pub mod activate;
pub mod branch_delete;
pub mod carry;
pub mod checkout;
pub mod clone;
pub mod complete;
pub mod completions;
pub mod config;
pub mod docs;
pub mod doctor;
pub mod dump_store;
pub mod env;
pub mod exec;
pub mod fetch;
pub mod file;
pub mod forge_cache;
pub mod git_hook;
pub mod hooks;
pub mod init;
pub mod install;
pub mod layout;
pub mod list;
pub mod list_empty;
pub mod list_live;
pub mod merge;
pub mod multi_remote;
pub mod prune;
pub mod push;
pub mod release_notes;
pub mod repo;
pub mod run;
pub mod shared;
pub mod shell_init;
pub mod shortcuts;
pub mod size_cache;
pub mod skill;
pub mod sync;
pub(super) mod sync_shared;
pub mod warm;
pub mod worktree_branch;
