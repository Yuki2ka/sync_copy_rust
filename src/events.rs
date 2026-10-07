use std::sync::{Arc, Mutex};
use std::thread;

use crate::gui::{ask_permanent_delete, confirm_sync, refresh_capture_list, show_preview, is_recycle_bin_error, Ui};
use crate::state::{Msg, State, Role};
use crate::sync::{compute_sync_plan, SyncStats};
use fltk::app;
use fltk::prelude::{WidgetExt, ButtonExt};
use std::path::PathBuf;

pub fn handle_message(
	msg: Msg,
	state: &Arc<Mutex<State>>,
	ui: &mut Ui,
	tx: &app::Sender<Msg>,
) -> Option<thread::JoinHandle<()>> {
	match msg {
		Msg::ClipboardChanged(files) => {
			if !files.is_empty() {
				let sig = {
					let mut v: Vec<_> = files
						.iter()
						.map(|p| {
							std::fs::canonicalize(p)
								.unwrap_or_else(|_| p.clone())
								.display()
								.to_string()
						})
						.collect();
					v.sort();
					v.join("|")
				};

				let seen = {
					let mut s = state.lock().unwrap();
					if s.seen.contains(&sig) {
						true
					} else {
						s.seen.insert(sig);
						false
					}
				};

				if !seen {
					let mut s = state.lock().unwrap();
					let role = if !s.has_user_captures() {
						Role::SOURCE
					} else if files.len() == 1 && files[0].is_dir() {
						Role::DESTINATION
					} else {
						Role::SKIP
					};
					let mut cap = crate::state::Capture::new(s.next_id, files);
					s.next_id += 1;
					cap.role = role;
					s.captures.push(cap);
				}
			}

			refresh_capture_list(ui, tx, state);

			let s = state.lock().unwrap();
			if !s.has_user_captures() {
				ui.progress_lbl.set_label("");
				ui.stats_lbl.set_label("Ready.");
			} else {
				ui.progress_lbl.set_label("");
				ui.stats_lbl.set_label("Press Sync Now to start.");
			}
			ui.win.redraw();
			None
		}

		Msg::HotkeyClipboard(res) => match res {
			Ok(files) => {
				if !files.is_empty() {
					let sig = {
						let mut v: Vec<_> = files
							.iter()
							.map(|p| {
								std::fs::canonicalize(p)
									.unwrap_or_else(|_| p.clone())
									.display()
									.to_string()
							})
							.collect();
						v.sort();
						v.join("|")
					};

					let seen = {
						let mut s = state.lock().unwrap();
						if s.seen.contains(&sig) {
							true
						} else {
							s.seen.insert(sig);
							false
						}
					};

					if !seen {
						let mut s = state.lock().unwrap();
						let role = if !s.has_user_captures() {
							Role::SOURCE
						} else if files.len() == 1 && files[0].is_dir() {
							Role::DESTINATION
						} else {
							Role::SKIP
						};
						let mut cap = crate::state::Capture::new(s.next_id, files);
						s.next_id += 1;
						cap.role = role;
						s.captures.push(cap);
					}
				}

				refresh_capture_list(ui, tx, state);

				let s = state.lock().unwrap();
				let source_cap = s.captures.iter().find(|c| c.role == Role::SOURCE);
				if !s.has_user_captures() {
					ui.progress_lbl.set_label("No files found in clipboard.");
					ui.stats_lbl.set_label("Copy files/folders first.");
				} else if source_cap.is_some() {
					ui.progress_lbl.set_label("Source set. Click Sync Now to start.");
				} else {
					ui.progress_lbl.set_label("Set a Source before syncing.");
				}
				ui.win.redraw();
				None
			}
			Err(e) => {
				ui.progress_lbl.set_label("Clipboard read failed.");
				ui.stats_lbl.set_label(&format!("Error: {e}"));
				ui.win.redraw();
				None
			}
		},

		Msg::ExplorerChanged(path) => {
			let mut s = state.lock().unwrap();
			s.update_explorer(path.clone());
			drop(s);

			refresh_capture_list(ui, tx, state);

			ui.progress_lbl.set_label(&format!("Destination folder updated from Explorer: {}", path.display()));
			ui.win.redraw();
			None
		}

		Msg::SetRoleRowIndex(row_index, role) => {
			let mut s = state.lock().unwrap();
			let cap_id = s.captures.get(row_index).map(|c| c.id);
			if let Some(id) = cap_id {
				if role == Role::DESTINATION {
					if let Some(c) = s.captures.iter().find(|c| c.id == id) {
						if c.paths.len() != 1 {
							drop(s);
							ui.progress_lbl.set_label("Destination must be a single folder.");
							ui.win.redraw();
							return None;
						}
						if !c.paths[0].is_dir() {
							drop(s);
							ui.progress_lbl.set_label("Destination must be a folder, not a file.");
							ui.win.redraw();
							return None;
						}
					}
				}
				if role == Role::SOURCE {
					for c in &mut s.captures {
						if c.role == Role::SOURCE {
							c.role = Role::SKIP;
						}
					}
				}
				let dest_name = if role == Role::DESTINATION {
					s.captures.iter().find(|c| c.id == id).map(|c| c.paths[0].display().to_string())
				} else {
					None
				};
				if let Some(c) = s.captures.iter_mut().find(|c| c.id == id) {
					c.role = role;
				}
				s.skip_unchanged = ui.skip_chk.value();

				if let Some(name) = dest_name {
					ui.progress_lbl.set_label(&format!("Destination set: {}", name));
					ui.stats_lbl.set_label("");
				}
			}
			drop(s);

			refresh_capture_list(ui, tx, state);
			ui.win.redraw();
			None
		}

		Msg::PreviewCaptureRowIndex(row_index) => {
			let mut s = state.lock().unwrap();

			let source_paths_opt = s.captures.iter()
				.find(|c| c.role == Role::SOURCE)
				.map(|c| c.paths.clone());

			let dest_cap_opt = s.captures.get(row_index).cloned();

			let Some(source_paths) = source_paths_opt else {
				ui.progress_lbl.set_label("No source selected for preview.");
				ui.win.redraw();
				return None;
			};

			let Some(dest_cap) = dest_cap_opt else {
				ui.progress_lbl.set_label("Capture not found for preview.");
				ui.win.redraw();
				return None;
			};

			if let Some(first) = dest_cap.paths.first().cloned() {
				if !first.exists() || !first.is_dir() {
					ui.progress_lbl.set_label("Destination folder is not a valid directory.");
					ui.win.redraw();
					return None;
				}
			} else {
				ui.progress_lbl.set_label("Destination has no paths.");
				ui.win.redraw();
				return None;
			}

			let should_preview = s.active_preview_id != Some(dest_cap.id);
			if should_preview {
				s.active_preview_id = Some(dest_cap.id);
			} else {
				s.active_preview_id = None;
			}
			let skip_unchanged = s.skip_unchanged;
			let dest_path = dest_cap.paths[0].clone();
			drop(s);

			refresh_capture_list(ui, tx, state);

			if should_preview {
				match compute_sync_plan(&source_paths, &dest_path, skip_unchanged) {
					Ok(plan) => {
						show_preview(&mut ui.preview_tree, &plan);
						ui.progress_lbl.set_label(&format!("Preview ready: {} items.", plan.len()));
					}
					Err(e) => {
						ui.progress_lbl.set_label(&format!("Preview failed: {e}"));
					}
				}
			} else {
				ui.preview_tree.clear();
				ui.progress_lbl.set_label("Preview hidden.");
			}

			ui.win.redraw();
			None
		}

		Msg::ClearCaptures => {
			let mut s = state.lock().unwrap();
			s.clear_user_captures();
			drop(s);

			refresh_capture_list(ui, tx, state);
			ui.progress_lbl.set_label("List cleared.");
			ui.stats_lbl.set_label("");
			ui.win.redraw();
			None
		}

		Msg::SyncNow => {
			if !confirm_sync() {
				return None;
			}
			{
				let mut s = state.lock().unwrap();
				s.sync_close_on_success = false;
			}
			start_sync(state, ui, tx);
			None
		}

		Msg::SyncNowClose => {
			if !confirm_sync() {
				return None;
			}
			{
				let mut s = state.lock().unwrap();
				s.sync_close_on_success = true;
			}
			start_sync(state, ui, tx);
			None
		}

		Msg::WindowClose => {
			ui.win.hide();
			None
		}

		Msg::SyncDone(res) => {
			let should_close = {
				let mut s = state.lock().unwrap();
				s.syncing = false;
				let close = s.sync_close_on_success && res.is_ok();
				s.sync_close_on_success = false;
				close
			};

			match res {
				Ok(stats) => {
					ui.progress_lbl.set_label("Done.");
					ui.stats_lbl.set_label(&format!(
						"Copied: {}, Skipped: {}, Deleted: {}",
						stats.copied, stats.skipped, stats.deleted
					));
				}
				Err(e) => {
					if is_recycle_bin_error(&e.to_string()) {
						let src_paths_clone = {
							let s = state.lock().unwrap();
							let src = s.captures.iter().find(|c| c.role == Role::SOURCE);
							src.map(|c| c.paths.clone()).unwrap_or_default()
						};
						let mut dest_paths_clone = {
							let s = state.lock().unwrap();
							s.captures.iter()
								.filter(|c| c.role == Role::DESTINATION)
								.filter_map(|c| c.paths.first().cloned())
								.collect::<Vec<_>>()
						};
						if let Some(p) = {
							let s = state.lock().unwrap();
							s.explorer_path.clone()
						} {
							dest_paths_clone.push(p);
						}

						let _ = tx.send(Msg::RetrySync(src_paths_clone, dest_paths_clone));
					} else {
						ui.progress_lbl.set_label("Error.");
						ui.stats_lbl.set_label(&format!("Error: {e}"));
					}
				}
			}

			refresh_capture_list(ui, tx, state);
			ui.win.redraw();

			if should_close {
				ui.win.hide();
			}

			None
		}

		Msg::RetrySync(sources, dests) => {
			if ask_permanent_delete() {
				let mut s = state.lock().unwrap();
				s.use_recycle_bin = false;
				s.syncing = true;
				drop(s);

				ui.recycle_chk.set_value(false);
				ui.progress_lbl.set_label("Retrying without Recycle Bin...");
				ui.stats_lbl.set_label("Deleting permanently because Recycle Bin failed.");
				ui.win.redraw();

				let tx_clone = tx.clone();
				let sources = sources.clone();
				let dests = dests.clone();

				Some(thread::spawn(move || {
					let mut combined = SyncStats::default();
					for dest_path in &dests {
						if let Ok(stats) = crate::gui::sync_items_and_prune(&sources, dest_path, false) {
							combined.copied += stats.copied;
							combined.skipped += stats.skipped;
							combined.deleted += stats.deleted;
						}
					}
					let _ = tx_clone.send(Msg::SyncDone(Ok(combined)));
					app::awake();
				}))
			} else {
				let mut s = state.lock().unwrap();
				s.syncing = false;
				drop(s);

				ui.progress_lbl.set_label("Cancelled.");
				ui.stats_lbl.set_label(
					"Recycle Bin unavailable, permanent deletion cancelled.",
				);
				ui.win.redraw();
				None
			}
		}
	}
}

