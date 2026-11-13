use super::*;

impl App {
    pub(super) fn draw(&mut self, frame: &mut Frame) {
        self.geometry = Geometry::default();
        ui::background(frame);
        self.too_small = frame.area().width < 72 || frame.area().height < 26;
        if self.too_small {
            frame.render_widget(
                Paragraph::new(
                    "AgentSync needs 72 x 26 cells.\nResize the terminal. Ctrl+Q quits.",
                ),
                frame.area(),
            );
            return;
        }
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(frame.area());
        let toolbar = Layout::horizontal([
            Constraint::Min(10),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Length(11),
        ])
        .split(rows[0]);
        frame.render_widget(
            Paragraph::new("AgentSync")
                .style(Style::default().fg(ui::ACCENT).bold())
                .block(Block::default().padding(Padding::top(1))),
            toolbar[0],
        );
        for (i, label) in ["F5 Sync", "F6 Push", "F7 Pull", "F1 Help"]
            .iter()
            .enumerate()
        {
            ui::button(frame, label, toolbar[i + 1], false);
            self.geometry.buttons.push(toolbar[i + 1]);
        }
        let tabs = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(rows[1]);
        for (i, label) in ["Alt+1 Local skills", "Alt+2 Remote skills", "Alt+3 History"]
            .iter()
            .enumerate()
        {
            frame.render_widget(
                Paragraph::new(*label).style(if i == self.tab {
                    ui::selected()
                } else {
                    Style::default().fg(ui::MUTED)
                }),
                tabs[i],
            );
            self.geometry.tabs.push(tabs[i]);
        }
        let panels = Layout::horizontal([
            Constraint::Length((frame.area().width / 3).clamp(24, 48)),
            Constraint::Min(1),
        ])
        .split(rows[2]);
        self.geometry.browser = panels[0];
        self.geometry.output = panels[1];
        let labels = self
            .visible()
            .into_iter()
            .map(|row| ListItem::new(row.label))
            .collect::<Vec<_>>();
        let title = if let Some(p) = &self.project {
            format!("Chats: {}", p.name)
        } else if self.filter.is_empty() {
            "Browse".into()
        } else {
            format!("Filter: {}", self.filter)
        };
        frame.render_stateful_widget(
            List::new(labels)
                .block(ui::panel(
                    title,
                    !self.focus_output && self.dialog.is_none(),
                ))
                .highlight_style(ui::selected()),
            panels[0],
            &mut self.selection,
        );
        let reader_title = self
            .selected_row()
            .and_then(|row| match row.node {
                tree::Node::File(index) => {
                    Some(self.items[row.item].files().unwrap()[index].title.clone())
                }
                _ => None,
            })
            .map_or_else(|| "Plain text".into(), |file| format!("File: {file}"));
        self.output.draw(
            frame,
            0,
            panels[1],
            &reader_title,
            self.focus_output && self.dialog.is_none(),
        );
        frame.render_widget(
            Paragraph::new(format!(
                "{}{}\nPush visibility: {} (F8)",
                if self.busy { "Running | " } else { "" },
                self.status,
                self.visibility
            ))
            .style(Style::default().fg(if self.status.starts_with("Error:") {
                Color::LightRed
            } else {
                ui::MUTED
            })),
            rows[3],
        );
        frame.render_widget(
            Paragraph::new(
                "Tab focus | Enter open | / filter | F3 refresh | ^S sync all | ^Q quit",
            )
            .style(Style::default().fg(ui::MUTED)),
            rows[4],
        );
        if self.dialog.is_some() {
            self.draw_dialog(frame);
        }
    }
    fn draw_dialog(&mut self, frame: &mut Frame) {
        let area = ui::centered(frame.area(), 68, 22);
        frame.render_widget(Clear, area);
        frame.render_widget(
            ui::panel("AgentSync", true).style(Style::default().bg(ui::BG)),
            area,
        );
        let inner = area.inner(Margin::new(2, 1));
        let rows = Layout::vertical([
            Constraint::Min(4),
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Length(3),
        ])
        .split(inner);
        let (description, editable, apply) = match &self.dialog {
            Some(Dialog::Connection) => (
                "SQL connection URL\nPostgres, SQLite, MySQL/MariaDB, libSQL/Turso.\nTurso token: TURSO_AUTH_TOKEN or ?authToken=... in URL.
Saved in config.toml (0600 on Unix); server TLS is verified.\nCtrl+V paste | Ctrl+A select | Ctrl+Z/Y undo/redo",
                true,
                "Connect",
            ),
            Some(Dialog::Filter) => (
                "Filter the browser by skill name, scope, or author.\nClear the field to show every item.",
                true,
                "Apply",
            ),
            Some(Dialog::Overwrite(request)) => {
                frame.render_widget(Paragraph::new(format!("Replace differing local files for skill {}?\nSome nonconflicting files may already have been pulled.\nEnter confirms; Esc keeps remaining local files.", request.skill_name.as_deref().unwrap_or("selected"))).wrap(Wrap { trim: false }), rows[0]);
                ("", false, "Overwrite")
            }
            _ => (
                "Alt+1 Local skills | Alt+2 Remote skills | Alt+3 History\nTab / Shift+Tab: browser and plain-text reader\nArrows: navigate / expand / collapse | Enter: open | Esc: back\nF5: sync all, or selected project history\nF6: push selected skill | F7: pull selected skill\nF8: change push visibility (default private)\nCtrl+S: sync all | F3: refresh | /: filter\nReader: mouse drag / Shift+arrows, Ctrl+A, Ctrl+C\nCtrl+Q: quit after running work completes",
                false,
                "Close",
            ),
        };
        if !description.is_empty() {
            frame.render_widget(
                Paragraph::new(description).wrap(Wrap { trim: false }),
                rows[0],
            );
        }
        if editable {
            self.geometry.field = rows[1];
            ui::field(frame, &mut self.input, "", rows[1], self.dialog_focus == 0);
        }
        frame.render_widget(
            Paragraph::new(&*self.status)
                .style(Style::default().fg(if self.status.starts_with("Error:") {
                    Color::LightRed
                } else {
                    ui::MUTED
                }))
                .wrap(Wrap { trim: false }),
            rows[2],
        );
        let buttons = Layout::horizontal([
            Constraint::Length(15),
            Constraint::Length(14),
            Constraint::Min(0),
        ])
        .split(rows[3]);
        self.geometry.apply = buttons[0];
        self.geometry.cancel = buttons[1];
        ui::button(frame, apply, buttons[0], self.dialog_focus == 1);
        ui::button(frame, "Esc Cancel", buttons[1], self.dialog_focus == 2);
    }
}
