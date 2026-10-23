//! The generated application icon (build_icon.rs, embedded by build.rs).

#[path = "../build_icon.rs"]
mod icon;

fn pixel(px: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

#[test]
fn draws_the_favicon() {
    let size = 32;
    let px = icon::rgba(size);
    // Rounded corner: transparent.
    assert_eq!(pixel(&px, size, 0, 0)[3], 0);
    // Square left of the eye, and the pupil: opaque blue.
    assert_eq!(pixel(&px, size, 2, 16), [0, 120, 212, 255]);
    assert_eq!(pixel(&px, size, 16, 16), [0, 120, 212, 255]);
    // Between upper lid and pupil: white.
    assert_eq!(pixel(&px, size, 16, 9), [255, 255, 255, 255]);
    // Symmetric about both axes.
    for y in 0..size {
        for x in 0..size {
            let p = pixel(&px, size, x, y);
            assert_eq!(p, pixel(&px, size, size - 1 - x, y), "({x}, {y})");
            assert_eq!(p, pixel(&px, size, x, size - 1 - y), "({x}, {y})");
        }
    }
}

/// Straight RGBA from an icon DIB (bottom-up BGRA after a 40-byte header).
fn from_dib(data: &[u8], size: u32) -> Vec<u8> {
    let n = size as usize;
    let mut out = vec![0; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let s = &data[40 + ((n - 1 - y) * n + x) * 4..][..4];
            out[(y * n + x) * 4..][..4].copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
    }
    out
}

#[test]
fn packs_dib_and_png_entries() {
    let sizes = [16, 32, 256];
    let ico = icon::ico(&sizes);
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(ico[i..i + 4].try_into().unwrap());
    assert_eq!((u16_at(0), u16_at(2), u16_at(4)), (0, 1, 3));
    for (k, &size) in sizes.iter().enumerate() {
        let entry = 6 + 16 * k;
        let dim = if size == 256 { 0 } else { size as u8 };
        assert_eq!(&ico[entry..entry + 2], &[dim, dim]);
        let (len, offset) = (u32_at(entry + 8) as usize, u32_at(entry + 12) as usize);
        let data = &ico[offset..offset + len];
        if size < 256 {
            assert_eq!(u32::from_le_bytes(data[..4].try_into().unwrap()), 40);
            assert_eq!(from_dib(data, size), icon::rgba(size));
            continue;
        }
        let decoder = png::Decoder::new(std::io::Cursor::new(data));
        let mut reader = decoder.read_info().expect("valid png");
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (size, size));
        assert_eq!(&buf[..info.buffer_size()], &icon::rgba(size)[..]);
    }
}
