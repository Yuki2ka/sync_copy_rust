use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use anyhow::Result;
use crate::layout::{BTN_GAP, ContentWidth, GAP, MARGIN, ROW_H, SEPARATOR_H, BG_DARK, BG_PANEL, BG_ROW_ALT, SEPARATOR_COLOR, TEXT_LIGHT, SRC_BTN_COLOR, DST_BTN_COLOR, SKIP_BTN_COLOR};
use crate::state::{State, Role, Msg};
use crate::sync::SyncStats;
use fltk::{
	app, button,
	enums::{Align, Color, FrameType},
	frame, group,
	prelude::*,
	tree, window,
};

pub fn ask_permanent_delete() -> bool {
	rfd::MessageDialog::new()
		.set_title("Unable to move files to the Recycle Bin")
		.set_description(
			"Windows could not move one or more files to the Recycle Bin.\n\n\
			This can happen if:\n\
			• the Recycle Bin is disabled for the drive\n\
			• the drive has insufficient free space\n\
			• the file is on a network or removable drive\n\
			• Windows returned \"Incorrect function (OS error 1)\" or another I/O error\n\n\
			Disable \"Use Recycle Bin\" and delete permanently instead?",
		)
		.set_buttons(rfd::MessageButtons::YesNo)
		.show()
		== rfd::MessageDialogResult::Yes
}

pub fn confirm_sync() -> bool {
	rfd::MessageDialog::new()
		.set_title("Confirm Sync")
		.set_description(
			"Content of folder will be REPLACED (mirrored).\n\n\
			New/changed files will be copied.\n\
			Files in destination not in source will be DELETED.\n\n\
			Proceed with sync?",
		)
		.set_buttons(rfd::MessageButtons::YesNo)
		.show()
		== rfd::MessageDialogResult::Yes
}

pub fn confirm_close_during_sync() -> bool {
	rfd::MessageDialog::new()
		.set_title("Sync in Progress")
		.set_description(
			"A sync operation is currently running.\n\n\
			Closing the window now may interrupt the sync and leave your\n\
			destination in an incomplete state.\n\n\
			Are you sure you want to close?",
		)
		.set_buttons(rfd::MessageButtons::YesNo)
		.show()
		== rfd::MessageDialogResult::Yes
}

pub fn is_recycle_bin_error(msg: &str) -> bool {
	msg.to_ascii_lowercase().contains("recycle_bin_error")
}

/// One row in the capture list. Widgets are created once and only their
/// label / color / visibility is mutated on updates.
#[derive(Clone)]
pub struct CaptureRow {
	pub container: group::Group,
	pub id_lbl: frame::Frame,
	pub summary_lbl: frame::Frame,
	pub src_btn: button::Button,
	pub dst_btn: button::Button,
	pub skip_btn: button::Button,
	pub prev_btn: button::Button,
}

pub struct Ui {
	#[allow(dead_code)]
	pub win: window::Window,
	#[allow(dead_code)]
	pub status_lbl: frame::Frame,
	pub list_area: group::Scroll,
	pub list_pack: group::Pack,
	#[allow(dead_code)]
	pub empty_lbl: frame::Frame,
	pub rows: Vec<CaptureRow>,
	pub capture_count_lbl: frame::Frame,
	#[allow(dead_code)]
	pub preview_lbl: frame::Frame,
	pub preview_tree: tree::Tree,
	pub skip_chk: button::CheckButton,
	pub clear_btn: button::Button,
	pub recycle_chk: button::CheckButton,
	pub sync_btn: button::Button,
	pub sync_close_btn: button::Button,
	pub progress_lbl: frame::Frame,
	pub stats_lbl: frame::Frame,
}

// Fixed widths for action buttons — never change between states.
const BTN_SRC_W: i32 = 80;
const BTN_DST_W: i32 = 100;
const BTN_SKIP_W: i32 = 70;
const BTN_PREV_W: i32 = 80;
const ID_W: i32 = 40;
const ROW_PAD: i32 = 4;
const SCROLLBAR_W: i32 = 16;

