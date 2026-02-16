# Homebrew Formula Customization Guide

This document explains the Homebrew formula setup for daft and how to customize
it for the multicall binary architecture.

## Overview

daft uses cargo-dist to automatically generate and maintain a Homebrew formula
in this repository at `Formula/daft.rb`. The formula is automatically updated on
each release.

## Multicall Binary Architecture

daft uses a **single binary with multiple symlinks** (like BusyBox):

- Single binary: `daft` (~589KB)
- Symlinks for Git commands:
  - `git-worktree-clone` → `daft`
  - `git-worktree-checkout` → `daft`
  - `git-worktree-checkout-branch` → `daft`
  - `git-worktree-init` → `daft`
  - `git-worktree-prune` → `daft`
  - `git-worktree-carry` → `daft`
  - `git-worktree-fetch` → `daft`
  - `git-daft` → `daft`
- Git-style shortcuts (default):
  - `gwtclone`, `gwtinit`, `gwtco`, `gwtcb`, `gwtprune`, `gwtcarry`, `gwtfetch`

## Formula Customization

### After First Release

When the first release is created (e.g., `v0.1.0`), cargo-dist will generate the
initial formula. You'll need to manually add the symlink creation to the
`install` block.

**Location:** `Formula/daft.rb`

### Required Modifications

The auto-generated formula will look something like this:

```ruby
class Daft < Formula
  desc "Git extensions toolkit for efficient branch management with worktrees"
  homepage "https://github.com/avihut/daft"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/avihut/daft/releases/download/v0.1.0/daft-aarch64-apple-darwin.tar.xz"
      sha256 "AUTO_GENERATED_SHA256"
    else
      url "https://github.com/avihut/daft/releases/download/v0.1.0/daft-x86_64-apple-darwin.tar.xz"
      sha256 "AUTO_GENERATED_SHA256"
    end
  end

  depends_on "git"

  def install
    bin.install "daft"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/daft --version")
  end
end
```

**You need to modify the `install` block to:**

```ruby
def install
  bin.install "daft"

  # Create symlinks for multicall binary (Git command integration)
  bin.install_symlink bin/"daft" => "git-worktree-clone"
  bin.install_symlink bin/"daft" => "git-worktree-checkout"
  bin.install_symlink bin/"daft" => "git-worktree-checkout-branch"
  bin.install_symlink bin/"daft" => "git-worktree-init"
  bin.install_symlink bin/"daft" => "git-worktree-prune"
  bin.install_symlink bin/"daft" => "git-worktree-carry"
  bin.install_symlink bin/"daft" => "git-worktree-fetch"
  bin.install_symlink bin/"daft" => "git-daft"

  # Create git-style shortcuts (default)
  bin.install_symlink bin/"daft" => "gwtclone"
  bin.install_symlink bin/"daft" => "gwtinit"
  bin.install_symlink bin/"daft" => "gwtco"
  bin.install_symlink bin/"daft" => "gwtcb"
  bin.install_symlink bin/"daft" => "gwtprune"
  bin.install_symlink bin/"daft" => "gwtcarry"
  bin.install_symlink bin/"daft" => "gwtfetch"

  # Generate and install man pages
  system bin/"daft", "man", "--output-dir=#{buildpath}/man"
  man1.install Dir["#{buildpath}/man/*.1"]

  # Install shell completions (if available in release)
  bash_completion.install "completions/daft.bash" if File.exist?("completions/daft.bash")
  zsh_completion.install "completions/_daft" if File.exist?("completions/_daft")
  fish_completion.install "completions/daft.fish" if File.exist?("completions/daft.fish")
end
```

**And add a `caveats` block (optional but recommended):**

```ruby
def caveats
  <<~EOS
    daft is now installed! Git worktree commands are available:
      git worktree-clone <repo>
      git worktree-checkout <branch>
      git worktree-checkout-branch <new-branch>
      git worktree-init <repo-name>
      git worktree-prune

    Run 'git daft' for full documentation.

    RECOMMENDED: For automatic cd into new worktrees, run:

      daft setup

    This will detect your shell and add the integration automatically.

    Or manually add to your shell config:
      Bash:  eval "$(daft shell-init bash)"
      Zsh:   eval "$(daft shell-init zsh)"
      Fish:  daft shell-init fish | source
  EOS
end
```

