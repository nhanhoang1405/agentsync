use crossterm::event::{KeyCode, KeyEvent, KeyModifiers as M};
use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::{CursorMove, TextArea};
pub const ACCENT: Color = Color::Rgb(96, 215, 174);
pub const BG: Color = Color::Rgb(18, 23, 32);
pub const MUTED: Color = Color::Rgb(140, 155, 175);
pub fn panel(title: impl Into<String>, active: bool) -> Block<'static> {
    Block::bordered()
        .title(title.into())
        .border_style(Style::default().fg(if active {
            ACCENT
        } else {
            Color::Rgb(55, 69, 85)
        }))
}
pub fn text(value: &str) -> TextArea<'static> {
    let mut field = TextArea::from(value.split('\n').map(str::to_owned).collect::<Vec<_>>());
    field.set_style(Style::default().fg(Color::White).bg(BG));
    field.set_cursor_line_style(Style::default());
    field.set_selection_style(Style::default().bg(Color::Rgb(40, 87, 78)));
    field
}
pub fn value(field: &TextArea<'_>) -> String {
    field.lines().join("\n")
}
pub fn selected() -> Style {
    Style::default().fg(BG).bg(ACCENT).bold()
}
pub fn button(frame: &mut Frame, label: &str, rect: Rect, active: bool) {
    frame.render_widget(
        Paragraph::new(label)
            .alignment(Alignment::Center)
            .block(panel("", active))
            .style(Style::default().bg(BG)),
        rect,
    );
}
pub fn field(frame: &mut Frame, editor: &mut TextArea<'_>, title: &str, rect: Rect, active: bool) {
    editor.set_block(panel(title, active));
    editor.set_cursor_style(if active {
        Style::default().reversed()
    } else {
        Style::default()
    });
    frame.render_widget(&*editor, rect);
}
pub fn edit(editor: &mut TextArea<'_>, key: KeyEvent) -> bool {
    if key.modifiers.contains(M::CONTROL) {
        match key.code {
            KeyCode::Char('a') => {
                editor.select_all();
                return false;
            }
            KeyCode::Char('z') => return editor.undo(),
            KeyCode::Char('y') => return editor.redo(),
            _ => {}
        }
    }
    editor.input(key)
}
pub fn copied(editor: &TextArea<'_>) -> String {
    let Some((start, end)) = editor.selection_range().filter(|(s, e)| s != e) else {
        return value(editor);
    };
    (start.0..=end.0)
        .map(|row| {
            let from = if row == start.0 { start.1 } else { 0 };
            let count = if row == end.0 {
                end.1.saturating_sub(from)
            } else {
                usize::MAX
            };
            editor.lines()[row]
                .chars()
                .skip(from)
                .take(count)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn click(editor: &mut TextArea<'_>, rect: Rect, point: Position) {
    let inner = rect.inner(Margin::new(1, 1));
    if !inner.contains(point) {
        return;
    }
    let (top, left) = editor.scroll_offset();
    let cursor = editor.screen_to_data(
        (top + point.y - inner.y) as usize,
        (left + point.x - inner.x) as usize,
    );
    editor.move_cursor(CursorMove::Jump(
        cursor.0.min(u16::MAX as usize) as u16,
        cursor.1.min(u16::MAX as usize) as u16,
    ));
}
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}
pub fn background(frame: &mut Frame) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(Color::White)),
        frame.area(),
    );
}