pub fn build_window(_tx: &app::Sender<Msg>) -> Ui {
	let win_w = 860;
	let win_h = 600;
	let content_w = ContentWidth::from(win_w);

	let mut win = window::Window::default().with_size(win_w, win_h).with_label("sync_copy");
	win.set_color(BG_DARK);

	// Status bar
	let status_y = MARGIN;
	let mut status_lbl = frame::Frame::new(MARGIN, status_y, content_w.0, 28, None);
	status_lbl.set_label("Copy files/folders, open the destination folder in Explorer, then press Ctrl+B.");
	status_lbl.set_align(Align::Left | Align::Inside | Align::Wrap);
	status_lbl.set_label_color(TEXT_LIGHT);

	let sep_y = status_y + 28 + GAP;
	let mut sep1 = frame::Frame::new(MARGIN, sep_y, content_w.0, SEPARATOR_H, None);
	sep1.set_frame(FrameType::FlatBox);
	sep1.set_color(SEPARATOR_COLOR);

	// Capture count
	let count_y = sep_y + SEPARATOR_H + GAP;
	let mut capture_count_lbl = frame::Frame::new(MARGIN, count_y, content_w.0, 20, None);
	capture_count_lbl.set_label("0 captures");
	capture_count_lbl.set_align(Align::Left | Align::Inside);
	capture_count_lbl.set_label_color(TEXT_LIGHT);

	// Scroll + Pack for rows
	let list_y = count_y + 20 + 4;
	let list_h = 220;
	let mut list_area = group::Scroll::new(MARGIN, list_y, content_w.0, list_h, None);
	list_area.set_color(BG_PANEL);
	list_area.set_frame(FrameType::ThinDownBox);
	// Only show scrollbar when needed.
	list_area.set_type(group::ScrollType::Vertical);
	list_area.set_scrollbar_size(SCROLLBAR_W);

	// Pack holds rows vertically; its width is fixed so rebuilds don't
	// recompute x positions based on whether the scrollbar is visible.
	let pack_w = content_w.0 - SCROLLBAR_W - 2;
	let mut list_pack = group::Pack::new(
		list_area.x() + 1,
		list_area.y() + 1,
		pack_w,
		1, // height grows automatically
		None,
	);
	list_pack.set_spacing(2);
	list_pack.end();

	// Empty label - just a placeholder for UI struct (not used)
	let empty_lbl = frame::Frame::new(0, 0, 1, 1, None);

	list_area.end();

	let sep_y2 = list_y + list_h + GAP;
	let mut sep2 = frame::Frame::new(MARGIN, sep_y2, content_w.0, SEPARATOR_H, None);
	sep2.set_frame(FrameType::FlatBox);
	sep2.set_color(SEPARATOR_COLOR);

	// Preview
	let preview_lbl_y = sep_y2 + SEPARATOR_H + GAP;
	let mut preview_lbl = frame::Frame::new(MARGIN, preview_lbl_y, content_w.0, 18, None);
	preview_lbl.set_label("Preview");
	preview_lbl.set_align(Align::Left | Align::Inside);
	preview_lbl.set_label_color(TEXT_LIGHT);

	let tree_y = preview_lbl_y + 22;
	let tree_h = 140;
	let mut preview_tree = tree::Tree::new(MARGIN, tree_y, content_w.0, tree_h, None);
	preview_tree.set_root_label("");

	let sep_y3 = tree_y + tree_h + GAP;
	let mut sep3 = frame::Frame::new(MARGIN, sep_y3, content_w.0, SEPARATOR_H, None);
	sep3.set_frame(FrameType::FlatBox);
	sep3.set_color(SEPARATOR_COLOR);

	// Toolbar
	let toolbar_y = sep_y3 + SEPARATOR_H + GAP;
	let toolbar_h = 30;

	let mut skip_chk = button::CheckButton::new(MARGIN, toolbar_y, 140, toolbar_h, None);
	skip_chk.set_label("Skip Unchanged");
	skip_chk.set_value(true);
	skip_chk.set_label_color(TEXT_LIGHT);

	let mut recycle_chk = button::CheckButton::new(MARGIN + 150, toolbar_y, 150, toolbar_h, None);
	recycle_chk.set_label("Use Recycle Bin");
	recycle_chk.set_value(true);
	recycle_chk.set_label_color(TEXT_LIGHT);

	let sync_w = 140;
	let sync_close_w = 210;
	let clear_w = 110;
	let clear_x = MARGIN + 140 + BTN_GAP + 150 + BTN_GAP;
	let sync_x = clear_x + clear_w + BTN_GAP;
	let sync_close_x = sync_x + sync_w + BTN_GAP;

	let mut clear_btn = button::Button::new(clear_x, toolbar_y, clear_w, toolbar_h, None);
	clear_btn.set_label("Clear List");

	let mut sync_btn = button::Button::new(sync_x, toolbar_y, sync_w, toolbar_h, None);
	sync_btn.set_label("Sync");
	sync_btn.set_color(SRC_BTN_COLOR);
	sync_btn.set_label_color(TEXT_LIGHT);

	let mut sync_close_btn = button::Button::new(sync_close_x, toolbar_y, sync_close_w, toolbar_h, None);
	sync_close_btn.set_label("Sync, if success close \u{274C}");
	sync_close_btn.set_color(SRC_BTN_COLOR);
	sync_close_btn.set_label_color(TEXT_LIGHT);

	let sep_y4 = toolbar_y + toolbar_h + GAP;
	let mut sep4 = frame::Frame::new(MARGIN, sep_y4, content_w.0, SEPARATOR_H, None);
	sep4.set_frame(FrameType::FlatBox);
	sep4.set_color(SEPARATOR_COLOR);

	let progress_y = sep_y4 + SEPARATOR_H + GAP;
	let mut progress_lbl = frame::Frame::new(MARGIN, progress_y, content_w.0, 24, None);
	progress_lbl.set_label("");
	progress_lbl.set_align(Align::Left | Align::Inside | Align::Wrap);
	progress_lbl.set_label_color(TEXT_LIGHT);

	let stats_y = progress_y + 24;
	let mut stats_lbl = frame::Frame::new(MARGIN, stats_y, content_w.0, 24, None);
	stats_lbl.set_label("");
	stats_lbl.set_align(Align::Left | Align::Inside | Align::Wrap);
	stats_lbl.set_label_color(TEXT_LIGHT);

	win.end();

	Ui {
		win,
		status_lbl,
		list_area,
		list_pack,
		empty_lbl,
		rows: Vec::new(),
		capture_count_lbl,
		preview_lbl,
		preview_tree,
		skip_chk,
		clear_btn,
		recycle_chk,
		sync_btn,
		sync_close_btn,
		progress_lbl,
		stats_lbl,
	}
}
/// Build one row (called only when the *number* of captures grows).
fn build_row(pack_w: i32, tx: &app::Sender<Msg>, row_index: usize, bg_color: Color) -> CaptureRow {
	let mut container = group::Group::new(0, 0, pack_w, ROW_H, None);
	container.set_frame(FrameType::FlatBox);
	container.set_color(bg_color);

	container.begin();

	let mut id_lbl = frame::Frame::new(ROW_PAD, ROW_PAD, ID_W, ROW_H - 2 * ROW_PAD, None);
	id_lbl.set_align(Align::Left | Align::Inside);
	id_lbl.set_label_color(TEXT_LIGHT);

	let buttons_total = BTN_SRC_W + BTN_DST_W + BTN_SKIP_W + BTN_PREV_W + BTN_GAP * 3;
	let buttons_x = pack_w - ROW_PAD - buttons_total;

	let summary_x = ROW_PAD + ID_W + 4;
	let summary_w = buttons_x - summary_x - 8;

	let mut summary_lbl = frame::Frame::new(summary_x, ROW_PAD, summary_w.max(40), ROW_H - 2 * ROW_PAD, None);
	summary_lbl.set_align(Align::Left | Align::Inside | Align::Clip);
	summary_lbl.set_label_color(TEXT_LIGHT);

	let btn_y = (ROW_H - (ROW_H - 12)) / 2;
	let btn_h = ROW_H - 12;
	let mut bx = buttons_x;

	let tx_src = tx.clone();
	let ri = row_index;
	let mut src_btn = button::Button::new(bx, btn_y, BTN_SRC_W, btn_h, "Source");
	src_btn.set_label_color(TEXT_LIGHT);
	src_btn.set_callback(move |_| { let _ = tx_src.send(Msg::SetRoleRowIndex(ri, Role::SOURCE)); });
	bx += BTN_SRC_W + BTN_GAP;

	let tx_dst = tx.clone();
	let ri = row_index;
	let mut dst_btn = button::Button::new(bx, btn_y, BTN_DST_W, btn_h, "Destination");
	dst_btn.set_label_color(TEXT_LIGHT);
	dst_btn.set_callback(move |_| { let _ = tx_dst.send(Msg::SetRoleRowIndex(ri, Role::DESTINATION)); });
	bx += BTN_DST_W + BTN_GAP;

	let tx_skip = tx.clone();
	let ri = row_index;
	let mut skip_btn = button::Button::new(bx, btn_y, BTN_SKIP_W, btn_h, "Skip");
	skip_btn.set_label_color(TEXT_LIGHT);
	skip_btn.set_callback(move |_| { let _ = tx_skip.send(Msg::SetRoleRowIndex(ri, Role::SKIP)); });
	bx += BTN_SKIP_W + BTN_GAP;

	let tx_prev = tx.clone();
	let ri = row_index;
	let mut prev_btn = button::Button::new(bx, btn_y, BTN_PREV_W, btn_h, "Preview");
	prev_btn.set_label_color(TEXT_LIGHT);
	prev_btn.set_callback(move |_| { let _ = tx_prev.send(Msg::PreviewCaptureRowIndex(ri)); });

	container.end();

	CaptureRow {
		container,
		id_lbl,
		summary_lbl,
		src_btn,
		dst_btn,
		skip_btn,
		prev_btn,
	}
}

