use anyhow::Result;
use std::path::PathBuf;

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;

#[cfg(windows)]
pub fn get_clipboard_files() -> Result<Vec<PathBuf>> {
	use winapi::um::shellapi::{DragQueryFileW, HDROP};
	use winapi::um::winuser::{
		CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
	};

	const CF_HDROP: u32 = 15;
	const CF_UNICODETEXT: u32 = 13;
	const CF_TEXT: u32 = 1;

	unsafe {
		let hwnd = winapi::um::winuser::GetForegroundWindow();
		if OpenClipboard(hwnd) == 0 {
			return Ok(Vec::new());
		}

		let mut files = Vec::new();

		if IsClipboardFormatAvailable(CF_HDROP) != 0 {
			let handle = GetClipboardData(CF_HDROP);
			if !handle.is_null() {
				let hdrop = handle as HDROP;
				let count = DragQueryFileW(hdrop, u32::MAX, std::ptr::null_mut(), 0);

				for i in 0..count {
					let len = DragQueryFileW(hdrop, i, std::ptr::null_mut(), 0);
					if len == 0 {
						continue;
					}

					let mut buf = vec![0u16; len as usize + 1];
					let written =
						DragQueryFileW(hdrop, i, buf.as_mut_ptr(), buf.len() as u32);
					if written > 0 {
						let path =
							PathBuf::from(OsString::from_wide(&buf[..written as usize]));
						if path.exists() {
							files.push(path);
						}
					}
				}
			}
		}

		if files.is_empty() {
			let mut fmt = None;
			if IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 {
				fmt = Some(CF_UNICODETEXT);
			} else if IsClipboardFormatAvailable(CF_TEXT) != 0 {
				fmt = Some(CF_TEXT);
			}

			if let Some(f) = fmt {
				let handle = GetClipboardData(f);
				if !handle.is_null() {
					let text = if f == CF_UNICODETEXT {
						let mut len_words = 0;
						loop {
							let p = (handle as *const u16).add(len_words);
							if *p == 0u16 {
								break;
							}
							len_words += 1;
						}
						let u16s = std::slice::from_raw_parts(
							handle as *const u16,
							len_words,
						);
						String::from_utf16_lossy(u16s)
					} else {
						let mut len_bytes = 0;
						loop {
							let p = (handle as *const u8).add(len_bytes);
							if *p == 0 {
								break;
							}
							len_bytes += 1;
						}
						let bytes =
							std::slice::from_raw_parts(handle as *const u8, len_bytes);
						String::from_utf8_lossy(bytes).to_string()
					};
					files = parse_path_lines(&text);
				}
			}
		}

		CloseClipboard();
		Ok(files)
	}
}

#[cfg(target_os = "linux")]
pub fn get_clipboard_files() -> Result<Vec<PathBuf>> {
	use std::process::Command;

	if let Ok(out) = Command::new("wl-paste")
		.args(["--type", "text/uri-list"])
		.output()
	{
		if out.status.success() {
			let text = String::from_utf8_lossy(&out.stdout);
			let paths = parse_uri_list(&text);
			if !paths.is_empty() {
				return Ok(paths);
			}
		}
	}

	if let Ok(out) = Command::new("xclip")
		.args(["-selection", "clipboard", "-o", "-t", "text/uri-list"])
		.output()
	{
		if out.status.success() {
			let text = String::from_utf8_lossy(&out.stdout);
			let paths = parse_uri_list(&text);
			if !paths.is_empty() {
				return Ok(paths);
			}
		}
	}

	let mut text_data = None;
	if let Ok(out) = Command::new("wl-paste")
		.args(["--type", "text/plain"])
		.output()
	{
		if out.status.success() {
			text_data = Some(String::from_utf8_lossy(&out.stdout).to_string());
		}
	}

	if text_data.is_none() {
		if let Ok(out) = Command::new("xclip")
			.args(["-selection", "clipboard", "-o", "-t", "text/plain"])
			.output()
		{
			if out.status.success() {
				text_data = Some(String::from_utf8_lossy(&out.stdout).to_string());
			}
		}
	}

	if let Some(text) = text_data {
		Ok(parse_path_lines(&text))
	} else {
		Ok(Vec::new())
	}
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn get_clipboard_files() -> Result<Vec<PathBuf>> {
	Ok(Vec::new())
}

#[cfg(target_os = "linux")]
fn parse_uri_list(text: &str) -> Vec<PathBuf> {
	let mut files = Vec::new();

	for line in text.lines() {
		let line = line.trim();
		if line.starts_with('#') || line.is_empty() {
			continue;
		}

		if let Some(path) = line.strip_prefix("file://") {
			let path = path.strip_prefix("//").unwrap_or(path);
			let decoded =
				percent_encoding::percent_decode_str(path).decode_utf8_lossy();
			let p = PathBuf::from(decoded.as_ref());
			if p.exists() {
				files.push(p);
			}
		} else {
			let p = PathBuf::from(line);
			if p.exists() {
				files.push(p);
			}
		}
	}

	files
}

fn parse_path_lines(text: &str) -> Vec<PathBuf> {
	let mut paths = Vec::new();

	for line in text.lines() {
		let line = line.trim();
		if line.is_empty() {
			continue;
		}
		let cleaned = if (line.starts_with('"') && line.ends_with('"'))
			|| (line.starts_with('\'') && line.ends_with('\''))
		{
			&line[1..line.len() - 1]
		} else {
			line
		};
		let p = PathBuf::from(cleaned);
		if p.exists() {
			paths.push(p);
		}
	}

	paths
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_path_lines() {
		let text = r#""C:\Users\test\file.txt"
		'/home/user/file.txt'
		C:\Users\test\other.txt
		"#;
		let paths = parse_path_lines(text);
		assert_eq!(paths.len(), 3);
		assert_eq!(paths[0], PathBuf::from("C:\\Users\\test\\file.txt"));
		assert_eq!(paths[1], PathBuf::from("/home/user/file.txt"));
	}

	#[test]
	fn test_parse_path_lines_empty() {
		let paths = parse_path_lines("");
		assert!(paths.is_empty());
	}
}
