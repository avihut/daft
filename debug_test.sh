#!/bin/bash

echo "Debug test script running..."
echo "Current directory: $(pwd)"
echo "PATH: $PATH"

# Test basic commands
echo "Testing basic commands..."
which git && echo "Git found" || echo "Git not found"
which awk && echo "AWK found" || echo "AWK not found"
which basename && echo "basename found" || echo "basename not found"

# Test git worktree commands
echo "Testing git worktree commands..."
which git-worktree-init && echo "git-worktree-init found" || echo "git-worktree-init not found"
which git-worktree-clone && echo "git-worktree-clone found" || echo "git-worktree-clone not found"

# Test directory operations
echo "Testing directory operations..."
mkdir -p test-dir && echo "Directory creation works" || echo "Directory creation failed"
touch test-file && echo "File creation works" || echo "File creation failed"
ls -la test-dir test-file 2>/dev/null && echo "Files exist" || echo "Files missing"

# Test git operations
echo "Testing git operations..."
git config --global user.name "Test User" 2>/dev/null || echo "Git config failed"
git config --global user.email "test@example.com" 2>/dev/null || echo "Git config failed"

# Test temporary directory
echo "Testing temporary directory..."
mkdir -p /tmp/test-temp && echo "Temp directory works" || echo "Temp directory failed"

echo "Debug test completed"