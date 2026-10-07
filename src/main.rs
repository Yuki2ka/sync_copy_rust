mod clipboard;
mod events;
mod explorer;
mod gui;
mod hotkey;
mod layout;
mod state;
mod sync;

use anyhow::Result;
use fltk::app;
use fltk::prelude::*;
use std::sync::{Arc, Mutex};

fn main() -> Result<()> {
	let app = app::App::default();
	let (tx, rx) = app::channel::<state::Msg>();

	let hotkey_tx = tx.clone();
	std::thread::spawn(move || {
		if let Err(e) = hotkey::start_listener(move || {
			let _ = hotkey_tx.send(state::Msg::HotkeyClipboard(clipboard::get_clipboard_files()));
			app::awake();
		}) {
			eprintln!("Hotkey listener error: {e}");
		}
	});

	#[cfg(windows)]
	{
		state::start_foreground_hook(tx.clone());
	}

	let clipboard_tx = tx.clone();
	std::thread::spawn(move || {
		let mut last_files: Vec<std::path::PathBuf> = Vec::new();

		loop {
			std::thread::sleep(std::time::Duration::from_millis(250));

			let files = clipboard::get_clipboard_files().unwrap_or_default();
			if files != last_files {
				last_files = files.clone();
				let _ = clipboard_tx.send(state::Msg::ClipboardChanged(files));
				app::awake();
			}
		}
	});

	let mut ui = gui::build_window(&tx);
	let state = Arc::new(Mutex::new(state::State::new()));
	gui::configure_callbacks(&mut ui, &tx, &state);
	ui.win.show();
	ui.win.set_on_top();

	loop {
		while let Some(msg) = rx.recv() {
			events::handle_message(msg, &state, &mut ui, &tx);
		}

		if !app.wait() {
			break;
		}
	}

	Ok(())
}