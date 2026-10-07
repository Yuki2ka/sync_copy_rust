use fltk::enums::Color;

pub const MARGIN: i32 = 16;
pub const GAP: i32 = 8;
pub const ROW_H: i32 = 40;
pub const BTN_GAP: i32 = 6;
pub const SEPARATOR_H: i32 = 1;

pub const BG_DARK: Color = Color::from_rgb(60, 60, 65);
pub const BG_PANEL: Color = Color::from_rgb(140, 145, 165);
pub const BG_ROW_ALT: Color = Color::from_rgb(160, 165, 185);
pub const SEPARATOR_COLOR: Color = Color::from_rgb(70, 70, 90);
pub const TEXT_LIGHT: Color = Color::from_rgb(245, 245, 255);
pub const SRC_BTN_COLOR: Color = Color::from_rgb(60, 130, 80);
pub const DST_BTN_COLOR: Color = Color::from_rgb(70, 90, 150);
pub const SKIP_BTN_COLOR: Color = Color::from_rgb(140, 120, 60);

#[derive(Clone, Copy)]
pub struct ContentWidth(pub i32);

impl From<i32> for ContentWidth {
	fn from(win_w: i32) -> Self {
		Self(win_w - MARGIN * 2)
	}
}