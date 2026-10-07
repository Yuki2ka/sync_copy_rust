use anyhow::Result;

#[cfg(windows)]
pub fn start_listener<F>(on_hotkey: F) -> Result<()>
where
	F: Fn() + Send + Sync + 'static,
{
	use std::ptr::null_mut;
	use winapi::um::winuser::{
		GetMessageW, RegisterHotKey, UnregisterHotKey, MSG, MOD_CONTROL, WM_HOTKEY,
	};

	const HOTKEY_ID: i32 = 1;
	const VK_B: u32 = b'B' as u32;
	const MOD_NOREPEAT: u32 = 0x4000;

	unsafe {
		if RegisterHotKey(
			null_mut(),
			HOTKEY_ID,
			MOD_CONTROL as u32 | MOD_NOREPEAT,
			VK_B,
		) == 0
		{
			let err = std::io::Error::last_os_error();
			anyhow::bail!("Could not register Ctrl+B global hotkey: {err}");
		}

		let mut msg: MSG = std::mem::zeroed();

		loop {
			let ret = GetMessageW(&mut msg, null_mut(), 0, 0);

			if ret == -1 {
				let err = std::io::Error::last_os_error();
				let _ = UnregisterHotKey(null_mut(), HOTKEY_ID);
				anyhow::bail!("Hotkey message loop failed: {err}");
			}

			if ret == 0 {
				break;
			}

			if msg.message == WM_HOTKEY && msg.wParam as i32 == HOTKEY_ID {
				on_hotkey();
			}
		}

		let _ = UnregisterHotKey(null_mut(), HOTKEY_ID);
	}

	Ok(())
}

#[cfg(not(windows))]
pub fn start_listener<F>(on_hotkey: F) -> Result<()>
where
	F: Fn() + Send + Sync + 'static,
{
	use rdev::{listen, Event, EventType, Key};

	let mut ctrl = false;
	let mut fired = false;

	if let Err(e) = listen(move |event: Event| match event.event_type {
		EventType::KeyPress(key) => match key {
			Key::ControlLeft | Key::ControlRight => ctrl = true,
			Key::KeyB if ctrl && !fired => {
				fired = true;
				on_hotkey();
			}
			_ => {}
		},
		EventType::KeyRelease(key) => match key {
			Key::ControlLeft | Key::ControlRight => {
				ctrl = false;
				fired = false;
			}
			Key::KeyB => fired = false,
			_ => {}
		},
		_ => {}
	}) {
		eprintln!("rdev listener error: {e:?}");
	}

	Ok(())
}