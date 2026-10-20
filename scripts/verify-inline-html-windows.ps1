param([switch]$CheckOnly)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$probe = Join-Path ([System.IO.Path]::GetTempPath()) ('grove-inline-html-windows-probe-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force (Join-Path $probe 'src') | Out-Null
Copy-Item (Join-Path $PSScriptRoot 'verify-inline-html-windows.rs') (Join-Path $probe 'src/main.rs') -Force
Copy-Item (Join-Path $repo 'src/platform/inline_html_windows.rs') (Join-Path $probe 'src/inline_html_windows.rs') -Force
# A probe-only accessor is appended to the copied module. Production has no JS bridge.
Add-Content (Join-Path $probe 'src/inline_html_windows.rs') @'
pub fn probe_focus(document: &Document) {
    unsafe { document.state.view.borrow().as_ref().unwrap().controller().MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC).unwrap(); }
}
pub fn probe_native(document: &Document) -> ICoreWebView2 {
    document.state.view.borrow().as_ref().unwrap().webview()
}
'@
@'
[package]
name = "grove-inline-html-windows-probe"
version = "0.0.0"
edition = "2021"
[dependencies]
wry = { version = "=0.57.0", default-features = false, features = ["os-webview"] }
webview2-com = "0.39"
windows = { version = "0.62", features = ["Win32_Foundation", "Win32_Graphics_Gdi", "Win32_System_Com", "Win32_System_Ole", "Win32_UI_Shell", "Win32_UI_WindowsAndMessaging", "Win32_UI_Input_KeyboardAndMouse"] }
raw-window-handle = "0.6"
tempfile = "3"
'@ | Set-Content (Join-Path $probe 'Cargo.toml')
Write-Host "Probe source and Cargo output retained in $probe"
if ($CheckOnly) { cargo check --manifest-path (Join-Path $probe 'Cargo.toml') } else { cargo run --manifest-path (Join-Path $probe 'Cargo.toml') }
if ($LASTEXITCODE -ne 0) { throw "Windows HTML backend probe failed ($LASTEXITCODE)" }
