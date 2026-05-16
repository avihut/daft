#!/bin/bash

# Integration tests for daft activate command
# Tests automatic shell config activation functionality

set -eo pipefail

# Source the test framework
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/test_framework.sh"

# --- Test Functions ---

test_activate_help() {
    log "Testing: daft activate --help shows usage"

    local output
    output=$(daft activate --help 2>&1)

    if echo "$output" | grep -q "shell-init"; then
        log_success "Help text mentions shell-init"
    else
        log_error "Help text missing shell-init"
        return 1
    fi

    if echo "$output" | grep -q "\-\-dry-run"; then
        log_success "Help text mentions --dry-run"
    else
        log_error "Help text missing --dry-run"
        return 1
    fi

    if echo "$output" | grep -q "\-\-force"; then
        log_success "Help text mentions --force"
    else
        log_error "Help text missing --force"
        return 1
    fi

    return 0
}

test_activate_dry_run() {
    log "Testing: daft activate --dry-run shows what would be done"

    # Use a fake HOME so the dry-run isn't short-circuited by the developer's
    # already-configured ~/.zshrc (would hit the "already configured" branch).
    local fake_home
    fake_home=$(mktemp -d)
    local output
    output=$(HOME="$fake_home" SHELL="/bin/zsh" daft activate --dry-run 2>&1)
    rm -rf "$fake_home"

    if echo "$output" | grep -q "Detected shell:"; then
        log_success "Output shows detected shell"
    else
        log_error "Output missing detected shell"
        return 1
    fi

    if echo "$output" | grep -q "Config file:"; then
        log_success "Output shows config file"
    else
        log_error "Output missing config file"
        return 1
    fi

    if echo "$output" | grep -q "Will append"; then
        log_success "Output shows what will be appended"
    else
        log_error "Output missing append preview"
        return 1
    fi

    if echo "$output" | grep -q "\[dry-run\] No changes made"; then
        log_success "Output confirms no changes made"
    else
        log_error "Output missing dry-run confirmation"
        return 1
    fi

    return 0
}

test_activate_creates_config() {
    log "Testing: daft activate creates config in new file"

    # Create a temp directory for a fake home
    local fake_home
    fake_home=$(mktemp -d)

    # Run setup with fake home, forcing zsh
    local output
    output=$(HOME="$fake_home" SHELL="/bin/zsh" daft activate --force 2>&1) || true

    local config_file="$fake_home/.zshrc"

    if [ -f "$config_file" ]; then
        log_success "Config file was created"
    else
        log_error "Config file was not created"
        rm -rf "$fake_home"
        return 1
    fi

    if grep -q "daft shell-init zsh" "$config_file"; then
        log_success "Config file contains shell-init line"
    else
        log_error "Config file missing shell-init line"
        cat "$config_file"
        rm -rf "$fake_home"
        return 1
    fi

    rm -rf "$fake_home"
    return 0
}

test_activate_idempotent() {
    log "Testing: daft activate is idempotent (doesn't add duplicates)"

    # Create a temp directory for a fake home
    local fake_home
    fake_home=$(mktemp -d)

    # Create a .zshrc with daft already configured
    echo '# existing config' > "$fake_home/.zshrc"
    echo 'eval "$(daft shell-init zsh)"' >> "$fake_home/.zshrc"

    # Run activate without --force — it should detect the existing line and
    # return early. (--force would skip the check by design.)
    local output
    output=$(HOME="$fake_home" SHELL="/bin/zsh" daft activate 2>&1) || true

    if echo "$output" | grep -q "already configured"; then
        log_success "Setup detected existing configuration"
    else
        log_error "Setup did not detect existing configuration"
        echo "Output: $output"
        rm -rf "$fake_home"
        return 1
    fi

    # Check that the line only appears once
    local count
    count=$(grep -c "daft shell-init" "$fake_home/.zshrc" || echo "0")
    if [ "$count" -eq 1 ]; then
        log_success "Config file has exactly one shell-init line"
    else
        log_error "Config file has $count shell-init lines (expected 1)"
        cat "$fake_home/.zshrc"
        rm -rf "$fake_home"
        return 1
    fi

    rm -rf "$fake_home"
    return 0
}

test_activate_creates_backup() {
    log "Testing: daft activate creates backup of existing config"

    # Create a temp directory for a fake home
    local fake_home
    fake_home=$(mktemp -d)

    # Create an existing .zshrc
    echo '# existing config' > "$fake_home/.zshrc"
    echo 'export FOO=bar' >> "$fake_home/.zshrc"

    # Run setup
    local output
    output=$(HOME="$fake_home" SHELL="/bin/zsh" daft activate --force 2>&1) || true

    local backup_file="$fake_home/.zshrc.bak"

    if [ -f "$backup_file" ]; then
        log_success "Backup file was created"
    else
        log_error "Backup file was not created"
        rm -rf "$fake_home"
        return 1
    fi

    if grep -q "export FOO=bar" "$backup_file"; then
        log_success "Backup contains original content"
    else
        log_error "Backup missing original content"
        rm -rf "$fake_home"
        return 1
    fi

    rm -rf "$fake_home"
    return 0
}

test_activate_bash_detection() {
    log "Testing: daft activate detects bash shell"

    local output
    output=$(SHELL="/bin/bash" daft activate --dry-run 2>&1)

    if echo "$output" | grep -q "Detected shell: bash"; then
        log_success "Correctly detected bash"
    else
        log_error "Did not detect bash"
        echo "Output: $output"
        return 1
    fi

    if echo "$output" | grep -q "\.bashrc"; then
        log_success "Shows .bashrc config file"
    else
        log_error "Did not show .bashrc"
        return 1
    fi

    return 0
}

test_activate_fish_detection() {
    log "Testing: daft activate detects fish shell"

    local output
    output=$(SHELL="/usr/local/bin/fish" daft activate --dry-run 2>&1)

    if echo "$output" | grep -q "Detected shell: fish"; then
        log_success "Correctly detected fish"
    else
        log_error "Did not detect fish"
        echo "Output: $output"
        return 1
    fi

    if echo "$output" | grep -q "config\.fish"; then
        log_success "Shows config.fish config file"
    else
        log_error "Did not show config.fish"
        return 1
    fi

    return 0
}

# --- Main Test Runner ---

main() {
    setup

    echo
    echo "========================================================="
    echo "Running daft activate Integration Tests"
    echo "========================================================="
    echo

    run_test "activate_help" test_activate_help
    run_test "activate_dry_run" test_activate_dry_run
    run_test "activate_creates_config" test_activate_creates_config
    run_test "activate_idempotent" test_activate_idempotent
    run_test "activate_creates_backup" test_activate_creates_backup
    run_test "activate_bash_detection" test_activate_bash_detection
    run_test "activate_fish_detection" test_activate_fish_detection

    print_summary
}

main "$@"
