//! A question with two buttons, drawn over the viewer ("Remove the marks
//! of 12 images?" — `docs/specs/picks.md` AC-04). Modal: every key and
//! click goes to it; Enter takes the first button, Esc the second.

use blinzel_core::geom::Rect;

use crate::backend::{Canvas, Key};
use crate::widgets::{self, Painter};

/// What a key or click did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Nothing yet: the question stays.
    Open,
    /// The first button, or Enter.
    Yes,
    /// The second button, or Esc.
    No,
}

/// The question and its two buttons.
pub struct Confirm {
    /// The question.
    pub text: String,
    /// The first button's label (the one that acts).
    pub yes: String,
    /// The second button's label (the one that cancels).
    pub no: String,
    /// The buttons where they were drawn last.
    rects: Option<(Rect, Rect)>,
}

impl Confirm {
    /// `text` with the buttons `yes` and `no`.
    pub fn new(text: String, yes: String, no: String) -> Confirm {
        Confirm {
            text,
            yes,
            no,
            rects: None,
        }
    }

    /// What `key` answers: Enter yes, Esc no, everything else nothing.
    pub fn key(&self, key: Key) -> Answer {
        match key {
            Key::Enter => Answer::Yes,
            Key::Escape => Answer::No,
            _ => Answer::Open,
        }
    }

    /// What a click at `p` answers (the buttons as drawn last).
    pub fn mouse_down(&self, p: (f64, f64)) -> Answer {
        match self.rects {
            Some((yes, _)) if yes.contains(p) => Answer::Yes,
            Some((_, no)) if no.contains(p) => Answer::No,
            _ => Answer::Open,
        }
    }

    /// Where the first button was drawn (tests click it).
    pub fn yes_rect(&self) -> Option<Rect> {
        self.rects.map(|r| r.0)
    }

    /// Dims the viewer and draws the panel in the middle of `client`.
    pub fn draw(&mut self, canvas: &mut impl Canvas, client: (f64, f64), scale: f64) {
        let mut p = Painter { canvas, scale };
        p.fill(Rect::new(0.0, 0.0, client.0, client.1), 0.0, widgets::SCRIM);
        let (pad, gap) = (p.px(20.0), p.px(12.0));
        let (tw, th) = p.text_size(&self.text, widgets::TEXT_BODY);
        let bh = p.px(widgets::CONTROL_H);
        let (yw, nw) = (p.button_width(&self.yes), p.button_width(&self.no));
        let w = (tw.max(yw + gap + nw) + 2.0 * pad).min(client.0);
        let h = pad + th + gap + bh + pad;
        let r = Rect::new(
            ((client.0 - w) / 2.0).round(),
            ((client.1 - h) / 2.0).round(),
            w,
            h,
        );
        p.fill(r, widgets::RADIUS_PANEL, widgets::PANEL);
        p.edge_outline(r, 1.0, widgets::RADIUS_PANEL, widgets::BORDER);
        p.text(
            &self.text,
            widgets::TEXT_BODY,
            r.x + pad,
            r.y + pad,
            widgets::FG,
        );
        let y = r.y + pad + th + gap;
        let no = Rect::new(r.right() - pad - nw, y, nw, bh);
        let yes = Rect::new(no.x - gap - yw, y, yw, bh);
        p.button(&self.yes, true, false, true, yes);
        p.button(&self.no, false, false, true, no);
        self.rects = Some((yes, no));
    }
}
