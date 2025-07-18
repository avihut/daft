#!/bin/bash

# Minimal test to debug CI issues
echo "=== MINIMAL TEST START ==="

# Test basic commands
echo "Testing basic commands..."
which git || echo "Git not found"
which awk || echo "AWK not found"
which basename || echo "basename not found"

# Test PATH
echo "Testing PATH..."
echo "PATH: $PATH"
which git-worktree-init || echo "git-worktree-init not in PATH"

# Test directory operations
echo "Testing directory operations..."
mkdir -p /tmp/test-dir || echo "Failed to create temp directory"
touch /tmp/test-file || echo "Failed to create temp file"
ls -la /tmp/test-dir /tmp/test-file || echo "Failed to list files"

# Test git config
echo "Testing git config..."
git config --global user.name "Test User" || echo "Failed to set git user name"
git config --global user.email "test@example.com" || echo "Failed to set git email"

# Test git worktree init
echo "Testing git worktree init..."
cd /tmp
git worktree-init minimal-test-repo || echo "Failed to init worktree repo"
ls -la minimal-test-repo || echo "Failed to list repo contents"

echo "=== MINIMAL TEST END ==="