mod tree;
mod view;

use std::{
    collections::{HashMap, HashSet},
    sync::mpsc,
    thread,
    time::Duration,
};

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode as K, KeyEvent, KeyEventKind, KeyModifiers as M, MouseButton,
    MouseEventKind,
};
use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::TextArea;

use super::{clipboard::TextClipboard, output::Output, widgets as ui};
use crate::{
    config::Config,
    history::SessionSummary,
    model::{Scope, Visibility},
    service::{
        self, HistorySyncRequest, ProjectView, PullRequest, PushRequest, RemoteSkillView, SkillView,
    },
};

#[derive(Clone)]
enum Item {
    Local(SkillView),
    Remote(RemoteSkillView),
    Project(ProjectView),
    Session(SessionSummary),
}
impl Item {
    fn label(&self) -> String {
        match self {
            Self::Local(s) => format!("{} [{}]", s.name, s.scope),
            Self::Remote(s) => format!(
                "{} [{}] {} ({}, v{})",
                s.name, s.scope, s.author_email, s.visibility, s.sync_version
            ),
            Self::Project(p) => format!("{} ({} chats)", p.name, p.session_count),
            Self::Session(s) => s.title.clone(),
        }
    }
    fn document(&self) -> String {
        match self {
            Self::Local(s) => format!(
                "{} [{}]\n{}\n{} files\n\nEnter or Right: expand skill\nSelect a file to read it.\nF6: push the whole skill",
                s.name,
                s.scope,
                s.project_key,
                s.files.len()
            ),
            Self::Remote(s) => format!(
                "{} [{}]\nAuthor: {} | {} | v{}\n{} files\n\nEnter or Right: expand skill\nSelect a file to read it.\nF7: pull the whole skill",
                s.name,
                s.scope,
                s.author_email,
                s.visibility,
                s.sync_version,
                s.files.len()
            ),
            Self::Project(p) => format!(
                "{}\n{}\n{} conversations\n\nEnter: browse conversations\nF5: sync this project's history\nCtrl+S: sync all",
                p.name, p.path, p.session_count
            ),
            Self::Session(s) => format!(
                "{}\n{}\n\nEnter: read conversation",
                s.title,
                s.started_at.as_deref().unwrap_or("Unknown date")
            ),
        }
    }
}
#[derive(Clone)]
enum Action {
    Refresh,
    Sessions(ProjectView),
    Read(SessionSummary),
    SyncAll,
    Push(SkillView, Visibility),
    Pull(RemoteSkillView, bool),
    History(ProjectView),
}
enum Update {
    Items(Vec<Item>),
    Document(String),
    Done(String, Option<PullRequest>),
    Connected(Config),
}
enum Dialog {
    Connection,
    Filter,
    Overwrite(PullRequest),
    Help,
}

#[derive(Default)]
struct Geometry {
    tabs: Vec<Rect>,
    buttons: Vec<Rect>,
    browser: Rect,
    output: Rect,
    field: Rect,
    apply: Rect,
    cancel: Rect,
}

