use std::collections::BTreeSet;

use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, IconName, h_flex,
    menu::PopupMenu,
    table::{Column, ColumnSort, TableDelegate, TableState},
};

use crate::{
    CopyPath, Open, Reveal, SearchFolder, icons,
    search::{Entry, human_size},
};

pub enum FilesEvent {
    Sort(usize, bool),
    ContextRow(usize),
}

pub struct Files {
    pub rows: Vec<Entry>,
    pub columns: Vec<Column>,
    pub icons: icons::Cache,
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
    pub clicked: Option<usize>,
    pub cursor: Option<usize>,
}

impl Files {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            icons: icons::Cache::new(),
            selected: BTreeSet::new(),
            anchor: None,
            clicked: None,
            cursor: None,
            columns: [
                ("name", "名称", 280.),
                ("path", "路径", 420.),
                ("type", "类型", 72.),
                ("size", "大小", 84.),
                ("modified", "修改时间", 155.),
            ]
            .into_iter()
            .map(|(key, name, width)| {
                let column = Column::new(key, name).width(px(width)).sortable().p_0();
                match key {
                    "name" => column.ascending(),
                    "size" => column.text_right(),
                    _ => column,
                }
            })
            .collect(),
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.clicked = None;
        self.cursor = None;
    }

    pub fn is_selected(&self, row: usize) -> bool {
        self.selected.contains(&row)
    }

    pub fn selected_entries(&self) -> Vec<Entry> {
        self.selected
            .iter()
            .filter_map(|index| self.rows.get(*index).cloned())
            .collect()
    }

    pub fn select(&mut self, row: usize) {
        self.selected.clear();
        self.selected.insert(row);
        self.anchor = Some(row);
        self.cursor = Some(row);
    }

    pub fn toggle(&mut self, row: usize) {
        if self.selected.remove(&row) {
            self.anchor = Some(row);
        } else {
            self.selected.insert(row);
            self.anchor = Some(row);
        }
        self.cursor = Some(row);
    }

    pub fn extend(&mut self, row: usize) {
        let anchor = *self.anchor.get_or_insert(row);
        let (start, end) = if anchor <= row {
            (anchor, row)
        } else {
            (row, anchor)
        };
        self.selected.clear();
        self.selected.extend(start..=end);
        self.cursor = Some(row);
    }

    pub fn select_all(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (0..self.rows.len()).collect();
        self.anchor.get_or_insert(0);
    }
}

impl EventEmitter<FilesEvent> for TableState<Files> {}

impl TableDelegate for Files {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }
    fn column(&self, index: usize, _: &App) -> &Column {
        &self.columns[index]
    }

    fn perform_sort(
        &mut self,
        column: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        cx.emit(FilesEvent::Sort(column, sort == ColumnSort::Descending));
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> gpui::Stateful<gpui::Div> {
        let current = self.cursor == Some(row);
        div().id(("row", row)).when(current, |row| {
            row.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .bottom_0()
                    .w(px(3.))
                    .bg(cx.theme().selection),
            )
        })
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let entry = &self.rows[row];
        let text = match column {
            0 => entry.name.clone(),
            1 => entry.directory.clone(),
            2 => entry.kind(),
            3 => human_size(entry.size),
            _ => entry.modified.clone(),
        };
        let selected = self.is_selected(row);
        let current = self.cursor == Some(row);
        h_flex()
            .px_1()
            .gap_1()
            .w_full()
            .h_full()
            .overflow_hidden()
            .when(column == 3, |row| row.justify_end())
            .when(selected && !current, |row| {
                row.bg(cx.theme().selection.alpha(0.22))
            })
            .when(current, |row| row.bg(cx.theme().selection))
            .when(column == 0, |row| {
                let icon = self.icons.get(&icons::key(entry)).and_then(Option::as_ref);
                row.child(match icon {
                    Some(icon) => img(icon.clone())
                        .size_4()
                        .flex_shrink_0()
                        .into_any_element(),
                    None => Icon::new(if entry.folder {
                        IconName::Folder
                    } else {
                        IconName::File
                    })
                    .size_4()
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground)
                    .into_any_element(),
                })
            })
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(text),
            )
    }

    fn context_menu(
        &mut self,
        row: usize,
        menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        self.clicked = Some(row);
        if !self.is_selected(row) {
            self.select(row);
        }
        cx.emit(FilesEvent::ContextRow(row));
        menu.action_context(cx.focus_handle())
            .menu("打开", Box::new(Open))
            .menu("打开所在目录", Box::new(Reveal))
            .separator()
            .menu("复制完整路径", Box::new(CopyPath))
            .menu("搜索此目录", Box::new(SearchFolder))
            .separator()
            .menu("删除到回收站", Box::new(crate::Delete))
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(cx.theme().muted_foreground)
            .child("没有搜索结果")
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Files};

    fn sample(count: usize) -> Files {
        let mut files = Files::new();
        files.rows = (0..count)
            .map(|index| Entry {
                name: index.to_string(),
                directory: String::new(),
                size: None,
                modified: String::new(),
                folder: false,
            })
            .collect();
        files
    }

    #[test]
    fn ctrl_and_shift_selection() {
        let mut files = sample(5);
        files.select(1);
        files.toggle(3);
        assert_eq!(files.selected.iter().copied().collect::<Vec<_>>(), [1, 3]);
        files.extend(4);
        assert_eq!(files.cursor, Some(4));
        assert_eq!(files.selected.iter().copied().collect::<Vec<_>>(), [3, 4]);
        files.select_all();
        assert_eq!(files.selected.len(), 5);
        assert_eq!(files.anchor, Some(3));
        files.clear_selection();
        assert!(files.selected.is_empty());
    }
}
