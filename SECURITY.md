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

## ant Enrichment Key Handling

The optional `ant` enrichment (model metadata + per-account usage/cost, off by default)
talks to the Anthropic API and therefore handles API keys. Its security model is built on
**least privilege**, a **never-log / never-cache** invariant, and a CI regression guard —
not on the repository `.claudeignore`, whose scope is deliberately limited (see the caveat
below).

### Least-privilege key mapping

Three `ant` subcommands open outbound network connections. Only two of them resolve a
credential at all, and those two resolve **different** credentials against **different** API
surfaces; the third is keyless. The separation is enforced **structurally** by which command
resolves which credential:

| Command            | Credential          | API surface used        |
| ------------------ | ------------------- | ----------------------- |
| `ant sync-models`  | standard API key    | Models API only         |
| `ant sync-usage`   | per-account Admin key (`admin_key_command`) | Usage & Cost (Admin) API only |
| `ant sync-pricing` | none (keyless)      | public LiteLLM snapshot on raw.githubusercontent.com |

The standard key is used **only** for the Models API; the Admin key is used **only** for the
Usage & Cost API. `sync-models` never resolves an Admin key, and `sync-usage` never uses the
standard key. There is no code path that grants the standard key Admin scope or vice versa, so
a misconfiguration cannot escalate a key beyond the single API it was provisioned for.

`ant sync-pricing` is the one **non-Anthropic** egress in the feature: it fetches a public
LiteLLM price snapshot from `raw.githubusercontent.com`, resolves no credential on any code
path, and sends no request header or query parameter derived from your configuration or
session data. The fetch is pinned to `https` for the initial request **and** for every
redirect, with a bounded redirect count (`--proto =https`, `--proto-redir =https`,
`--max-redirs 5` in `src/pricing/fetch.rs`), so a redirect cannot downgrade the chain to
cleartext. Rendering never performs this fetch — only the explicit, out-of-band
`ant sync-pricing` command does.

### Key-handling invariants

- **Never logged.** Keys are not written to stdout, stderr, the debug log, or any diagnostic
  output. `ant doctor` reports credential **source labels** only and never resolves a key in
  passive mode.
- **Never cached.** The cache structs (`ModelsCache` and `UsageCache` in `src/ant/cache.rs`,
  and `PriceCache` in `src/pricing/cache.rs`) carry **no key/secret field** — only model
  metadata, numeric spend/token totals and per-token price rates. A key cannot be serialized
  to disk because there is nowhere on any of these schemas to put it, and `PriceCache` is
  written by a command that never resolves a key in the first place.
- **Materialized only for the single API call.** A key is resolved via the credential-command
  seam (the `admin_key_command` argv for usage; the standard-key env/auth chain for models)
  and handed to `curl` via a leak-free STDIN config (never on argv, never on disk), existing
  in memory only for the duration of that one request.
- **Process env is in-scope by design, not a leak vector.** A key passed via
  `ANTHROPIC_API_KEY` legitimately lives in the process environment transiently while a sync
  runs. This is expected and is therefore **NOT** part of the audited artifact set — the leak
  scanner (`scan_artifacts_for_keys`) explicitly never reads `std::env` (D-18); scanning it
  would only produce false positives.

### `.claudeignore` home-cache scope caveat

The repository-root `.claudeignore` (header scope note at `.claudeignore:9-19`) excludes any
**in-tree** `ant` cache (e.g. when a test points `XDG_CACHE_HOME` at a directory under the
repo) from agent reads. It **cannot** protect the real user-home cache at
`~/.cache/claudia-statusline/ant/` (or the macOS `~/Library/Caches` equivalent), which lives
outside the repository and is therefore outside this file's reach. Do not read `.claudeignore`
as a guarantee about files under the user's home directory.

The real defense-in-depth is **structural**, not the ignore file:

1. The cache struct stores no key field (so a key can never be written to any cache).
2. The key is never logged and is materialized only for the single API call.
3. The CI leak-scan test (`tests/ant_audit_tests.rs::written_cache_artifacts_contain_no_key`)
   runs the real cache writers under an isolated cache root and asserts, via the shared
   `scan_artifacts_for_keys` scanner, that **no** written artifact (model cache, per-account
   usage cache, or debug log) contains an `sk-ant-` string — a permanent regression guard
   that the same scanner also powers as the `ant doctor` security self-audit.