pub struct App {
    config: Option<Config>,
    tab: usize,
    items: Vec<Item>,
    selection: ListState,
    output: Output,
    focus_output: bool,
    status: String,
    busy: bool,
    receiver: Option<mpsc::Receiver<Result<Update>>>,
    dialog: Option<Dialog>,
    dialog_focus: usize,
    input: TextArea<'static>,
    pending: Option<Action>,
    geometry: Geometry,
    clipboard: TextClipboard,
    visibility: Visibility,
    filter: String,
    expanded: HashSet<String>,
    project: Option<ProjectView>,
    dragging: bool,
    too_small: bool,
}
impl App {
    fn empty(config: Option<Config>) -> Self {
        Self {
            config,
            tab: 0,
            items: Vec::new(),
            selection: ListState::default(),
            output: Output::default(),
            focus_output: false,
            status: "Loading local skills...".into(),
            busy: false,
            receiver: None,
            dialog: None,
            dialog_focus: 0,
            input: ui::text(""),
            pending: None,
            geometry: Geometry::default(),
            clipboard: TextClipboard::new("AGENTSYNC_CLIPBOARD"),
            visibility: Visibility::Private,
            filter: String::new(),
            expanded: HashSet::new(),
            project: None,
            dragging: false,
            too_small: false,
        }
    }
    pub fn new(config: Option<Config>) -> Self {
        let mut app = Self::empty(config);
        app.action(Action::Refresh);
        app
    }
    fn spawn(&mut self, task: impl FnOnce() -> Result<Update> + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(task());
        });
        self.receiver = Some(rx);
        self.busy = true;
        self.status = "Working...".into();
    }
    fn action(&mut self, action: Action) {
        if self.busy {
            return;
        }
        let needs_connection = matches!(
            action,
            Action::SyncAll | Action::Push(..) | Action::Pull(..) | Action::History(..)
        ) || (matches!(action, Action::Refresh) && self.tab == 1);
        if needs_connection && self.config.is_none() {
            self.pending = Some(action);
            self.input = ui::text("");
            self.input.set_mask_char('•');
            self.dialog_focus = 0;
            self.dialog = Some(Dialog::Connection);
            self.status = "Enter a SQL connection URL to sync.".into();
            return;
        }
        let config = self.config.clone();
        let tab = self.tab;
        let project = self.project.clone();
        self.spawn(move || {
            let connected = || config.as_ref().ok_or_else(|| anyhow::anyhow!("Connect to a database first"));
            match action {
                Action::Refresh => {
                    let items = match tab {
                        0 => service::list_skills()?.into_iter().map(Item::Local).collect(),
                        1 => service::list_remote_skills(connected()?)?.into_iter().map(Item::Remote).collect(),
                        _ => match project {
                            Some(p) => service::list_sessions(&p.path)?.into_iter().map(Item::Session).collect(),
                            None => service::list_projects()?.into_iter().map(Item::Project).collect(),
                        },
                    }; Ok(Update::Items(items))
                }
                Action::Sessions(p) => Ok(Update::Items(service::list_sessions(&p.path)?.into_iter().map(Item::Session).collect())),
                Action::Read(s) => {
                    let session = service::load_session(&s.path)?;
                    let mut document = format!("{}\n{}\n", session.summary.title, session.summary.project_path);
                    for message in session.messages { document.push_str(&format!("\n[{}] {}\n{}\n", message.role, message.timestamp.as_deref().unwrap_or(""), message.content)); }
                    Ok(Update::Document(document))
                }
                Action::SyncAll => Ok(Update::Done(super::summary(&service::sync_all(connected()?)?), None)),
                Action::History(p) => Ok(Update::Done(super::summary(&service::sync_history(connected()?, HistorySyncRequest { project_root: p.path, project_key: Some(p.key) })?), None)),
                Action::Push(s, visibility) => Ok(Update::Done(super::summary(&service::push(connected()?, PushRequest {
                    scope: s.scope, project_root: s.project_path, project_key: Some(s.project_key), resource: "skills".into(),
                    default_visibility: visibility, skill_visibility: HashMap::new(), skill_name: Some(s.name),
                })?), None)),
                Action::Pull(s, overwrite) => {
                    let root = if s.scope == Scope::Project {
                        Some(service::list_projects()?.into_iter().find(|p| p.key == s.project_key)
                            .ok_or_else(|| anyhow::anyhow!("No local project matches this skill. Open the matching checkout and restart AgentSync."))?.path)
                    } else { None };
                    let request = PullRequest { scope: s.scope, project_root: root, project_key: Some(s.project_key), resource: "skills".into(),
                        author: Some(s.author_email), overwrite, skill_name: Some(s.name) };
                    let result = service::pull(connected()?, request.clone())?;
                    let confirmation = (result.conflicts > 0).then_some(request);
                    Ok(Update::Done(super::summary(&result), confirmation))
                }
            }
        });
    }
    fn visible(&self) -> Vec<tree::Row> {
        tree::rows(&self.items, &self.expanded, &self.filter)
    }
    fn selected_row(&self) -> Option<tree::Row> {
        self.selection
            .selected()
            .and_then(|i| self.visible().get(i).cloned())
    }
    fn selected(&self) -> Option<Item> {
        self.selected_row().map(|row| self.items[row.item].clone())
    }
    fn preview(&mut self) {
        let text = self.selected_row().map_or_else(
            || "No items. F3 refreshes; / filters by name, author or file path.".into(),
            |row| match row.node {
                tree::Node::File(index) => {
                    self.items[row.item].files().unwrap()[index].content.clone()
                }
                tree::Node::Directory(path) => {
                    format!("{path}\n\nEnter or Right: expand directory\nSelect a file to read it.")
                }
                tree::Node::Item => self.items[row.item].document(),
            },
        );
        self.output.set(&text, "");
    }
    fn toggle_branch(&mut self) -> bool {
        if let Some(row) = self.selected_row().filter(|row| row.branch) {
            if !self.expanded.remove(&row.key) {
                self.expanded.insert(row.key);
            }
            self.preview();
            return true;
        }
        false
    }
    fn tree_arrow(&mut self, right: bool) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if right {
            if row.branch {
                if !row.open {
                    self.toggle_branch();
                } else if self
                    .visible()
                    .get(self.selection.selected().unwrap_or(0) + 1)
                    .is_some_and(|child| child.parent.as_deref() == Some(&row.key))
                {
                    self.select(self.selection.selected().unwrap_or(0) + 1);
                }
            }
        } else if row.branch && row.open && self.filter.is_empty() {
            self.toggle_branch();
        } else if let Some(parent) = row.parent
            && let Some(index) = self.visible().iter().position(|row| row.key == parent)
        {
            self.select(index);
        }
    }
    fn select(&mut self, index: usize) {
        let len = self.visible().len();
        self.selection
            .select((len > 0).then_some(index.min(len.saturating_sub(1))));
        self.preview();
    }
    fn poll(&mut self) {
        let result = self.receiver.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err(anyhow::anyhow!("background task stopped unexpectedly")))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        });
        if let Some(result) = result {
            self.receiver = None;
            self.busy = false;
            match result {
                Ok(Update::Items(items)) => {
                    self.items = items;
                    self.select(0);
                    self.status = format!("{} items", self.items.len());
                }
                Ok(Update::Document(document)) => {
                    self.output.set(&document, "");
                    self.focus_output = true;
                    self.status = "Conversation loaded (plain text)".into();
                }
                Ok(Update::Done(status, confirmation)) => {
                    self.status = status.clone();
                    self.output.set(&status, "");
                    if let Some(request) = confirmation {
                        self.dialog_focus = 1;
                        self.dialog = Some(Dialog::Overwrite(request));
                    }
                }
                Ok(Update::Connected(config)) => {
                    self.config = Some(config);
                    self.dialog = None;
                    self.status = "Connected. URL saved privately in config.toml.".into();
                    if let Some(action) = self.pending.take() {
                        self.action(action);
                    }
                }
                Err(error) => {
                    self.status = format!("Error: {error:#}");
                    if self.dialog.is_none() {
                        self.output.set(&self.status, "");
                    }
                }
            }
        }
    }
    fn switch(&mut self, tab: usize) {
        if self.busy {
            return;
        }
        self.tab = tab;
        self.project = None;
        self.filter.clear();
        self.items.clear();
        self.expanded.clear();
        self.selection.select(None);
        self.output.set("", "");
        self.focus_output = false;
        self.action(Action::Refresh);
    }
    fn activate(&mut self) {
        if self.toggle_branch() {
            return;
        }
        match self.selected() {
            Some(Item::Project(p)) => {
                self.project = Some(p.clone());
                self.filter.clear();
                self.action(Action::Sessions(p));
            }
            Some(Item::Session(s)) => self.action(Action::Read(s)),
            _ => self.focus_output = true,
        }
    }
    fn main_sync(&mut self) {
        if self.tab == 2
            && let Some(p) = self.project.clone().or_else(|| match self.selected() {
                Some(Item::Project(p)) => Some(p),
                _ => None,
            })
        {
            self.action(Action::History(p));
            return;
        }
        self.action(Action::SyncAll);
    }
    fn toolbar(&mut self, i: usize) {
        match i {
            0 => self.main_sync(),
            1 => {
                if let Some(Item::Local(s)) = self.selected() {
                    self.action(Action::Push(s, self.visibility));
                }
            }
            2 => {
                if let Some(Item::Remote(s)) = self.selected() {
                    self.action(Action::Pull(s, false));
                }
            }
            3 => {
                self.dialog_focus = 1;
                self.dialog = Some(Dialog::Help);
            }
            _ => {}
        }
    }
    fn submit(&mut self) {
        if self.busy {
            return;
        }
        match self.dialog.take() {
            Some(Dialog::Connection) => {
                let url = ui::value(&self.input);
                let config = Config::from_url(url);
                if let Err(error) = config.validate() {
                    self.status = format!("Error: {error}");
                    self.dialog = Some(Dialog::Connection);
                    return;
                }
                self.dialog = Some(Dialog::Connection);
                self.spawn(move || Ok(Update::Connected(config.connect_and_save()?)));
            }
            Some(Dialog::Filter) => {
                self.filter = ui::value(&self.input);
                self.select(0);
            }
            Some(Dialog::Overwrite(mut request)) => {
                request.overwrite = true;
                let config = self.config.clone().unwrap();
                self.spawn(move || {
                    Ok(Update::Done(
                        super::summary(&service::pull(&config, request)?),
                        None,
                    ))
                });
            }
            _ => {}
        }
    }
    fn key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if key.modifiers.contains(M::CONTROL) && key.code == K::Char('q') {
            if self.busy {
                self.status = "Operation running. Wait for completion before quitting; committed changes persist.".into();
                return false;
            }
            return true;
        }
        if self.too_small {
            return false;
        }
        if self.dialog.is_some() {
            match key.code {
                K::Esc if !self.busy => {
                    self.dialog = None;
                    self.pending = None;
                }
                K::Tab | K::BackTab => {
                    let first = usize::from(!matches!(
                        self.dialog,
                        Some(Dialog::Connection | Dialog::Filter)
                    ));
                    self.dialog_focus = if key.code == K::BackTab {
                        if self.dialog_focus <= first {
                            2
                        } else {
                            self.dialog_focus - 1
                        }
                    } else if self.dialog_focus >= 2 {
                        first
                    } else {
                        self.dialog_focus + 1
                    };
                }
                K::Enter if self.dialog_focus == 2 && !self.busy => {
                    self.dialog = None;
                    self.pending = None;
                }
                K::Enter => self.submit(),
                _ if matches!(self.dialog, Some(Dialog::Connection | Dialog::Filter))
                    && !self.busy
                    && self.dialog_focus == 0 =>
                {
                    self.edit_input(key)
                }
                _ => {}
            }
            return false;
        }
        if key.modifiers.contains(M::ALT) {
            match key.code {
                K::Char('1') => self.switch(0),
                K::Char('2') => self.switch(1),
                K::Char('3') => self.switch(2),
                _ => {}
            }
            return false;
        }
        if key.modifiers.contains(M::CONTROL) {
            match key.code {
                K::Char('c') => {
                    let text = if self.focus_output {
                        self.output.copy(0)
                    } else {
                        self.selected_row().map_or(String::new(), |row| row.label)
                    };
                    self.clipboard.copy(text);
                    self.status = "Copied (desktop clipboard or internal fallback)".into();
                    return false;
                }
                K::Char('s') => {
                    self.action(Action::SyncAll);
                    return false;
                }
                _ => {}
            }
        }
        match key.code {
            K::Tab | K::BackTab => self.focus_output = !self.focus_output,
            K::F(1) => {
                self.dialog_focus = 1;
                self.dialog = Some(Dialog::Help);
            }
            K::F(3) => self.action(Action::Refresh),
            K::F(5) => self.main_sync(),
            K::F(6) => self.toolbar(1),
            K::F(7) => self.toolbar(2),
            K::F(8) => {
                self.visibility = if self.visibility == Visibility::Private {
                    Visibility::Public
                } else {
                    Visibility::Private
                };
            }
            K::Char('/') => {
                self.dialog_focus = 0;
                self.input = ui::text(&self.filter);
                self.dialog = Some(Dialog::Filter);
            }
            K::Esc if !self.busy => {
                self.focus_output = false;
                if self.project.take().is_some() {
                    self.filter.clear();
                    self.action(Action::Refresh);
                }
            }
            _ if self.focus_output => self.output.key(0, key),
            K::Left => self.tree_arrow(false),
            K::Right => self.tree_arrow(true),
            K::Up => self.select(self.selection.selected().unwrap_or(0).saturating_sub(1)),
            K::Down => self.select(self.selection.selected().map_or(0, |i| i + 1)),
            K::Enter if !self.busy => self.activate(),
            _ => {}
        }
        false
    }
    fn edit_input(&mut self, key: KeyEvent) {
        if key.modifiers.contains(M::CONTROL) {
            match key.code {
                K::Char('c') => {
                    self.clipboard.copy(ui::copied(&self.input));
                    return;
                }
                K::Char('v') => {
                    match self.clipboard.paste() {
                        Ok(text) => {
                            self.input.insert_str(text.replace(['\r', '\n'], ""));
                        }
                        Err(e) => self.status = e.to_string(),
                    }
                    return;
                }
                _ => {}
            }
        }
        if key.code == K::Char('m') && key.modifiers.contains(M::CONTROL) {
            return;
        }
        if !matches!(
            key.code,
            K::Enter | K::Tab | K::BackTab | K::Char('\n' | '\r')
        ) {
            ui::edit(&mut self.input, key);
        }
    }
    fn mouse(&mut self, event: event::MouseEvent) {
        let point = Position::new(event.column, event.row);
        if self.dialog.is_some() {
            if event.kind == MouseEventKind::Down(MouseButton::Left) {
                if self.geometry.apply.contains(point) {
                    self.submit();
                } else if self.geometry.cancel.contains(point) && !self.busy {
                    self.dialog = None;
                    self.pending = None;
                } else if self.geometry.field.contains(point) && !self.busy {
                    self.dialog_focus = 0;
                    self.input.cancel_selection();
                    ui::click(&mut self.input, self.geometry.field, point);
                    self.input.start_selection();
                    self.dragging = true;
                }
            } else if event.kind == MouseEventKind::Drag(MouseButton::Left) && self.dragging {
                ui::click(&mut self.input, self.geometry.field, point);
            } else if event.kind == MouseEventKind::Up(MouseButton::Left) {
                self.dragging = false;
            }
            return;
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.dragging = false;
                if let Some(i) = self.geometry.tabs.iter().position(|r| r.contains(point)) {
                    self.switch(i);
                } else if let Some(i) = self.geometry.buttons.iter().position(|r| r.contains(point))
                {
                    self.toolbar(i);
                } else if self
                    .geometry
                    .browser
                    .inner(Margin::new(1, 1))
                    .contains(point)
                {
                    self.focus_output = false;
                    let index = self.selection.offset()
                        + usize::from(point.y - self.geometry.browser.y - 1);
                    if index < self.visible().len() {
                        let same = self.selection.selected() == Some(index);
                        self.select(index);
                        if !self.busy && (same || self.selected_row().is_some_and(|row| row.branch))
                        {
                            self.activate();
                        }
                    }
                } else if self.output.click(0, self.geometry.output, point, true) {
                    self.focus_output = true;
                    self.dragging = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.output.click(0, self.geometry.output, point, false);
            }
            MouseEventKind::Up(MouseButton::Left) => self.dragging = false,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if event.kind == MouseEventKind::ScrollUp {
                    -3
                } else {
                    3
                };
                if self.geometry.output.contains(point) {
                    if event.modifiers.contains(M::SHIFT) {
                        self.output.scroll(0, 0, delta);
                    } else {
                        self.output.scroll(0, delta, 0);
                    }
                } else if self.geometry.browser.contains(point) {
                    self.select(
                        self.selection
                            .selected()
                            .unwrap_or(0)
                            .saturating_add_signed(delta as isize),
                    );
                }
            }
            _ => {}
        }
    }
    pub fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        loop {
            self.poll();
            terminal.draw(|frame| self.draw(frame))?;
            if !event::poll(Duration::from_millis(50))? {
                continue;
            }
            match event::read()? {
                Event::Key(key) if self.key(key) => break,
                Event::Paste(text)
                    if !self.too_small
                        && !self.busy
                        && matches!(self.dialog, Some(Dialog::Connection | Dialog::Filter))
                        && self.dialog_focus == 0 =>
                {
                    self.input.insert_str(text.replace(['\r', '\n'], ""));
                }
                Event::Mouse(mouse) => self.mouse(mouse),
                Event::Resize(..) => {
                    self.geometry = Geometry::default();
                    self.dragging = false;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    pub(super) fn render_for_tree(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let text = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        if let Ok(directory) = std::env::var("AGENTSYNC_LAYOUT_DIR") {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                format!(
                    "{directory}/{width}x{height}-{}.txt",
                    if app.dialog.is_some() {
                        "dialog"
                    } else {
                        "main"
                    }
                ),
                &text,
            )
            .unwrap();
        }
        text
    }
    #[test]
    fn layouts_show_complete_controls_and_clear_hit_targets_on_resize() {
        let mut app = App::empty(None);
        for (width, height) in [(72, 26), (80, 26), (100, 30), (120, 40), (160, 48)] {
            let text = render_for_tree(&mut app, width, height);
            for label in [
                "AgentSync",
                "F5 Sync",
                "F6 Push",
                "F7 Pull",
                "F1 Help",
                "Alt+1 Local skills",
                "Alt+2 Remote skills",
                "Alt+3 History",
                "^Q quit",
            ] {
                assert!(
                    text.contains(label),
                    "missing {label} at {width}x{height}\n{text}"
                );
            }
            for rect in &app.geometry.buttons {
                assert_eq!(rect.y, 0);
                assert_eq!(rect.height, 3);
                assert!(rect.right() <= width);
            }
            for pair in app.geometry.buttons.windows(2) {
                assert!(pair[0].right() <= pair[1].x);
            }
            app.dialog = Some(Dialog::Connection);
            let text = render_for_tree(&mut app, width, height);
            for label in [
                "SQL connection URL",
                "Connect",
                "Esc Cancel",
                "Postgres, SQLite, MySQL/MariaDB, libSQL/Turso.",
            ] {
                assert!(text.contains(label), "{label}\n{text}");
            }
            assert!(app.geometry.apply.bottom() < height);
            app.dialog = None;
        }
        assert!(render_for_tree(&mut app, 55, 18).contains("Ctrl+Q quits"));
        assert!(app.geometry.buttons.is_empty());
        assert_eq!(app.geometry.output, Rect::default());
        app.key(KeyEvent::new(K::F(5), M::NONE));
        assert!(app.dialog.is_none(), "hidden controls cannot start a sync");
    }
    #[test]
    fn focus_cycles_dialog_cancellation_and_blank_clicks_are_safe() {
        let mut app = App::empty(None);
        app.key(KeyEvent::new(K::Tab, M::NONE));
        assert!(app.focus_output);
        app.key(KeyEvent::new(K::BackTab, M::SHIFT));
        assert!(!app.focus_output);
        app.dialog = Some(Dialog::Filter);
        app.input = ui::text("original");
        app.key(KeyEvent::new(K::Tab, M::NONE));
        assert_eq!(app.dialog_focus, 1);
        app.key(KeyEvent::new(K::BackTab, M::SHIFT));
        assert_eq!(app.dialog_focus, 0);
        app.key(KeyEvent::new(K::BackTab, M::SHIFT));
        assert_eq!(app.dialog_focus, 2);
        app.key(KeyEvent::new(K::Enter, M::NONE));
        assert!(app.dialog.is_none());
        assert!(app.filter.is_empty());
        render_for_tree(&mut app, 72, 26);
        app.mouse(event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 12,
            row: 0,
            modifiers: M::NONE,
        });
        assert!(!app.busy);
        assert!(app.dialog.is_none());
        app.key(KeyEvent::new(K::F(5), M::NONE));
        assert!(matches!(app.dialog, Some(Dialog::Connection)));
        app.key(KeyEvent::new(K::Esc, M::NONE));
        assert!(app.pending.is_none());
    }
    #[test]
    fn filters_have_explicit_clipboard_undo_redo_and_readonly_output() {
        let mut app = App::empty(None);
        app.dialog = Some(Dialog::Filter);
        app.clipboard.copy("hello 世界\nsecond".into());
        app.edit_input(KeyEvent::new(K::Char('v'), M::CONTROL));
        assert_eq!(ui::value(&app.input), "hello 世界second");
        app.edit_input(KeyEvent::new(K::Char('z'), M::CONTROL));
        assert_eq!(ui::value(&app.input), "");
        app.edit_input(KeyEvent::new(K::Char('y'), M::CONTROL));
        assert_eq!(ui::value(&app.input), "hello 世界second");
        app.edit_input(KeyEvent::new(K::Char('a'), M::CONTROL));
        app.edit_input(KeyEvent::new(K::Char('c'), M::CONTROL));
        assert_eq!(app.clipboard.paste().unwrap(), "hello 世界second");
        app.dialog = None;
        app.focus_output = true;
        app.output.set("immutable", "");
        app.key(KeyEvent::new(K::Char('v'), M::CONTROL));
        app.key(KeyEvent::new(K::Char('z'), M::CONTROL));
        assert_eq!(app.output.copy(0), "immutable");
    }
}

