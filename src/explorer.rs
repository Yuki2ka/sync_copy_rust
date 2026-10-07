use std::path::PathBuf;
use std::process::Command;

#[cfg(windows)]
pub fn get_active_explorer_path() -> Option<PathBuf> {
	let ps_script = r#"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Native {
	[DllImport("user32.dll")]
	public static extern IntPtr GetForegroundWindow();
}
"@
$fg = [Native]::GetForegroundWindow().ToInt64()
$shell = New-Object -ComObject Shell.Application
$win = $shell.Windows() | Where-Object { [int64]$_.HWND -eq $fg } | Select-Object -First 1
if ($win -and $win.Document -and $win.Document.Folder -and $win.Document.Folder.Self) {
	$win.Document.Folder.Self.Path
}
"#;

	let output = Command::new("powershell")
		.args(["-NoProfile", "-NonInteractive", "-Command", ps_script])
		.output()
		.ok()?;

	if !output.status.success() {
		return None;
	}

	let path = String::from_utf8(output.stdout).ok()?.trim().to_string();
	if path.is_empty() {
		None
	} else {
		Some(PathBuf::from(path))
	}
}

#[cfg(not(windows))]
pub fn get_active_explorer_path() -> Option<PathBuf> {
	None
}