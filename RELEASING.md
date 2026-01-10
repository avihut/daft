# Release Process

This project uses [cargo-dist](https://github.com/axodotdev/cargo-dist) for fully automated cross-platform releases.

## Prerequisites

- Write access to the repository
- All tests passing locally (`make test`)
- Code quality checks passing (`cargo clippy -- -D warnings`, `cargo fmt --check`)
- GitHub token configured (automatic for maintainers)

## Quick Release Guide

### 1. Update Version

Edit `Cargo.toml` to update the version:

```toml
[package]
version = "0.2.0"  # Change from 0.1.0
```

### 2. Update Changelog

Edit `CHANGELOG.md` (create if it doesn't exist) with release notes:

```markdown
## [0.2.0] - 2025-10-19

### Added
- Windows support via PowerShell installer and MSI
- Homebrew formula auto-updates
- Cross-platform release automation

### Changed
- Improved installation documentation

### Fixed
- Bug fixes from Issue #X
```

### 3. Commit and Tag

```bash
# Commit the version bump
git add Cargo.toml CHANGELOG.md
git commit -m "chore: release v0.2.0"

# Create and push tag
git tag v0.2.0
git push origin main --tags
```

### 4. Monitor Release

The GitHub Actions workflow will automatically:

1. **Build** binaries for:
   - macOS Intel (x86_64-apple-darwin)
   - macOS Apple Silicon (aarch64-apple-darwin)
   - Windows 64-bit (x86_64-pc-windows-msvc)

2. **Generate** installers:
   - PowerShell installer script (`daft-installer.ps1`)
   - MSI installer for Windows
   - Homebrew formula (`Formula/daft.rb`)

3. **Create** GitHub Release with:
   - Compiled binaries (tar.xz for macOS, zip for Windows)
   - Installers
   - Checksums (`SHA256SUMS`)
   - Auto-generated release notes

4. **Update** Homebrew formula:
   - Commits updated `Formula/daft.rb` to repository
   - Updates version and checksums automatically

Watch the workflow at: https://github.com/avihut/daft/actions

Typical completion time: **8-12 minutes**

## What Happens Automatically

### GitHub Actions Workflow

The release is triggered when you push a tag matching the pattern `v*.*.*` (e.g., `v0.2.0`, `v1.0.0-beta.1`).

**Jobs:**
1. **plan**: Determines what needs to be built
2. **build-local-artifacts**: Builds binaries for each platform in parallel
3. **host**: Creates GitHub Release and uploads artifacts
4. **publish-homebrew-formula**: Updates and commits Homebrew formula

### Artifacts Generated

For each release, the following artifacts are created:

- `daft-aarch64-apple-darwin.tar.xz` - macOS Apple Silicon binary
- `daft-x86_64-apple-darwin.tar.xz` - macOS Intel binary
- `daft-x86_64-pc-windows-msvc.zip` - Windows binary
- `daft-installer.ps1` - PowerShell installer
- `daft-installer.msi` - Windows MSI installer
- `SHA256SUMS` - Checksums for all artifacts
- `Formula/daft.rb` - Updated Homebrew formula

### Homebrew Formula Updates

The formula is automatically updated with:
- New version number
- SHA256 checksums for macOS binaries
- URLs pointing to new release artifacts

The updated formula is committed back to the `Formula/` directory in the main repository.

## Testing Before Release

### Local Build Test

Test the build process locally before creating a release:

```bash
# Test building for current platform
cargo dist build --target=aarch64-apple-darwin --artifacts=local

# Check generated artifacts
ls -lh target/distrib/

# Verify tarball contents
tar -tzf target/distrib/daft-*.tar.xz
```

### Pre-Release Quality Checks

Run these checks before pushing a tag:

```bash
# All tests must pass
make test

# No clippy warnings
cargo clippy -- -D warnings

# Formatting is correct
cargo fmt --check

# Build succeeds
cargo build --release
```

### Dry Run (Optional)

Create a pre-release to test the workflow:

```bash
# Tag as pre-release
git tag v0.2.0-rc.1
git push origin v0.2.0-rc.1

# This creates a "Pre-release" on GitHub
# Test installations work before final release
```

## Installation Testing

After release, verify installations work:

### macOS

```bash
# Install from Homebrew
brew install avihut/daft

# Verify all commands
daft --version
git worktree-clone --help
git worktree-checkout --help
```

### Windows

```powershell
# Test PowerShell installer
irm https://github.com/avihut/daft/releases/latest/download/daft-installer.ps1 | iex

# Verify installation
daft --version
git-worktree-clone --help
```

## Troubleshooting

### Build Fails for Specific Platform

1. Check GitHub Actions logs: https://github.com/avihut/daft/actions
2. Look for the failed job (build-local-artifacts)
3. Review error messages in the job output

**Common issues:**
- Missing dependencies on target platform
- Cross-compilation toolchain problems (handled by GitHub runners)
- Platform-specific code issues

### Homebrew Formula Not Updating

**Symptoms:** Formula file in repository not updated after release

**Possible causes:**
1. `HOMEBREW_TAP_TOKEN` secret not configured
2. Formula path incorrect in `dist-workspace.toml`
3. publish-homebrew-formula job failed

**Solutions:**
1. Verify GitHub secret is set: Settings → Secrets → Actions → `HOMEBREW_TAP_TOKEN`
2. Check `dist-workspace.toml`:
   ```toml
   tap = "avihut/daft"
   formula = "Formula/daft.rb"
   ```
3. Review publish-homebrew-formula job logs

### Installation Fails

**Symptoms:** `brew install avihut/daft` fails with checksum mismatch

**Cause:** Checksums in formula don't match downloaded artifacts

**Solution:**
1. Verify formula was updated correctly in `Formula/daft.rb`
2. Check SHA256 values match those in release artifacts
3. If mismatch, manually update formula or re-run release

### Windows MSI Issues

**Symptoms:** MSI installer doesn't work or fails to install

**Cause:** WiX configuration issues

**Check:**
1. Review `wix/main.wxs` configuration
2. Verify GUIDs are unique and stable
3. Test MSI locally before release (requires Windows machine)

## GitHub Secrets Configuration

The release workflow requires this GitHub secret:

### HOMEBREW_TAP_TOKEN

**Purpose:** Allows workflow to commit updated Homebrew formula to repository

**How to create:**

1. Go to GitHub Settings → Developer Settings → Personal Access Tokens → Tokens (classic)
2. Click "Generate new token (classic)"
3. Name: `daft-homebrew-updater`
4. Scopes:
   - ✅ `repo` (all repository permissions)
5. Click "Generate token"
6. Copy the token (you won't see it again!)

7. Add to repository:
   - Go to repository Settings → Secrets and variables → Actions
   - Click "New repository secret"
   - Name: `HOMEBREW_TAP_TOKEN`
   - Value: paste the token
   - Click "Add secret"

**Note:** This token allows the workflow to push commits to your repository. Keep it secure!

## Release Checklist

Use this checklist for each release:

### Pre-Release
- [ ] All features tested and working
- [ ] All tests passing: `make test`
- [ ] No clippy warnings: `cargo clippy -- -D warnings`
- [ ] Code formatted: `cargo fmt --check`
- [ ] CHANGELOG.md updated with release notes
- [ ] Version updated in Cargo.toml
- [ ] Local build test successful: `cargo dist build`

### Release
- [ ] Changes committed: `git add . && git commit -m "chore: release vX.Y.Z"`
- [ ] Tag created and pushed: `git tag vX.Y.Z && git push origin main --tags`
- [ ] GitHub Actions workflow triggered
- [ ] All jobs completed successfully (check Actions tab)
- [ ] GitHub Release created with all artifacts
- [ ] Formula/daft.rb updated in repository

### Post-Release
- [ ] macOS Intel installation tested
- [ ] macOS Apple Silicon installation tested
- [ ] Windows installation tested (PowerShell and/or MSI)
- [ ] All git commands working after installation
- [ ] Shell completions loading correctly
- [ ] `brew upgrade daft` works (if not first release)
- [ ] Release announced (GitHub, social media, etc.)

## Version Numbering

Follow [Semantic Versioning](https://semver.org/):

- **MAJOR.MINOR.PATCH** (e.g., 1.2.3)
  - **MAJOR**: Incompatible API changes
  - **MINOR**: New functionality (backwards compatible)
  - **PATCH**: Bug fixes (backwards compatible)

**Pre-release versions:**
- Alpha: `v0.2.0-alpha.1`
- Beta: `v0.2.0-beta.1`
- Release candidate: `v0.2.0-rc.1`

## Rolling Back a Release

If a release has critical issues:

### Option 1: Delete Tag and Release (Pre-Distribution)

If users haven't installed yet:

```bash
# Delete local tag
git tag -d v0.2.0

# Delete remote tag
git push origin :refs/tags/v0.2.0

# Delete GitHub Release manually in UI
# Go to Releases → Edit → Delete release
```

### Option 2: Quick Hotfix Release (Post-Distribution)

If users have already installed:

```bash
# Fix the issue
vim src/...

# Test thoroughly
make test

# Release hotfix
vim Cargo.toml  # version = "0.2.1"
git add .
git commit -m "fix: critical bug in worktree-clone"
git tag v0.2.1
git push origin main --tags
```

## Support

For issues with the release process:
- Check https://opensource.axo.dev/cargo-dist/book/
- Open issue: https://github.com/avihut/daft/issues
- Review cargo-dist docs: https://github.com/axodotdev/cargo-dist

## Future Enhancements

Planned improvements to the release process:

- **Automatic CHANGELOG generation** from conventional commits
- **crates.io publishing** for Rust library users
- **Linux packages**: DEB/RPM via cargo-dist
- **AUR package** for Arch Linux
- **Nix package** for NixOS
- **Homebrew-core submission** after stability proven (requires 1000+ users)