/// Update labels/colors of existing rows; add or remove rows only if the
/// count actually changed. Uses row index-based callbacks for stability.
pub fn refresh_capture_list(
	ui: &mut Ui,
	tx: &app::Sender<Msg>,
	state_arc: &Arc<Mutex<State>>,
) {
	let s = state_arc.lock().unwrap();
	let captures = s.captures.clone();
	let active_preview_id = s.active_preview_id;
	drop(s);

	let pack_w = ui.list_pack.w();

	while ui.rows.len() < captures.len() {
		ui.list_pack.begin();
		let row = build_row(pack_w, tx, ui.rows.len(), BG_PANEL);
		ui.list_pack.end();
		ui.rows.push(row);
	}
	while ui.rows.len() > captures.len() {
		if let Some(mut row) = ui.rows.pop() {
			ui.list_pack.remove(&row.container);
			row.container.hide();
			drop(row);
		}
	}

	for (idx, cap) in captures.iter().enumerate() {
		let row = &mut ui.rows[idx];

		let bg_color = if idx % 2 == 0 { BG_PANEL } else { BG_ROW_ALT };
		row.container.set_color(bg_color);

		row.id_lbl.set_label(&format!("#{}", cap.id));

		let summary = cap.summary();
		row.summary_lbl.set_label(&summary);
		row.summary_lbl.set_tooltip(&summary);

		row.src_btn.set_label("Source");
		row.dst_btn.set_label("Destination");
		row.skip_btn.set_label("Skip");

		row.src_btn.set_color(if cap.role == Role::SOURCE { SRC_BTN_COLOR } else { BG_PANEL });
		row.dst_btn.set_color(if cap.role == Role::DESTINATION { DST_BTN_COLOR } else { BG_PANEL });
		row.skip_btn.set_color(if cap.role == Role::SKIP { SKIP_BTN_COLOR } else { BG_PANEL });

		row.prev_btn.set_label(if active_preview_id == Some(cap.id) { "Hide" } else { "Preview" });

		row.src_btn.redraw();
		row.dst_btn.redraw();
		row.skip_btn.redraw();
		row.prev_btn.redraw();
		row.container.redraw();
	}

	ui.capture_count_lbl.set_label(&format!("{} captures", captures.len()));

	ui.list_pack.redraw();
	ui.list_area.redraw();
}