#[cfg(test)]
mod tree_tests {
    use super::*;

    fn skill() -> SkillView {
        SkillView {
            id: "global:test".into(),
            name: "test".into(),
            scope: Scope::Global,
            project_key: "".into(),
            project_path: None,
            files: vec![
                service::SkillFileView {
                    path: "skills/test/SKILL.md".into(),
                    title: "SKILL.md".into(),
                    content: "skill document".into(),
                },
                service::SkillFileView {
                    path: "skills/test/scripts/run.rs".into(),
                    title: "scripts/run.rs".into(),
                    content: "helper document 世界".into(),
                },
            ],
        }
    }
    #[test]
    fn skill_tree_opens_individual_files_and_navigates_parents() {
        let mut app = App::empty(None);
        app.items = vec![Item::Local(skill())];
        app.select(0);
        assert_eq!(app.visible().len(), 1);
        assert!(!app.output.copy(0).contains("skill document"));
        app.key(KeyEvent::new(K::Right, M::NONE));
        assert_eq!(app.visible().len(), 3); // skill, scripts/, SKILL.md
        app.key(KeyEvent::new(K::Down, M::NONE));
        app.key(KeyEvent::new(K::Enter, M::NONE));
        assert_eq!(app.visible().len(), 4);
        app.key(KeyEvent::new(K::Down, M::NONE));
        assert_eq!(app.output.copy(0), "helper document 世界");
        assert!(matches!(app.selected(),Some(Item::Local(s)) if s.name == "test"));
        app.key(KeyEvent::new(K::Left, M::NONE));
        assert!(matches!(
            app.selected_row().unwrap().node,
            tree::Node::Directory(_)
        ));
        app.key(KeyEvent::new(K::Left, M::NONE));
        assert_eq!(app.visible().len(), 3);
        app.key(KeyEvent::new(K::Left, M::NONE));
        assert_eq!(app.selection.selected(), Some(0));
        app.key(KeyEvent::new(K::Left, M::NONE));
        assert_eq!(app.visible().len(), 1);
        app.key(KeyEvent::new(K::Enter, M::NONE));
        app.select(2);
        assert_eq!(app.output.copy(0), "skill document");
        let rendered = super::tests::render_for_tree(&mut app, 72, 26);
        assert!(rendered.contains("File: SKILL.md"));
        assert!(!rendered.contains("Browse | /"));
    }
    #[test]
    fn remote_file_filter_preserves_ancestors_and_sync_owner() {
        let mut app = App::empty(None);
        let s = skill();
        app.items = vec![Item::Remote(RemoteSkillView {
            id: "author:test".into(),
            name: s.name,
            scope: s.scope,
            project_key: s.project_key,
            author_email: "author@example.com".into(),
            visibility: Visibility::Public,
            sync_version: 7,
            files: s.files,
        })];
        app.filter = "run.rs".into();
        app.select(0);
        let rows = app.visible();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].label.contains("author@example.com"));
        assert!(rows[1].label.contains("scripts/"));
        app.select(2);
        assert_eq!(app.output.copy(0), "helper document 世界");
        assert!(
            matches!(app.selected(),Some(Item::Remote(s)) if s.author_email == "author@example.com")
        );
        app.filter.clear();
        app.select(0);
        assert_eq!(app.visible().len(), 1);
        // A mouse click on the folder expands it even when another row was selected.
        super::tests::render_for_tree(&mut app, 72, 26);
        app.mouse(event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 5,
            modifiers: M::NONE,
        });
        assert_eq!(app.visible().len(), 3);
    }
}
