This project provides a set of shell scripts to streamline a Git workflow that heavily utilizes `git worktree`. The scripts are designed to be run as Git subcommands (e.g., `git worktree-clone`).

### Core Concepts

*   **Worktree-centric:** The workflow is built around the idea of having one worktree per branch, with worktrees for a given repository being organized under a common parent directory.
*   **`direnv` Integration:** The scripts include optional integration with `direnv`, automatically running `direnv allow` when you `cd` into a new worktree that contains a `.envrc` file.

### Directory Structure and Assumptions

*   **Initial Setup (`git worktree-clone`):** The `git worktree-clone` script establishes the foundational directory structure. It clones a repository into a new parent folder named after the repo, and the initial worktree is created in a subdirectory named after the remote's default branch. For example, running `git worktree-clone git@github.com:user/my-repo.git` (where `main` is the default branch) results in:
    ```
    ./my-repo/
    └── main/      <-- This is the first worktree
        ├── .git   <-- This is a file pointing to the real git data
        └── ... (repository files)
    ```

*   **Creating New Worktrees:** The other scripts (`git worktree-checkout`, `git worktree-checkout-branch`, etc.) are intended to be run from *inside* an existing worktree (e.g., from the `~/projects/my-repo/main` directory). They create new worktrees as siblings to the current one by using a relative path (`../<new-branch-name>`).

*   **`.git` Directory Location:** The scripts do not make any hardcoded assumptions about the location of the main `.git` directory. They correctly use `git rev-parse --git-common-dir` to locate the shared Git metadata directory, making them robust and compatible with the standard `git worktree` mechanism.

*   **Default Branch:** The scripts do not assume a default branch name like `master`. Both `git worktree-clone` and `git worktree-checkout-branch-from-default` dynamically query the remote repository to determine its actual default branch (`main`, `develop`, etc.) before creating new branches or worktrees.

### Scripts

*   **`git-worktree-clone`**: Clones a remote repository into the structured directory layout: `<repo-name>/<default-branch>`.
*   **`git-worktree-checkout-branch`**: Creates a new worktree and a new branch from the current or a specified base branch.
*   **`git-worktree-checkout-branch-from-default`**: Creates a new worktree and a new branch from the remote's default branch.
*   **`git-worktree-checkout`**: Creates a worktree from an *existing* local or remote branch.
*   **`git-worktree-prune`**: Prunes local branches whose remote counterparts have been deleted, also removing any associated worktrees.

### Installation and Usage

These scripts are intended to be used as custom Git commands.

1.  **Add to PATH:** Add the absolute path to the `scripts` directory to your system's `PATH` environment variable. You can do this by adding the following line to your shell's startup file (e.g., `~/.bashrc`, `~/.zshrc`, or `~/.config/fish/config.fish`):

    ```shell
    export PATH="/path/to/your/git-worktree-workflow/scripts:$PATH"
    ```

    Remember to replace `/path/to/your/git-worktree-workflow` with the actual path to this project on your machine.

2.  **Execute as Git Commands:** Once the `scripts` directory is in your `PATH`, Git will automatically detect and treat the scripts as subcommands. You can then execute them directly:

    *   `git worktree-clone <repository-url>`
    *   `git worktree-checkout-branch <new-branch-name> [base-branch-name]`
    *   `git worktree-checkout-branch-from-default <new-branch-name>`
    *   `git worktree-checkout <existing-branch-name>`
    *   `git worktree-prune`

This set of scripts provides a powerful and efficient way to manage a `git worktree`-based development workflow, keeping your projects organized and reducing the manual effort required for common Git operations.
