# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 2.15.x  | :white_check_mark: |
| 2.14.x  | :white_check_mark: |
| 2.13.x  | :white_check_mark: |
| < 2.13  | :x:                |

## Automated Security Scanning

The CI pipeline (`.github/workflows/security.yml`) runs the following on every push and nightly schedule:

### Continuous Security Monitoring
- **cargo-audit**: Scans for known vulnerabilities on every push and daily
- **cargo-deny**: Checks supply chain security, licenses, and advisories
- **Dependency Review**: Automatic review of dependency changes in PRs
- **License Compliance**: Ensures all dependencies have compatible licenses

## Security Hardening

Security hardening highlights:

### Terminal Output Sanitization
- All untrusted user input is sanitized before terminal display
- ANSI escape sequences are stripped to prevent injection attacks
- Control characters are removed (except tab, newline, carriage return)
- Applied to: Git branch names, model names, directory paths
- Function: `sanitize_for_terminal()` in utils.rs

### Git Operation Resilience
- Git operations enforce a soft timeout (default 200ms)
- Configurable via `config.git.timeout_ms` or `STATUSLINE_GIT_TIMEOUT_MS` env var
- Processes are killed if timeout exceeded with INFO logging
- `GIT_OPTIONAL_LOCKS=0` prevents lock conflicts
- Automatic retry on failure (2 attempts with 100ms backoff)

### Input Validation
- All user-supplied paths from JSON input are validated and canonicalized
- Directory traversal attempts are blocked (e.g., "../../../etc")
- Null byte injection is prevented
- Command injection via special characters is blocked
- Only verified git repositories can have git operations performed
- Transcript files are restricted to .jsonl extension (case-insensitive)
- Transcript files are limited to 10MB to prevent memory exhaustion

### Security Functions
- `sanitize_for_terminal()` in utils.rs - Removes control chars and ANSI escapes
- `validate_git_directory()` in git.rs - Validates directories before running git commands
- `validate_transcript_file()` in utils.rs - Validates transcript file paths before reading
- `execute_git_with_timeout()` in git_utils.rs - Enforces timeout on git operations

### Security Tests
The following security tests ensure our protection mechanisms work:
- `test_validate_git_directory_security` - Tests directory path validation
- `test_malicious_path_inputs` - Tests protection against malicious git paths
- `test_validate_transcript_file_security` - Tests transcript file validation
- `test_malicious_transcript_paths` - Tests protection against malicious transcript paths

## Reporting a Vulnerability

If you discover a security vulnerability in Claudia Statusline, please:

1. **Do NOT** create a public GitHub issue
2. Email the details to the maintainer (see repository owner)
3. Include:
   - Description of the vulnerability
   - Steps to reproduce
   - Potential impact
   - Suggested fix (if any)

We will acknowledge receipt within 48 hours and provide updates on the fix timeline.

## Security Updates

Security updates will be released as patch versions (e.g., 2.2.1, 2.2.2) and clearly marked in the CHANGELOG.

## Known Security Issues

### Fixed Issues
- Historical fixes are documented in `CHANGELOG.md`. No open advisories at this time.

### Currently Known Issues
- None reported.

## Best Practices for Users

1. Always use the latest version
2. Review JSON input from untrusted sources before processing
3. Run statusline with minimal privileges
4. Keep your Rust toolchain updated if building from source
5. Store transcript files in a trusted directory
6. Be aware that transcript files are limited to 10MB and must have .jsonl extension

## Security Audit History

- Automated checks run on every pull request and on a nightly schedule (see `.github/workflows/security.yml`).
- Formal audits are recorded in release notes within `CHANGELOG.md` when applicable.

## Credits

Security issues were identified and fixed by the Claudia Statusline maintainers with assistance from Claude Code Assistant.