### Complete Example

Here's what the complete customized formula should look like:

```ruby
class Daft < Formula
  desc "Git extensions toolkit for efficient branch management with worktrees"
  homepage "https://github.com/avihut/daft"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/avihut/daft/releases/download/v0.1.0/daft-aarch64-apple-darwin.tar.xz"
      sha256 "abcdef1234567890..."  # Auto-generated
    else
      url "https://github.com/avihut/daft/releases/download/v0.1.0/daft-x86_64-apple-darwin.tar.xz"
      sha256 "1234567890abcdef..."  # Auto-generated
    end
  end

  depends_on "git"

  def install
    bin.install "daft"

    # Create symlinks for multicall binary
    bin.install_symlink bin/"daft" => "git-worktree-clone"
    bin.install_symlink bin/"daft" => "git-worktree-checkout"
    bin.install_symlink bin/"daft" => "git-worktree-checkout-branch"
      bin.install_symlink bin/"daft" => "git-worktree-init"
    bin.install_symlink bin/"daft" => "git-worktree-prune"
    bin.install_symlink bin/"daft" => "git-worktree-carry"
    bin.install_symlink bin/"daft" => "git-worktree-fetch"
    bin.install_symlink bin/"daft" => "git-daft"

    # Create git-style shortcuts (default)
    bin.install_symlink bin/"daft" => "gwtclone"
    bin.install_symlink bin/"daft" => "gwtinit"
    bin.install_symlink bin/"daft" => "gwtco"
    bin.install_symlink bin/"daft" => "gwtcb"
      bin.install_symlink bin/"daft" => "gwtprune"
    bin.install_symlink bin/"daft" => "gwtcarry"
    bin.install_symlink bin/"daft" => "gwtfetch"

    # Generate and install man pages
    system bin/"daft", "man", "--output-dir=#{buildpath}/man"
    man1.install Dir["#{buildpath}/man/*.1"]

    # Install shell completions
    bash_completion.install "completions/daft.bash" if File.exist?("completions/daft.bash")
    zsh_completion.install "completions/_daft" if File.exist?("completions/_daft")
    fish_completion.install "completions/daft.fish" if File.exist?("completions/daft.fish")
  end

  def caveats
    <<~EOS
      daft is now installed! Git worktree commands are available:
        git worktree-clone <repo>
        git worktree-checkout <branch>
        git worktree-checkout-branch <new-branch>
        git worktree-init <repo-name>
        git worktree-prune

      Shortcuts are also available:
        gwtclone, gwtco, gwtcb, gwtprune, gwtinit, gwtcarry, gwtfetch

      Run 'git daft' for full documentation.
      Run 'daft setup shortcuts list' to see all shortcut styles.

      RECOMMENDED: For automatic cd into new worktrees, run:

        daft setup

      This will detect your shell and add the integration automatically.

      Or manually add to your shell config:
        Bash:  eval "$(daft shell-init bash)"
        Zsh:   eval "$(daft shell-init zsh)"
        Fish:  daft shell-init fish | source
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/daft --version")
    assert_match "daft", shell_output("#{bin}/git-worktree-clone --help")
    # Verify man pages were installed
    assert_predicate man1/"git-worktree-clone.1", :exist?
  end
end
```

## Workflow Integration

### Automatic Updates

The GitHub Actions workflow (`publish-homebrew-formula` job) will:

1. Download the generated formula from cargo-dist
2. Update version and checksums
3. **Preserve your custom `install` and `caveats` blocks**
4. Run `brew style --fix` to ensure proper formatting
5. Commit the updated formula back to the repository

### Important Notes

**The workflow preserves manually added code**, so your symlink creation and
completions installation will persist across releases. The workflow only
updates:

- Version number
- SHA256 checksums
- Download URLs

**However, you should verify after the first few releases** that customizations
are preserved correctly.

## Testing the Formula

### Local Testing

Before committing changes to the formula:

```bash
# Test installation from local formula
brew install --build-from-source ./Formula/daft.rb

# Verify all symlinks were created
ls -l $(brew --prefix)/bin/git-worktree-*

# Test commands work
git worktree-clone --help
git worktree-checkout --help
daft --version

# Uninstall to clean up
brew uninstall daft
```