fn start_sync(
	state: &Arc<Mutex<State>>,
	ui: &mut Ui,
	tx: &app::Sender<Msg>,
) {
	let mut s = state.lock().unwrap();

	if s.syncing {
		ui.progress_lbl.set_label("A sync operation is already running.");
		ui.win.redraw();
		return;
	}

	let explorer_path = s.explorer_path.clone();
	let source = s.captures.iter().find(|c| c.role == Role::SOURCE);
	let dests: Vec<_> = s.captures.iter().filter(|c| c.role == Role::DESTINATION).collect();

	if source.is_none() {
		ui.progress_lbl.set_label("No Source selected!");
		ui.stats_lbl.set_label("Click Source on one capture.");
		ui.win.redraw();
		return;
	}

	if dests.is_empty() && explorer_path.is_none() {
		ui.progress_lbl.set_label("No Destination selected!");
		ui.stats_lbl.set_label("Click Destination on at least one capture.");
		ui.win.redraw();
		return;
	}

	let src_paths = source.unwrap().paths.clone();
	let mut dest_paths: Vec<PathBuf> = dests.iter().filter_map(|c| c.paths.first().cloned()).collect();
	if let Some(p) = explorer_path {
		dest_paths.push(p);
	}

	if dest_paths.is_empty() {
		ui.progress_lbl.set_label("No destination paths available.");
		ui.win.redraw();
		return;
	}

	s.syncing = true;
	s.active_preview_id = None;
	let use_recycle = s.use_recycle_bin;
	drop(s);

	ui.progress_lbl.set_label("Syncing...");
	ui.stats_lbl.set_label("");
	ui.win.redraw();

	let tx_clone = tx.clone();
	thread::spawn(move || {
		let mut combined = SyncStats::default();
		let mut fatal_errors = Vec::new();
		let mut saw_recycle_error = false;

		for dest_path in &dest_paths {
			match crate::gui::sync_items_and_prune(&src_paths, dest_path, use_recycle) {
				Ok(stats) => {
					combined.copied += stats.copied;
					combined.skipped += stats.skipped;
					combined.deleted += stats.deleted;
				}
				Err(e) => {
					if is_recycle_bin_error(&e.to_string()) {
						saw_recycle_error = true;
					}
					fatal_errors.push(e.to_string());
				}
			}
		}

		if saw_recycle_error {
			let error_msg = if !fatal_errors.is_empty() {
				format!("RECYCLE_BIN_ERROR + other errors: {}", fatal_errors.join("; "))
			} else {
				"RECYCLE_BIN_ERROR: Windows refused to move at least one item to the Recycle Bin.".to_string()
			};
			let _ = tx_clone.send(Msg::SyncDone(Err(anyhow::anyhow!(error_msg))));
		} else if !fatal_errors.is_empty() {
			let _ = tx_clone.send(Msg::SyncDone(Err(anyhow::anyhow!(fatal_errors.join("; ")))));
		} else {
			let _ = tx_clone.send(Msg::SyncDone(Ok(combined)));
		}

		app::awake();
	});
}