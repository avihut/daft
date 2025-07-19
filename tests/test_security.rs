use anyhow::Result;
use git_worktree_workflow::{
    extract_repo_name,
    utils::{validate_branch_name, validate_repo_name},
};

/// Test security against malicious repository URLs
/// 
/// This test verifies that our URL parsing is resistant to common injection attacks
/// and path traversal attempts that could be used to escape the intended directory.
#[test]
fn test_malicious_repository_urls() {
    let malicious_urls = vec![
        // Path traversal attempts
        "https://github.com/user/../../../etc/passwd.git",
        "git@github.com:user/../../../etc/passwd.git",
        "https://github.com/user/repo/../../../etc/passwd.git",
        
        // Null byte injection
        "https://github.com/user/repo\0.git",
        "git@github.com:user/repo\0.git",
        
        // Command injection attempts in repository names
        "https://github.com/user/repo;rm -rf /.git",
        "https://github.com/user/repo&&whoami.git",
        "https://github.com/user/repo|cat /etc/passwd.git",
        
        // Unicode normalization attacks
        "https://github.com/user/rep\u{200B}o.git", // Zero-width space
        "https://github.com/user/rep\u{FEFF}o.git", // Byte order mark
        
        // Extremely long names that could cause buffer overflows
        "https://github.com/user/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.git",
        
        // Invalid characters that should be rejected
        "https://github.com/user/repo with spaces.git",
        "https://github.com/user/repo:with:colons.git",
        "https://github.com/user/repo<script>alert('xss')</script>.git",
    ];

    for url in malicious_urls {
        let result = extract_repo_name(url);
        
        // The function should either:
        // 1. Successfully extract a safe repository name, or
        // 2. Return an error for clearly malicious input
        match result {
            Ok(repo_name) => {
                // If extraction succeeded, the result must be safe
                assert!(!repo_name.contains(".."), "Path traversal in extracted name: {}", repo_name);
                assert!(!repo_name.contains('\0'), "Null byte in extracted name: {}", repo_name);
                assert!(!repo_name.contains(';'), "Command separator in extracted name: {}", repo_name);
                assert!(!repo_name.contains('&'), "Command operator in extracted name: {}", repo_name);
                assert!(!repo_name.contains('|'), "Pipe operator in extracted name: {}", repo_name);
                assert!(!repo_name.contains('<'), "Redirection in extracted name: {}", repo_name);
                assert!(!repo_name.contains('>'), "Redirection in extracted name: {}", repo_name);
                assert!(repo_name.len() < 256, "Extracted name too long: {} chars", repo_name.len());
            }
            Err(_) => {
                // Errors are acceptable for malicious input
            }
        }
    }
}

/// Test security of branch name validation
/// 
/// Verifies that branch name validation prevents injection attacks and
/// ensures only safe branch names are accepted.
#[test]
fn test_malicious_branch_names() {
    let malicious_branch_names = vec![
        // Path traversal
        "../../../etc/passwd",
        "../../.ssh/id_rsa",
        "../.git/config",
        
        // Command injection
        "branch; rm -rf /",
        "branch && cat /etc/passwd",
        "branch | whoami",
        "branch $(whoami)",
        "branch `ls -la`",
        
        // Git reference vulnerabilities  
        ".git/hooks/pre-commit",
        "refs/../config",
        "HEAD~1",
        
        // Null bytes and control characters
        "branch\0",
        "branch\x00",
        "branch\x01\x02\x03",
        "branch\n\r",
        
        // Unicode attacks
        "branch\u{200B}", // Zero-width space
        "branch\u{FEFF}", // BOM
        "branch\u{2028}", // Line separator
        
        // Extremely long names  
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // Very long but not dynamic
        
        // Hidden/system files
        ".hidden",
        "..hidden",
        ".gitignore",
    ];

    for branch_name in malicious_branch_names {
        let result = validate_branch_name(branch_name);
        
        // All malicious branch names should be rejected
        assert!(result.is_err(), "Malicious branch name was accepted: {}", branch_name);
    }
}