### Audit the Formula

Check the formula meets Homebrew standards:

```bash
# Run audit
brew audit --strict --online Formula/daft.rb

# Fix style issues
brew style --fix Formula/daft.rb
```

## Installation for Users

After the formula is in place, users can install with:

```bash
# Install from tap
brew install avihut/tap/daft

# Or tap first, then install
brew tap avihut/tap
brew install daft
```

## Man Pages

The formula generates and installs man pages during installation using
`daft man --output-dir`. This ensures:

- Man pages are always in sync with the installed binary version
- `man git-worktree-clone` works immediately after installation
- Man pages are installed to Homebrew's standard location
  (`$(brew --prefix)/share/man/man1/`)

### Man Page Installation

The formula includes:

```ruby
# Generate and install man pages
system bin/"daft", "man", "--output-dir=#{buildpath}/man"
man1.install Dir["#{buildpath}/man/*.1"]
```

This generates man pages for all commands:

- `git-worktree-clone.1`
- `git-worktree-checkout.1`
- `git-worktree-checkout-branch.1`
- `git-worktree-init.1`
- `git-worktree-prune.1`
- `git-worktree-carry.1`

### Verifying Man Pages

After installation:

```bash
# View man page
man git-worktree-clone

# Check man page location
man -w git-worktree-clone
# Should show: /usr/local/share/man/man1/git-worktree-clone.1
```

## Shell Completions

### Including Completions in Release

To include shell completions in the release artifacts, you need to generate them
and include them in the tarball.

**Option 1: Pre-generate before tagging**

```bash
# Generate completions
mkdir -p completions
./target/release/daft completions bash > completions/daft.bash
./target/release/daft completions zsh > completions/_daft
./target/release/daft completions fish > completions/daft.fish

# Ensure they're included in git
git add completions/
git commit -m "Add shell completions for release"
```

**Option 2: Build script (build.rs)**

Create `build.rs` to generate completions at build time (future enhancement).

### Completion Installation Paths

Homebrew installs completions to standard locations:

- **Bash**: `$(brew --prefix)/etc/bash_completion.d/`
- **Zsh**: `$(brew --prefix)/share/zsh/site-functions/`
- **Fish**: `$(brew --prefix)/share/fish/vendor_completions.d/`

These are automatically loaded by properly configured shells.

## Troubleshooting

### Symlinks Not Created

**Symptom:** `git worktree-clone` not found after installation

**Cause:** `install` block doesn't include symlink creation

**Fix:** Update `Formula/daft.rb` with the symlink commands shown above

### Completions Not Loading

**Symptom:** Tab completion doesn't work for daft commands

**Causes:**

1. Completions not included in release tarball
2. Completions not installed by formula
3. Shell not configured to load Homebrew completions

**Fix:**

1. Verify `completions/` directory exists in release artifacts
2. Check formula includes completion installation commands
3. For bash, ensure `.bash_profile` or `.bashrc` sources Homebrew completions:
   ```bash
   [[ -r "$(brew --prefix)/etc/profile.d/bash_completion.sh" ]] && . "$(brew --prefix)/etc/profile.d/bash_completion.sh"
   ```

### Wrong Architecture Downloaded

**Symptom:** Binary doesn't work on Apple Silicon Mac

**Cause:** Homebrew detected wrong CPU architecture

**Fix:** Formula's `on_macos` and `Hardware::CPU.arm?` check should handle this
automatically. Verify the formula structure matches the example above.

## Future: Homebrew-core Submission

After the tap is stable and has significant adoption (~1000+ installs), we can
submit to [homebrew-core](https://github.com/Homebrew/homebrew-core) for
official inclusion.

**Requirements:**

- Stable version (1.0.0+)
- Proven track record
- Active maintenance
- No major issues reported

**Benefits:**

- Users can install with just `brew install daft` (no tap needed)
- Wider discoverability
- Official Homebrew stamp of approval
- Automatic updates via Homebrew

See [Homebrew's Acceptable Formulae](https://docs.brew.sh/Acceptable-Formulae)
for submission guidelines.
