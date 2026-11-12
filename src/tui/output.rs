use super::widgets::{self, copied, text};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers as M};
use ratatui::prelude::*;
use ratatui_textarea::{CursorMove, TextArea};

/// Text selection and scrolling without exposing editing commands to responses.
pub struct Output {
    panes: [TextArea<'static>; 2],
}
impl Default for Output {
    fn default() -> Self {
        let mut output = Self {
            panes: [text(""), text("")],
        };
        output.set("", "");
        output
    }
}
impl Output {
    pub fn set(&mut self, body: &str, headers: &str) {
        self.panes = [text(body), text(headers)];
        for pane in &mut self.panes {
            pane.set_placeholder_text("Select a skill or conversation.");
        }
    }
    pub fn copy(&self, tab: usize) -> String {
        copied(&self.panes[tab])
    }
    pub fn key(&mut self, tab: usize, key: KeyEvent) {
        let pane = &mut self.panes[tab];
        let shift = key.modifiers.contains(M::SHIFT);
        if key.modifiers.contains(M::CONTROL) && key.code == KeyCode::Char('a') {
            pane.select_all();
            return;
        }
        if key.modifiers.intersects(M::CONTROL | M::ALT) && !shift {
            let movement = match key.code {
                KeyCode::Up => Some((-10, 0)),
                KeyCode::Down => Some((10, 0)),
                KeyCode::Left => Some((0, -10)),
                KeyCode::Right => Some((0, 10)),
                _ => None,
            };
            if let Some((rows, cols)) = movement {
                pane.scroll((rows, cols));
                return;
            }
        }
        match key.code {
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown => {
                pane.input(key);
            }
            KeyCode::Home | KeyCode::End => {
                if shift {
                    if !pane.is_selecting() {
                        pane.start_selection();
                    }
                } else {
                    pane.cancel_selection();
                }
                pane.move_cursor(if key.code == KeyCode::Home {
                    CursorMove::Top
                } else {
                    CursorMove::Bottom
                });
                if key.code == KeyCode::End {
                    pane.move_cursor(CursorMove::End);
                }
            }
            _ => {}
        }
    }
    pub fn scroll(&mut self, tab: usize, rows: i32, cols: i32) {
        self.panes[tab].scroll((
            rows.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            cols.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        ));
    }
    pub fn click(&mut self, tab: usize, rect: Rect, point: Position, start: bool) -> bool {
        if !rect.inner(Margin::new(1, 1)).contains(point) {
            return false;
        }
        let pane = &mut self.panes[tab];
        if start {
            pane.cancel_selection();
        }
        widgets::click(pane, rect, point);
        if start {
            pane.start_selection();
        }
        true
    }
    pub fn draw(&mut self, frame: &mut Frame, tab: usize, rect: Rect, title: &str, active: bool) {
        widgets::field(frame, &mut self.panes[tab], title, rect, active);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;
    #[test]
    fn keyboard_selection_copies_unicode_and_output_cannot_be_edited() {
        let mut output = Output::default();
        output.set("Hello 世界\nsecond line", "x-test: value");
        output.key(0, KeyEvent::new(KeyCode::Right, M::SHIFT));
        output.key(0, KeyEvent::new(KeyCode::Right, M::SHIFT));
        assert_eq!(output.copy(0), "He");
        output.key(0, KeyEvent::new(KeyCode::Char('a'), M::CONTROL));
        assert_eq!(output.copy(0), "Hello 世界\nsecond line");
        for code in [
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Enter,
            KeyCode::Char('x'),
            KeyCode::Char('v'),
            KeyCode::Char('z'),
        ] {
            output.key(0, KeyEvent::new(code, M::CONTROL));
            output.key(0, KeyEvent::new(code, M::NONE));
        }
        assert_eq!(output.copy(0), "Hello 世界\nsecond line");
        assert_eq!(output.copy(1), "x-test: value");
    }
    #[test]
    fn mouse_selection_maps_wide_characters_and_reverse_multiline_drags() {
        let mut output = Output::default();
        output.set("hello 世界\nsecond line", "x-test: value");
        let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
        let rect = Rect::new(0, 0, 50, 10);
        terminal
            .draw(|frame| output.draw(frame, 0, rect, "Response", true))
            .unwrap();
        output.click(0, rect, Position::new(7, 1), true);
        output.click(0, rect, Position::new(11, 1), false);
        assert_eq!(output.copy(0), "世界");
        output.click(0, rect, Position::new(7, 2), true);
        output.click(0, rect, Position::new(7, 1), false);
        assert_eq!(output.copy(0), "世界\nsecond");
        output.key(1, KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        output.key(1, KeyEvent::new(KeyCode::End, KeyModifiers::SHIFT));
        assert_eq!(output.copy(1), "x-test: value");
    }
    #[test]
    fn selection_tracks_scrolled_viewport_and_new_response_clears_it() {
        let mut output = Output::default();
        output.set(
            &(0..40)
                .map(|i| format!("row {i:02} value"))
                .collect::<Vec<_>>()
                .join("\n"),
            "header: old",
        );
        let mut terminal = Terminal::new(TestBackend::new(30, 8)).unwrap();
        let rect = Rect::new(0, 0, 30, 8);
        terminal
            .draw(|f| output.draw(f, 0, rect, "Response", true))
            .unwrap();
        output.scroll(0, 10, 0);
        terminal
            .draw(|f| output.draw(f, 0, rect, "Response", true))
            .unwrap();
        let offset = output.panes[0].scroll_offset().0;
        assert!(offset > 0);
        output.click(0, rect, Position::new(1, 1), true);
        output.click(0, rect, Position::new(7, 1), false);
        assert_eq!(output.copy(0), format!("row {offset:02}"));
        output.set("fresh response", "header: new");
        assert_eq!(output.copy(0), "fresh response");
        assert_eq!(output.copy(1), "header: new");
    }
}