/// Test security of repository name validation  
/// 
/// Ensures repository name validation prevents directory traversal and
/// other attacks that could affect filesystem operations.
#[test]
fn test_malicious_repo_names() {
    let malicious_repo_names = vec![
        // Path traversal
        "../../../etc",
        "../../passwd", 
        "../.ssh",
        
        // Absolute paths
        "/etc/passwd",
        "/home/user/.ssh/id_rsa",
        "C:\\Windows\\System32",
        
        // Command injection
        "repo; rm -rf /",
        "repo && whoami",
        "repo | cat /etc/passwd",
        
        // Special directories
        ".",
        "..",
        "...",
        ".git",
        ".ssh",
        
        // Control characters
        "repo\0name",
        "repo\nname",
        "repo\rname",
        "repo\tname",
        
        // Path separators
        "repo/name",
        "repo\\name",
        "repo:name",
        
        // Empty or whitespace
        "",
        " ",
        "\t",
        "\n",
        
        // Very long names
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    ];

    for repo_name in malicious_repo_names {
        let result = validate_repo_name(repo_name);
        
        // All malicious repository names should be rejected
        assert!(result.is_err(), "Malicious repo name was accepted: {}", repo_name);
    }
}

/// Test that valid, safe inputs are still accepted
/// 
/// Ensures our security hardening doesn't break legitimate use cases.
#[test]
fn test_legitimate_inputs_still_work() -> Result<()> {
    // Valid repository URLs
    let valid_urls = vec![
        "https://github.com/user/valid-repo.git",
        "git@github.com:user/valid-repo.git", 
        "https://gitlab.com/group/subgroup/project.git",
        "git@bitbucket.org:team/project.git",
    ];
    
    for url in valid_urls {
        let result = extract_repo_name(url);
        assert!(result.is_ok(), "Valid URL was rejected: {}", url);
    }
    
    // Valid branch names
    let valid_branches = vec![
        "main",
        "master", 
        "develop",
        "feature/user-auth",
        "bugfix/login-issue",
        "hotfix/critical-fix",
        "feature-123",
        "release-1.2.3",
    ];
    
    for branch in valid_branches {
        let result = validate_branch_name(branch);
        assert!(result.is_ok(), "Valid branch name was rejected: {}", branch);
    }
    
    // Valid repository names
    let valid_repos = vec![
        "my-project",
        "awesome_tool",
        "project123",
        "git-worktree-workflow",
        "react-app",
        "backend-api",
    ];
    
    for repo in valid_repos {
        let result = validate_repo_name(repo);
        assert!(result.is_ok(), "Valid repo name was rejected: {}", repo);
    }
    
    Ok(())
}

/// Test for potential buffer overflow conditions
/// 
/// Verifies that extremely large inputs are handled gracefully.
#[test]
fn test_large_input_handling() {
    // Test very large but potentially valid inputs
    let large_inputs = vec!(
        // Large repository URL
        format!("https://github.com/user/{}.git", "a".repeat(200)),
        
        // Large branch name  
        "b".repeat(300),
        
        // Large repo name
        "c".repeat(400),
    );
    
    // These should either be handled gracefully or rejected with clear errors
    // They should never cause panics, crashes, or memory corruption
    for input in large_inputs {
        // Test URL parsing
        let _ = extract_repo_name(&input);
        
        // Test branch validation
        let _ = validate_branch_name(&input);
        
        // Test repo validation  
        let _ = validate_repo_name(&input);
        
        // If we reach here without panicking, the test passes
    }
}

/// Test input sanitization edge cases
/// 
/// Tests various edge cases in input sanitization to ensure robust handling.
#[test] 
fn test_input_sanitization_edge_cases() {
    let edge_cases = vec![
        // Mixed valid/invalid characters
        "valid-name/../invalid",
        "good_start; rm -rf /", 
        "normal-repo\0hidden",
        
        // Borderline length inputs
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", // Very long (255 chars)
        "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy", // Just over (256 chars)
        
        // Unicode edge cases
        "café", // Accented characters
        "项目", // Non-Latin scripts
        "🚀project", // Emoji
        
        // Whitespace variations
        " leading-space",
        "trailing-space ",
        "  double-space  ",
        "\tpwn\t",
        
        // Case variations of dangerous patterns
        "BRANCH; RM -RF /",
        "Branch && WhoAmI",
        "../ETC/PASSWD",
    ];
    
    for input in edge_cases {
        // These should all be handled safely without causing security issues
        let _ = extract_repo_name(&format!("https://github.com/user/{}.git", input));
        let _ = validate_branch_name(input);
        let _ = validate_repo_name(input);
        
        // Success means no panics or undefined behavior occurred
    }
}