pub fn show_preview(tree: &mut tree::Tree, plan: &[crate::sync::PlanItem]) {
	tree.clear();

	let (counts, action_map, deleted_map) = crate::sync::analyze_plan(plan);

	let mut parts = Vec::new();
	parts.push(format!("New: {}", counts.get("new").unwrap_or(&0)));
	parts.push(format!("Updated: {}", counts.get("updated").unwrap_or(&0)));
	parts.push(format!("Deleted: {}", counts.get("deleted").unwrap_or(&0)));
	if counts.get("unchanged").copied().unwrap_or(0) > 0 {
		parts.push(format!("Unchanged: {}", counts.get("unchanged").unwrap_or(&0)));
	}
	tree.set_root_label(&parts.join(" | "));

	for item in plan {
		let name = item.rel.clone();
		let (parent_path, leaf_name) = if let Some((p, l)) = name.rsplit_once('/') {
			(p, l)
		} else {
			("", name.as_str())
		};

		let label = format!("{}\t{}\t{}", leaf_name, crate::sync::action_label_public(item.action), if item.is_dir { "Dir" } else { "File" });

		let color = match item.action {
			crate::sync::Action::NEW => Color::Green,
			crate::sync::Action::UPDATED => Color::Yellow,
			crate::sync::Action::DELETED => Color::Red,
			crate::sync::Action::UNCHANGED => Color::from_rgb(200, 200, 200),
		};

		if let Some(mut tree_item) = if parent_path.is_empty() {
			tree.add(&label)
		} else {
			tree.add(&format!("{}/{}", parent_path, label))
		} {
			tree_item.set_label_fgcolor(color);
		}
	}

	if let Some(deleted_children) = deleted_map.get("") {
		for name in deleted_children {
			if let Some(item) = action_map.get(name) {
				let label = format!("Deleted/{}\t{}\t{}", name, crate::sync::action_label_public(item.action), if item.is_dir { "Dir" } else { "File" });
				if let Some(mut tree_item) = tree.add(&label) {
					tree_item.set_label_fgcolor(Color::Red);
				}
			}
		}
	}

	tree.redraw();
}

