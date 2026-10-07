use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;
use anyhow::Result;
use crate::sync::SyncStats;
use std::sync::mpsc::{self, Sender};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
	SKIP,
	SOURCE,
	DESTINATION,
}

#[derive(Debug, Clone)]
pub struct Capture {
	pub id: i32,
	pub paths: Vec<PathBuf>,
	pub role: Role,
}

impl Capture {
	pub fn new(id: i32, paths: Vec<PathBuf>) -> Self {
		Self {
			id,
			paths,
			role: Role::SKIP,
		}
	}

	pub fn summary(&self) -> String {
		if self.paths.is_empty() {
			return "(empty)".to_string();
		}
		let names: Vec<_> = self.paths.iter().take(3).map(|p| p.display().to_string()).collect();
		let mut text = names.join(" | ");
		if self.paths.len() > 3 {
			text.push_str(&format!(" (+{} more)", self.paths.len() - 3));
		}
		text
	}
}

#[derive(Debug)]
pub enum Msg {
	ClipboardChanged(Vec<PathBuf>),
	HotkeyClipboard(Result<Vec<PathBuf>>),
	ExplorerChanged(PathBuf),
	SetRoleRowIndex(usize, Role),
	PreviewCaptureRowIndex(usize),
	ClearCaptures,
	SyncNow,
	SyncNowClose,
	SyncDone(Result<SyncStats>),
	RetrySync(Vec<PathBuf>, Vec<PathBuf>),
	WindowClose,
}

pub struct State {
	pub captures: Vec<Capture>,
	pub explorer_path: Option<PathBuf>,
	pub next_id: i32,
	pub seen: HashSet<String>,
	pub syncing: bool,
	pub use_recycle_bin: bool,
	pub active_preview_id: Option<i32>,
	pub skip_unchanged: bool,
	pub sync_close_on_success: bool,
}

impl State {
	pub fn new() -> Self {
		Self {
			captures: Vec::new(),
			explorer_path: None,
			next_id: 1,
			seen: HashSet::new(),
			syncing: false,
			use_recycle_bin: true,
			active_preview_id: None,
			skip_unchanged: true,
			sync_close_on_success: false,
		}
	}

	pub fn update_explorer(&mut self, path: PathBuf) {
		self.explorer_path = Some(path);
	}

	pub fn has_user_captures(&self) -> bool {
		!self.captures.is_empty()
	}

	pub fn clear_user_captures(&mut self) {
		self.captures.clear();
		self.seen.clear();
		self.next_id = 1;
		self.active_preview_id = None;
	}
}

#[cfg(windows)]
static EXPLORER_TX: OnceLock<Sender<Msg>> = OnceLock::new();

#[cfg(windows)]
unsafe extern "system" fn win_event_proc(
	_hwobj: winapi::shared::windef::HWINEVENTHOOK,
	event: u32,
	_hwnd: winapi::shared::windef::HWND,
	_id_obj: i32,
	_id_child: i32,
	_id_event_thread: u32,
	_dwms_event_time: u32,
) {
	use winapi::um::winuser::EVENT_SYSTEM_FOREGROUND;
	
	if event == EVENT_SYSTEM_FOREGROUND {
		if let Some(tx) = EXPLORER_TX.get() {
			if let Some(path) = crate::explorer::get_active_explorer_path() {
				let _ = tx.send(Msg::ExplorerChanged(path));
			}
		}
	}
}

#[cfg(windows)]
pub fn start_foreground_hook(tx: fltk::app::Sender<Msg>) {
	use winapi::um::winuser::{SetWinEventHook, GetMessageW, MSG, DispatchMessageW, TranslateMessage};
	
	let (raw_tx, raw_rx): (Sender<Msg>, mpsc::Receiver<Msg>) = mpsc::channel();
	EXPLORER_TX.set(raw_tx).ok();
	
	let forward_tx = tx.clone();
	std::thread::spawn(move || {
		loop {
			if let Ok(msg) = raw_rx.recv() {
				let _ = forward_tx.send(msg);
				fltk::app::awake();
			}
		}
	});
	
	std::thread::spawn(move || {
		unsafe {
			SetWinEventHook(
				winapi::um::winuser::EVENT_SYSTEM_FOREGROUND,
				winapi::um::winuser::EVENT_SYSTEM_FOREGROUND,
				std::ptr::null_mut(),
				Some(win_event_proc),
				0,
				0,
				winapi::um::winuser::WINEVENT_OUTOFCONTEXT | winapi::um::winuser::WINEVENT_SKIPOWNPROCESS,
			);

			let mut msg: MSG = std::mem::zeroed();
			while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
				TranslateMessage(&msg);
				DispatchMessageW(&msg);
			}
		}
	});
}