use super::*;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Node {
    Item,
    Directory(String),
    File(usize),
}
#[derive(Clone)]
pub(super) struct Row {
    pub item: usize,
    pub node: Node,
    pub key: String,
    pub parent: Option<String>,
    pub label: String,
    pub branch: bool,
    pub open: bool,
}
impl Item {
    pub(super) fn files(&self) -> Option<&[service::SkillFileView]> {
        match self {
            Self::Local(s) => Some(&s.files),
            Self::Remote(s) => Some(&s.files),
            _ => None,
        }
    }
    fn tree_key(&self) -> String {
        match self {
            Self::Local(s) => s.id.clone(),
            Self::Remote(s) => s.id.clone(),
            _ => self.label(),
        }
    }
}

#[derive(Default)]
struct Directory {
    directories: BTreeMap<String, Directory>,
    files: Vec<(usize, String)>,
}
impl Directory {
    fn insert(&mut self, path: &str, index: usize) {
        if let Some((name, rest)) = path.split_once('/') {
            self.directories
                .entry(name.into())
                .or_default()
                .insert(rest, index);
        } else {
            self.files.push((index, path.into()));
        }
    }
    fn rows(
        &self,
        item: usize,
        parent: &str,
        indent: &str,
        expanded: &HashSet<String>,
        filtering: bool,
        rows: &mut Vec<Row>,
    ) {
        let total = self.directories.len() + self.files.len();
        let mut position = 0;
        for (name, directory) in &self.directories {
            position += 1;
            let last = position == total;
            let key = format!("{parent}/{name}");
            let open = filtering || expanded.contains(&key);
            rows.push(Row {
                item,
                node: Node::Directory(name.clone()),
                key: key.clone(),
                parent: Some(parent.into()),
                label: format!(
                    "{indent}{} {} {name}/",
                    if last { "└" } else { "├" },
                    if open { "▾" } else { "▸" }
                ),
                branch: true,
                open,
            });
            if open {
                directory.rows(
                    item,
                    &key,
                    &format!("{indent}{} ", if last { " " } else { "│" }),
                    expanded,
                    filtering,
                    rows,
                );
            }
        }
        for (index, name) in &self.files {
            position += 1;
            rows.push(Row {
                item,
                node: Node::File(*index),
                key: format!("{parent}/{name}"),
                parent: Some(parent.into()),
                label: format!(
                    "{indent}{} {name}",
                    if position == total { "└" } else { "├" }
                ),
                branch: false,
                open: false,
            });
        }
    }
}

pub(super) fn rows(items: &[Item], expanded: &HashSet<String>, filter: &str) -> Vec<Row> {
    let query = filter.to_lowercase();
    let mut rows = Vec::new();
    for (item_index, item) in items.iter().enumerate() {
        let label = item.label();
        let matches = label.to_lowercase().contains(&query);
        let files = item.files();
        let matching_files = files
            .map(|files| {
                files
                    .iter()
                    .enumerate()
                    .filter(|(_, file)| matches || file.title.to_lowercase().contains(&query))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !matches && matching_files.is_empty() {
            continue;
        }
        let key = item.tree_key();
        let branch = files.is_some();
        let open = branch && (!query.is_empty() || expanded.contains(&key));
        rows.push(Row {
            item: item_index,
            node: Node::Item,
            key: key.clone(),
            parent: None,
            label: if branch {
                format!("{} {label}", if open { "▾" } else { "▸" })
            } else {
                label
            },
            branch,
            open,
        });
        if open {
            let mut directory = Directory::default();
            for (i, file) in matching_files {
                directory.insert(&file.title, i);
            }
            directory.rows(
                item_index,
                &key,
                "  ",
                expanded,
                !query.is_empty(),
                &mut rows,
            );
        }
    }
    rows
}