pub fn sync_items_and_prune(src_paths: &[PathBuf], dest_path: &Path, use_recycle: bool) -> Result<SyncStats> {
	use crate::sync::{sync_file, SyncOptions};
	use std::collections::HashSet;

	if src_paths.is_empty() {
		return Ok(SyncStats::default());
	}

	let src_expanded = if src_paths.len() == 1 && src_paths[0].is_dir() {
		let mut items = Vec::new();
		for entry in std::fs::read_dir(&src_paths[0])? {
			items.push(entry?.path());
		}
		items
	} else {
		src_paths.to_vec()
	};

	let opts = SyncOptions {
		use_recycle_bin: use_recycle,
		..Default::default()
	};

	let mut combined = SyncStats::default();
	let mut errors: Vec<anyhow::Error> = Vec::new();

	for src_item in &src_expanded {
		if let Some(name) = src_item.file_name() {
			let target = dest_path.join(name);
			match sync_file(src_item, &target, &opts) {
				Ok(stats) => {
					combined.copied += stats.copied;
					combined.skipped += stats.skipped;
					combined.deleted += stats.deleted;
				}
				Err(e) => errors.push(e),
			}
		}
	}

	let mut keep_names = HashSet::new();
	for src_item in &src_expanded {
		if let Some(name) = src_item.file_name() {
			keep_names.insert(name.to_os_string());
		}
	}
	if !keep_names.is_empty() {
		let pruned = crate::sync::prune_top_level(dest_path, &keep_names, &opts)?;
		combined.deleted += pruned;
	}

	if !errors.is_empty() {
		Err(anyhow::anyhow!(errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ")))
	} else {
		Ok(combined)
	}
}

pub fn configure_callbacks(ui: &mut Ui, tx: &app::Sender<Msg>, state: &Arc<Mutex<State>>) {
	let clear_tx = tx.clone();
	ui.clear_btn.set_callback(move |_| {
		let _ = clear_tx.send(Msg::ClearCaptures);
	});

	let sync_tx = tx.clone();
	ui.sync_btn.set_callback(move |_| {
		let _ = sync_tx.send(Msg::SyncNow);
	});

	let sync_close_tx = tx.clone();
	ui.sync_close_btn.set_callback(move |_| {
		let _ = sync_close_tx.send(Msg::SyncNowClose);
	});

	let state_for_close = state.clone();
	let tx_for_close = tx.clone();
	ui.win.handle(move |_w, ev| {
		if ev == fltk::enums::Event::Close {
			let s = state_for_close.lock().unwrap();
			if s.syncing {
				if !confirm_close_during_sync() {
					return true;
				}
			}
			let _ = tx_for_close.send(Msg::WindowClose);
			true
		} else {
			false
		}
	});
}