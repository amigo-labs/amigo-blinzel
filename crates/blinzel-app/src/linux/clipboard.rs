//! The system clipboard (X11 or Wayland, through arboard). A copy is
//! decoded on its own thread right away
//! ([`blinzel_ui::app::copy_in_background`]); its pixels go on the
//! clipboard as an image, which arboard offers as `image/png`.

/// Straight-alpha BGRA (the decoders' order) to the RGBA arboard takes.
pub fn rgba(bgra: &[u8]) -> Vec<u8> {
    bgra.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect()
}

/// Puts `rgba` (`w` × `h`) on the clipboard, connecting to it on first
/// use. `clipboard` must live on: it serves the image to other programs.
pub fn put_image(
    clipboard: &mut Option<arboard::Clipboard>,
    rgba: Vec<u8>,
    w: u32,
    h: u32,
) -> Result<(), String> {
    let cb = match clipboard {
        Some(c) => c,
        None => clipboard.insert(arboard::Clipboard::new().map_err(|e| e.to_string())?),
    };
    cb.set_image(arboard::ImageData {
        width: w as usize,
        height: h as usize,
        bytes: rgba.into(),
    })
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, WindowClass};

    /// `docs/specs/selection-and-export.md` AC-10: on the X server in
    /// `BLINZEL_X11_TEST` (an Xvfb display) another client asks for the
    /// clipboard as `image/png` and gets the copied pixels.
    #[test]
    fn a_copy_is_offered_to_other_programs_as_png() {
        let Some(display) = std::env::var_os("BLINZEL_X11_TEST") else {
            return;
        };
        let display = display.to_string_lossy().into_owned();
        // SAFETY: the X11 tests run one at a time (`--test-threads=1` in
        // CI); without `BLINZEL_X11_TEST` none of them reads or changes the
        // environment.
        unsafe {
            std::env::set_var("DISPLAY", &display);
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        let (w, h) = (5u32, 3u32);
        let bgra: Vec<u8> = (0..w * h * 4).map(|i| (i * 17) as u8).collect();
        let mut clipboard = None;
        put_image(&mut clipboard, rgba(&bgra), w, h).unwrap();

        let (conn, screen) = x11rb::connect(Some(&display)).expect("X server");
        let root = conn.setup().roots[screen].root;
        let win = conn.generate_id().unwrap();
        conn.create_window(
            0,
            win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new(),
        )
        .unwrap();
        let atom = |name: &[u8]| conn.intern_atom(false, name).unwrap().reply().unwrap().atom;
        let (selection, png, prop) = (atom(b"CLIPBOARD"), atom(b"image/png"), atom(b"BLINZEL"));
        conn.convert_selection(win, selection, png, prop, x11rb::CURRENT_TIME)
            .unwrap();
        conn.flush().unwrap();
        let notify = loop {
            if let Event::SelectionNotify(e) = conn.wait_for_event().unwrap() {
                break e;
            }
        };
        assert_eq!(notify.property, prop, "image/png is offered");
        let reply = conn
            .get_property(true, win, prop, AtomEnum::ANY, 0, u32::MAX)
            .unwrap()
            .reply()
            .unwrap();
        assert_eq!(reply.type_, png);
        let mut reader = png::Decoder::new(std::io::Cursor::new(reply.value))
            .read_info()
            .expect("a PNG");
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (w, h));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(&buf[..info.buffer_size()], &rgba(&bgra)[..]);
        drop(clipboard);
    }
}
