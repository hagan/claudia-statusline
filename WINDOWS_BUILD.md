# Building claudia-statusline on Windows

This guide covers building claudia-statusline on Windows 11 with the MSVC toolchain.

## Prerequisites

### Required Tools

1. **Rust** (MSVC toolchain):
   ```powershell
   # Install rustup
   Invoke-WebRequest -Uri https://win.rustup.rs/x86_64 -OutFile $env:TEMP\rustup-init.exe
   & $env:TEMP\rustup-init.exe --default-toolchain stable -y

   # Restart PowerShell, then verify
   rustup show
   ```

2. **Visual Studio Build Tools 2022** with C++ workload:
   ```powershell
   # Install Build Tools
   winget install Microsoft.VisualStudio.2022.BuildTools

   # Add C++ workload (requires Admin PowerShell)
   & "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vs_installer.exe" modify `
     --installPath "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools" `
     --add Microsoft.VisualStudio.Workload.VCTools `
     --includeRecommended
   ```

3. **Git for Windows**:
   ```powershell
   winget install Git.Git
   ```

## Important: Git Bash PATH Conflicts

Git for Windows includes Unix tools (like `/usr/bin/link`) that conflict with MSVC's `link.exe`. This causes Rust compilation to fail with errors like:

```
link: extra operand 'C:\\Users\\...'
Try 'link --help' for more information.
```

### Solution: PATH Ordering

Ensure Windows system directories come before Git paths. Add this to your PowerShell profile:

```powershell
# Fix PATH ordering: Prioritize Windows system tools over Git Bash Unix tools
$systemPaths = @(
    "$env:SystemRoot\system32",
    "$env:SystemRoot",
    "$env:SystemRoot\System32\Wbem",
    "$env:SystemRoot\System32\WindowsPowerShell\v1.0\",
    "$env:SystemRoot\System32\OpenSSH\"
)

$currentPath = $env:PATH -split ';'
$otherPaths = $currentPath | Where-Object {
    $path = $_
    -not ($systemPaths | Where-Object { $path -eq $_ })
}

$env:PATH = ($systemPaths + $otherPaths) -join ';'
```

## Building

1. **Clone the repository**:
   ```powershell
   cd $env:USERPROFILE\Projects
   git clone https://github.com/hagan/claudia-statusline.git
   cd claudia-statusline
   ```

2. **Activate MSVC environment** (if using a custom PowerShell profile with `Enable-MSVC`):
   ```powershell
   Enable-MSVC
   ```

   Or manually set up MSVC environment:
   ```powershell
   $vsPath = & "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe" `
     -latest -property installationPath
   & "$vsPath\VC\Auxiliary\Build\vcvars64.bat"
   ```

3. **Build the project**:
   ```powershell
   cargo build --release
   ```

   Build time: ~45 seconds on first compile.

4. **Verify the binary**:
   ```powershell
   .\target\release\statusline.exe --help
   ```

## Configuration

1. **Generate config file**:
   ```powershell
   .\target\release\statusline.exe generate-config
   ```

   Config location: `C:\Users\<username>\AppData\Roaming\claudia-statusline\config.toml`

2. **Configure Claude Code**:

   Create or edit `~\.claude\settings.json`:
   ```json
   {
     "statusLine": {
       "type": "command",
       "command": "C:\\Users\\<username>\\Projects\\claudia-statusline\\target\\release\\statusline.exe",
       "padding": 0
     }
   }
   ```

   **Important**: The `"type": "command"` field is required for Claude Code to recognize the statusline.

   Or use PowerShell:
   ```powershell
   $settingsPath = "$env:USERPROFILE\.claude\settings.json"
   $settings = @{
       statusLine = @{
           type = "command"
           command = "C:\Users\$env:USERNAME\Projects\claudia-statusline\target\release\statusline.exe"
           padding = 0
       }
   } | ConvertTo-Json -Depth 10

   $settings | Out-File -FilePath $settingsPath -Encoding UTF8
   ```

3. **Restart Claude Code** to load the statusline (though sometimes it works without restart!).

## Troubleshooting

### Linker Errors

**Error**: `link.exe not found` or `link: extra operand`

**Solution**:
- Ensure Visual Studio Build Tools C++ workload is installed
- Check PATH ordering (see above section)
- Restart PowerShell after installing Build Tools

### SQLite Compilation Errors

If you encounter SQLite build errors, ensure GCC or MSVC is properly configured.

### Rust Toolchain

To verify which Rust toolchain is active:
```powershell
rustup show
```

Should show: `x86_64-pc-windows-msvc (default)`

### Testing in PowerShell vs Git Bash

**Important**: Always build in native PowerShell, not Git Bash. Git Bash inherits Unix-style PATH that causes linker conflicts.

## Alternative: GNU Toolchain

If you prefer the GNU toolchain instead of MSVC:

1. Install MinGW via Scoop:
   ```powershell
   scoop install mingw-winlibs
   ```

2. Install GNU Rust toolchain:
   ```powershell
   rustup toolchain install stable-x86_64-pc-windows-gnu
   rustup default stable-x86_64-pc-windows-gnu
   ```

3. Configure Cargo (create `.cargo/config.toml` in project directory):
   ```toml
   [target.x86_64-pc-windows-gnu]
   linker = "C:\\Users\\<username>\\scoop\\apps\\mingw-winlibs\\current\\bin\\gcc.exe"
   ar = "C:\\Users\\<username>\\scoop\\apps\\mingw-winlibs\\current\\bin\\ar.exe"
   ```

4. Build:
   ```powershell
   cargo build --release --target x86_64-pc-windows-gnu
   ```

**Note**: MSVC toolchain is recommended for better Windows compatibility.

## Build System Considerations

For CI/CD pipelines (GitHub Actions):

```yaml
- name: Install MSVC Build Tools
  run: |
    winget install Microsoft.VisualStudio.2022.BuildTools --silent

- name: Add C++ workload
  shell: pwsh
  run: |
    & "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vs_installer.exe" modify `
      --installPath "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools" `
      --add Microsoft.VisualStudio.Workload.VCTools `
      --includeRecommended `
      --quiet `
      --wait

- name: Build
  shell: pwsh
  run: cargo build --release
```

## Performance

- First compile: ~45 seconds
- Incremental rebuilds: ~5 seconds
- Binary size: ~8 MB (release build)
- Runtime dependencies: None (statically linked SQLite)

## See Also

- [Main README](README.md)
- [Configuration Guide](docs/CONFIGURATION.md)
- [Troubleshooting](docs/TROUBLESHOOTING.md)
