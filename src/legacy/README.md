# Legacy Shell Scripts (DEPRECATED)

⚠️ **DEPRECATED**: These shell scripts are deprecated and maintained only for backward compatibility. Please use the new Rust implementation instead.

## 📁 Contents

This directory contains the original shell script implementations:

- `git-worktree-clone` - Clone repository with worktree structure
- `git-worktree-init` - Initialize new repository
- `git-worktree-checkout` - Create worktree from existing branch
- `git-worktree-checkout-branch` - Create new branch with worktree
- `git-worktree-checkout-branch-from-default` - Branch from remote default
- `git-worktree-prune` - Clean up deleted remote branches

## 🚨 Migration Notice

**These scripts are deprecated as of v2.0.0**

### Why Deprecated?
- Limited error handling and validation
- Manual argument parsing
- No type safety
- Harder to maintain and extend
- Slower execution

### Migration Path
Use the new Rust implementation instead:

```bash
# Instead of using shell scripts:
export PATH="./src/legacy:$PATH"
git worktree-clone https://github.com/user/repo.git

# Use the Rust implementation:
cargo build --release
export PATH="./target/release:$PATH"
git-worktree-clone https://github.com/user/repo.git
```

## 📋 Feature Comparison

| Feature | Shell Scripts | Rust Implementation |
|---------|---------------|-------------------|
| **Help System** | Basic usage on error | `--help` with detailed info |
| **Error Messages** | Basic | Detailed with context |
| **Argument Validation** | Manual validation | Type-safe parsing |
| **Performance** | Slower script parsing | Faster compiled binaries |
| **Maintenance** | Manual testing | Compile-time error checking |
| **Cross-platform** | Unix/Linux only | Better Windows support |

## 🔧 Using Legacy Scripts

If you must use the legacy scripts:

```bash
# Add to PATH
export PATH="./src/legacy:$PATH"

# Use commands (note: different naming)
git worktree-clone <repo-url>
git worktree-init <repo-name>
git worktree-checkout <branch>
git worktree-checkout-branch <new-branch>
git worktree-checkout-branch-from-default <new-branch>
git worktree-prune
```

## 📅 Deprecation Timeline

- **v2.0.0** (Current): Scripts moved to legacy, marked deprecated
- **v2.1.0** (Planned): Remove from main documentation
- **v3.0.0** (Future): Consider removal (with major version bump)

## 🤝 Support

For issues with legacy scripts:
1. **First**: Try the Rust implementation
2. **If blocked**: File an issue explaining why Rust version doesn't work
3. **Community**: Help welcome to maintain legacy scripts

## 🔗 Migration Resources

- [Installation Guide](../../README.md#installation)
- [Usage Examples](../../README.md#command-usage-examples)
- [Rust Implementation Benefits](../../README.md#rust-implementation-benefits)

---

**Recommendation**: Please migrate to the Rust implementation for better performance, reliability, and user experience! 🚀