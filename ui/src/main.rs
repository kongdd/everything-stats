#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod es;
mod icons;
mod search;

use std::{
    cell::Cell,
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use egui_extras::{Column, TableBuilder};
use search::{Entry, Filter, PAGE_SIZE, Query};

fn main() -> eframe::Result {
    eframe::run_native(
        "Everything",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1200.0, 760.0])
                .with_min_inner_size([800.0, 480.0])
                .with_title("Everything"),
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(Everything::new(cc)))),
    )
}

#[derive(Clone, Default)]
struct Selection {
    rows: BTreeSet<usize>,
    anchor: Option<usize>,
    cursor: Option<usize>,
}

impl Selection {
    fn clear(&mut self) {
        *self = Self::default();
    }

    fn select(&mut self, row: usize) {
        self.rows.clear();
        self.rows.insert(row);
        self.anchor = Some(row);
        self.cursor = Some(row);
    }

    fn toggle(&mut self, row: usize) {
        if !self.rows.remove(&row) {
            self.rows.insert(row);
        }
        self.anchor = Some(row);
        self.cursor = Some(row);
    }

    fn extend(&mut self, row: usize) {
        let anchor = *self.anchor.get_or_insert(row);
        let (start, end) = if anchor <= row {
            (anchor, row)
        } else {
            (row, anchor)
        };
        self.rows.clear();
        self.rows.extend(start..=end);
        self.cursor = Some(row);
    }

    fn move_to(&mut self, delta: isize, len: usize) {
        if len == 0 {
            return;
        }
        let current = self.cursor.unwrap_or(0).min(len - 1);
        self.select((current as isize + delta).clamp(0, len as isize - 1) as usize);
    }

    fn nudge(&mut self, delta: isize, len: usize) {
        if len == 0 {
            return;
        }
        let current = self.cursor.unwrap_or(0).min(len - 1);
        if self.anchor.is_none() {
            self.select(current);
        }
        self.extend((current as isize + delta).clamp(0, len as isize - 1) as usize);
    }

    fn all(&mut self, len: usize) {
        self.rows.clear();
        self.rows.extend(0..len);
        self.cursor.get_or_insert(0);
    }
}

#[derive(Clone, Copy)]
enum Click {
    Row {
        row: usize,
        ctrl: bool,
        shift: bool,
        double: bool,
        right: bool,
    },
    Open,
    Reveal,
    Copy,
    Delete,
    Folder,
    Sort(usize),
}

type Icon = (String, Option<std::sync::Arc<image::RgbaImage>>);

enum Job {
    Deleted(anyhow::Result<bool>, usize),
    Exported(anyhow::Result<PathBuf>),
}

struct Everything {
    text: String,
    query: Query,
    rows: Vec<Entry>,
    total: usize,
    selection: Selection,
    loading: bool,
    exporting: bool,
    deleting: bool,
    revision: u64,
    search_rx: Option<(u64, Receiver<anyhow::Result<search::Page>>)>,
    icon_rx: Option<(u64, Receiver<Icon>)>,
    job_rx: Option<Receiver<Job>>,
    textures: std::collections::HashMap<String, Option<TextureHandle>>,
    icon_cache: icons::Cache,
    error: Option<String>,
    note: String,
    bookmarks: Vec<Query>,
    bookmark_error: Option<String>,
    dirty: Option<Instant>,
    dark: bool,
    from_search: bool,
    menu: Option<&'static str>,
    follow: bool,
    cols: [f32; 5],
}

impl Everything {
    fn new(cc: &eframe::CreationContext) -> Self {
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            use windows::Win32::{
                Foundation::HWND,
                Graphics::Dwm::{DWMWA_BORDER_COLOR, DwmSetWindowAttribute},
                UI::WindowsAndMessaging::{SW_MAXIMIZE, ShowWindow},
            };
            if let Ok(handle) = cc.window_handle()
                && let RawWindowHandle::Win32(handle) = handle.as_raw()
            {
                let hwnd = HWND(handle.hwnd.get() as _);
                // `ViewportBuilder::with_maximized` maximizes before winit knows the
                // monitor and leaves the default size plus an unpainted frame on the right.
                unsafe {
                    let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                };
                // Match the native title bar instead of leaving a dark non-client strip.
                let color = 0x00f3f3f3_u32;
                if let Err(error) = unsafe {
                    DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_BORDER_COLOR,
                        &color as *const _ as _,
                        std::mem::size_of_val(&color) as _,
                    )
                } {
                    // Border colors are unsupported on Windows 10; keep its native frame.
                    eprintln!("Cannot set window border color: {error}");
                }
            }
        }
        style(&cc.egui_ctx, false);
        cc.egui_ctx
            .memory_mut(|memory| memory.request_focus(egui::Id::new("search")));
        let (bookmarks, bookmark_error) = match search::load_bookmarks() {
            Ok(bookmarks) => (bookmarks, None),
            Err(error) => (Vec::new(), Some(format!("{error:#}"))),
        };
        let mut app = Self {
            text: String::new(),
            query: Query::default(),
            rows: Vec::new(),
            total: 0,
            selection: Selection::default(),
            loading: false,
            exporting: false,
            deleting: false,
            revision: 0,
            search_rx: None,
            icon_rx: None,
            job_rx: None,
            textures: std::collections::HashMap::new(),
            icon_cache: icons::Cache::new(),
            error: bookmark_error.clone(),
            note: String::new(),
            bookmarks,
            bookmark_error,
            dirty: None,
            dark: false,
            from_search: false,
            menu: None,
            follow: false,
            cols: [280.0, 120.0, 72.0, 84.0, 148.0],
        };
        app.start_search(&cc.egui_ctx);
        app
    }

    fn start_search(&mut self, ctx: &egui::Context) {
        self.revision += 1;
        let revision = self.revision;
        let query = self.query.clone();
        self.loading = true;
        self.error = None;
        self.note.clear();
        self.selection.clear();
        let (tx, rx) = mpsc::channel();
        self.search_rx = Some((revision, rx));
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(search::search(&query));
            ctx.request_repaint();
        });
    }

    fn restart(&mut self, ctx: &egui::Context) {
        self.query.offset = 0;
        self.query.text = self.text.clone();
        self.dirty = None;
        self.start_search(ctx);
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        self.textures.clear();
        self.icon_cache.clear();
        self.restart(ctx);
    }

    fn page(&mut self, forward: bool, ctx: &egui::Context) {
        if self.loading {
            return;
        }
        let offset = if forward {
            self.query.offset.saturating_add(PAGE_SIZE)
        } else {
            self.query.offset.saturating_sub(PAGE_SIZE)
        };
        if offset != self.query.offset && offset < self.total {
            self.query.offset = offset;
            self.query.text = self.text.clone();
            self.start_search(ctx);
        }
    }

    fn start_icons(&mut self, ctx: &egui::Context) {
        let revision = self.revision;
        let mut seen = BTreeSet::new();
        let missing: Vec<Entry> = self
            .rows
            .iter()
            .filter(|entry| {
                let key = icons::key(entry);
                seen.insert(key.clone()) && !self.icon_cache.contains_key(&key)
            })
            .cloned()
            .collect();
        if missing.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.icon_rx = Some((revision, rx));
        let ctx = ctx.clone();
        thread::spawn(move || {
            let (work_tx, work_rx) = mpsc::channel();
            for entry in missing {
                let _ = work_tx.send(entry);
            }
            drop(work_tx);
            let work_rx = std::sync::Arc::new(std::sync::Mutex::new(work_rx));
            let mut workers = Vec::new();
            for _ in 0..4 {
                let work_rx = work_rx.clone();
                let tx = tx.clone();
                let ctx = ctx.clone();
                workers.push(thread::spawn(move || {
                    while let Ok(entry) = work_rx.lock().unwrap().recv() {
                        let key = icons::key(&entry);
                        let _ = tx.send((key, icons::fetch(&entry)));
                        ctx.request_repaint();
                    }
                }));
            }
            drop(tx);
            for worker in workers {
                let _ = worker.join();
            }
        });
    }

    fn selected_entries(&self) -> Vec<Entry> {
        self.selection
            .rows
            .iter()
            .filter_map(|index| self.rows.get(*index).cloned())
            .collect()
    }

    fn target(&self) -> Option<&Entry> {
        self.selection.cursor.and_then(|index| self.rows.get(index))
    }

    fn open_entries(&mut self, entries: &[Entry]) {
        // ponytail: cap at 20; raise if batch-open is required.
        for entry in entries.iter().take(20) {
            open_path(&entry.path());
        }
        if entries.len() > 20 {
            self.note = format!("已打开前 20 项，共 {} 项", entries.len());
        }
    }

    fn open_selected(&mut self) {
        let entries = self.selected_entries();
        if entries.is_empty() {
            if let Some(entry) = self.target().cloned() {
                open_path(&entry.path());
            }
            return;
        }
        self.open_entries(&entries);
    }

    fn reveal(&mut self) {
        if let Some(entry) = self.target() {
            reveal_path(&entry.path());
        }
    }

    fn copy_paths(&mut self, ctx: &egui::Context) {
        let paths: Vec<_> = self
            .selected_entries()
            .iter()
            .map(|entry| entry.path().display().to_string())
            .collect();
        if paths.is_empty() {
            return;
        }
        ctx.copy_text(paths.join("\n"));
        self.note = if paths.len() == 1 {
            "已复制完整路径".into()
        } else {
            format!("已复制 {} 条路径", paths.len())
        };
    }

    fn search_folder(&mut self, ctx: &egui::Context) {
        let Some(entry) = self.target().cloned() else {
            return;
        };
        let path = if entry.folder {
            entry.path()
        } else {
            PathBuf::from(entry.directory)
        };
        self.text = format!(
            "\"{}\\\"",
            path.display().to_string().trim_end_matches(['\\', '/'])
        );
        self.query.filter = Filter::All;
        self.query.regex = false;
        self.restart(ctx);
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("search")));
    }

    fn delete_selected(&mut self, ctx: &egui::Context) {
        if self.deleting {
            return;
        }
        let entries = self.selected_entries();
        if entries.is_empty() {
            return;
        }
        self.deleting = true;
        self.note = "正在删除…".into();
        let count = entries.len();
        let (tx, rx) = mpsc::channel();
        self.job_rx = Some(rx);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(Job::Deleted(search::recycle(&entries), count));
            ctx.request_repaint();
        });
    }

    fn export(&mut self, ctx: &egui::Context) {
        if self.exporting {
            return;
        }
        let Some(path) = save_csv() else {
            return;
        };
        self.exporting = true;
        self.note = "正在导出全部结果…".into();
        let query = self.query.clone();
        let (tx, rx) = mpsc::channel();
        self.job_rx = Some(rx);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = search::export(&query, &path).map(|()| path);
            let _ = tx.send(Job::Exported(result));
            ctx.request_repaint();
        });
    }

    fn bookmark(&mut self, remove: bool) {
        if let Some(error) = &self.bookmark_error {
            self.error = Some(format!("书签未写入：{error}；请修复 bookmarks.json 后重启"));
            return;
        }
        let mut query = self.query.clone();
        query.text = self.text.clone();
        query.offset = 0;
        let mut bookmarks = self.bookmarks.clone();
        if remove {
            bookmarks.retain(|saved| saved != &query);
        } else if !bookmarks.contains(&query) {
            bookmarks.push(query);
        }
        match search::save_bookmarks(&bookmarks) {
            Ok(()) => {
                self.bookmarks = bookmarks;
                self.note = if remove {
                    "已移除当前书签"
                } else {
                    "已收藏当前搜索"
                }
                .into();
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    fn apply_bookmark(&mut self, query: Query, ctx: &egui::Context) {
        self.text = query.text.clone();
        self.query = query;
        self.restart(ctx);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let search = self
            .search_rx
            .as_ref()
            .and_then(|(revision, rx)| match rx.try_recv() {
                Ok(result) => Some((*revision, Some(result))),
                Err(TryRecvError::Disconnected) => Some((*revision, None)),
                Err(TryRecvError::Empty) => None,
            });
        if let Some((revision, result)) = search {
            self.search_rx = None;
            if revision == self.revision {
                self.loading = false;
                match result {
                    Some(Ok(page))
                        if self.query.offset > 0
                            && page.rows.is_empty()
                            && self.query.offset >= page.total =>
                    {
                        self.total = page.total;
                        self.query.offset = page.total.saturating_sub(1) / PAGE_SIZE * PAGE_SIZE;
                        self.start_search(ctx);
                    }
                    Some(Ok(page)) => {
                        self.rows = page.rows;
                        self.total = page.total;
                        self.selection.clear();
                        self.start_icons(ctx);
                    }
                    Some(Err(error)) => {
                        self.rows.clear();
                        self.total = 0;
                        self.selection.clear();
                        self.error = Some(format!("{error:#}"));
                    }
                    None => self.error = Some("搜索线程异常退出".into()),
                }
            }
        }

        if let Some((revision, rx)) = self.icon_rx.take()
            && revision == self.revision
        {
            loop {
                match rx.try_recv() {
                    Ok((key, image)) => {
                        if !self.textures.contains_key(&key) {
                            let handle = image.as_ref().map(|image| {
                                let color = ColorImage::from_rgba_unmultiplied(
                                    [image.width() as usize, image.height() as usize],
                                    image.as_raw(),
                                );
                                ctx.load_texture(key.clone(), color, TextureOptions::LINEAR)
                            });
                            self.textures.insert(key.clone(), handle);
                        }
                        self.icon_cache.insert(key, image);
                    }
                    Err(TryRecvError::Empty) => {
                        self.icon_rx = Some((revision, rx));
                        break;
                    }
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }

        let job = self.job_rx.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(job) => Some(Some(job)),
            Err(TryRecvError::Disconnected) => Some(None),
            Err(TryRecvError::Empty) => None,
        });
        if let Some(job) = job {
            self.job_rx = None;
            self.deleting = false;
            self.exporting = false;
            match job {
                Some(Job::Deleted(Ok(true), count)) => {
                    self.note = format!("已移到回收站：{count} 项");
                    self.start_search(ctx);
                }
                Some(Job::Deleted(Ok(false), _)) => self.note.clear(),
                Some(Job::Deleted(Err(error), _)) => self.error = Some(format!("{error:#}")),
                Some(Job::Exported(Ok(path))) => {
                    self.note = format!("已导出：{}", path.display());
                }
                Some(Job::Exported(Err(error))) => self.error = Some(format!("{error:#}")),
                None => self.error = Some("后台任务异常退出".into()),
            }
        }

        if self
            .dirty
            .is_some_and(|time| time.elapsed() >= Duration::from_millis(120))
        {
            self.restart(ctx);
        }
        if self.dirty.is_some()
            || self.search_rx.is_some()
            || self.icon_rx.is_some()
            || self.job_rx.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let search = egui::Id::new("search");
        if ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::COMMAND, egui::Key::F)
                || input.consume_key(egui::Modifiers::COMMAND, egui::Key::L)
        }) {
            ctx.memory_mut(|memory| memory.request_focus(search));
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F5)) {
            self.refresh(ctx);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::E)) {
            self.export(ctx);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::D)) {
            self.bookmark(false);
        }
        if ctx.memory(|memory| memory.has_focus(search)) {
            return;
        }
        if self.from_search {
            self.from_search = false;
            ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            });
        } else if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            self.open_selected();
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::ALT, egui::Key::Enter)) {
            self.reveal();
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::C)) {
            self.copy_paths(ctx);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::A)) {
            self.selection.all(self.rows.len());
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Delete)) {
            self.delete_selected(ctx);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::PageUp)) {
            self.page(false, ctx);
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::PageDown)) {
            self.page(true, ctx);
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.menu = None;
        }
        let moved = if ctx
            .input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp))
        {
            self.selection.nudge(-1, self.rows.len());
            true
        } else if ctx
            .input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown))
        {
            self.selection.nudge(1, self.rows.len());
            true
        } else if ctx
            .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp))
        {
            self.selection.move_to(-1, self.rows.len());
            true
        } else if ctx
            .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
        {
            self.selection.move_to(1, self.rows.len());
            true
        } else {
            false
        };
        self.follow |= moved;
    }

    fn menus(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let open = Rc::new(Cell::new(self.menu));
        let mut over = false;
        let bar = egui::MenuBar::new()
            .style(|style: &mut egui::Style| {
                style.spacing.button_padding = egui::vec2(6.0, 1.0);
                style.spacing.item_spacing.x = 2.0;
                style.spacing.interact_size.y = 18.0;
                style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
                style.visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
                style.visuals.widgets.open.bg_stroke = egui::Stroke::NONE;
                style.visuals.widgets.hovered.expansion = 0.0;
            })
            .ui(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.spacing_mut().button_padding = egui::vec2(4.0, 0.0);
                over |= hover_menu(ui, "file", "文件", &open, |ui| {
                    if ui.button("打开").clicked() {
                        self.open_selected();
                        open.set(None);
                        ui.close();
                    }
                    if ui.button("打开所在目录").clicked() {
                        self.reveal();
                        open.set(None);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("导出全部结果 CSV…").clicked() {
                        self.export(&ctx);
                        open.set(None);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("退出").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                over |= hover_menu(ui, "edit", "编辑", &open, |ui| {
                    if ui.button("复制完整路径").clicked() {
                        self.copy_paths(&ctx);
                        open.set(None);
                        ui.close();
                    }
                    if ui.button("搜索此目录").clicked() {
                        self.search_folder(&ctx);
                        open.set(None);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("定位搜索框").clicked() {
                        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("search")));
                        open.set(None);
                        ui.close();
                    }
                });
                over |= hover_menu(ui, "search", "搜索", &open, |ui| {
                    let mut changed = false;
                    changed |= menu_check(ui, "区分大小写", "Ctrl+I", &mut self.query.match_case);
                    changed |= menu_check(ui, "匹配完整路径", "", &mut self.query.match_path);
                    changed |= menu_check(ui, "全字匹配", "", &mut self.query.whole_word);
                    changed |= menu_check(ui, "正则表达式", "", &mut self.query.regex);
                    ui.separator();
                    for item in Filter::ALL {
                        if menu_mark(ui, item.label(), self.query.filter == item) {
                            self.query.filter = item;
                            changed = true;
                            open.set(None);
                            ui.close();
                        }
                    }
                    if changed {
                        self.restart(&ctx);
                    }
                });
                over |= hover_menu(ui, "view", "查看", &open, |ui| {
                    if ui.button("刷新").clicked() {
                        self.refresh(&ctx);
                        open.set(None);
                        ui.close();
                    }
                    if ui.button("切换深色／浅色").clicked() {
                        self.dark = !self.dark;
                        style(&ctx, self.dark);
                        open.set(None);
                        ui.close();
                    }
                });
                over |= hover_menu(ui, "bookmarks", "书签", &open, |ui| {
                    if ui.button("收藏当前搜索").clicked() {
                        self.bookmark(false);
                        open.set(None);
                        ui.close();
                    }
                    if ui.button("移除当前书签").clicked() {
                        self.bookmark(true);
                        open.set(None);
                        ui.close();
                    }
                    ui.separator();
                    for query in self.bookmarks.clone() {
                        let label = format!(
                            "{} · {}",
                            query.filter.label(),
                            if query.text.is_empty() {
                                "全部对象"
                            } else {
                                &query.text
                            }
                        );
                        if ui.button(label).clicked() {
                            self.apply_bookmark(query, &ctx);
                            open.set(None);
                            ui.close();
                        }
                    }
                });
                over |= hover_menu(ui, "help", "帮助", &open, |ui| {
                    ui.hyperlink_to(
                        "Everything 搜索语法",
                        "https://www.voidtools.com/support/everything/searching/",
                    );
                });
            });
        // Stay open while the pointer is on the bar (the gaps between the titles
        // included) or inside the open menu.
        over |= ctx
            .pointer_hover_pos()
            .is_some_and(|pos| bar.response.rect.contains(pos));
        if !over {
            open.set(None);
        }
        self.menu = open.get();
    }

    fn search_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(180));
            ui.visuals_mut().selection.stroke = stroke;
            ui.visuals_mut().widgets.inactive.bg_stroke = stroke;
            ui.visuals_mut().widgets.hovered.bg_stroke = stroke;
            ui.visuals_mut().widgets.active.bg_stroke = stroke;
            let width = ui.available_width();
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .id(egui::Id::new("search"))
                    .desired_width(width)
                    .margin(egui::Margin {
                        left: 6,
                        right: 24,
                        top: 5,
                        bottom: 5,
                    }),
            );
            let center = response.rect.right_center() - egui::vec2(12.0, 0.0);
            let stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(96));
            ui.painter()
                .circle_stroke(center + egui::vec2(-2.0, -1.0), 4.5, stroke);
            ui.painter().line_segment(
                [center + egui::vec2(1.5, 2.2), center + egui::vec2(4.8, 5.5)],
                stroke,
            );
            if response.changed() {
                self.dirty = Some(Instant::now());
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                self.from_search = true;
                if !self.rows.is_empty() {
                    self.selection.select(0);
                }
            }
        });
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        // Keep partially scrolled rows from bleeding into the fixed header.
        ui.visuals_mut().clip_rect_margin = 0.0;
        // Idle column rules are full-height; Everything only hints them in the header.
        ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
        let event = Rc::new(Cell::new(None::<Click>));
        let n = self.rows.len();
        let selection = self.selection.clone();
        let table_rect = ui.available_rect_before_wrap();
        // The header reaches the panel's right edge, including the scroll bar gutter.
        let header_rect =
            egui::Rect::from_min_size(table_rect.min, egui::vec2(table_rect.width(), 24.0));
        ui.painter()
            .rect_filled(header_rect, 0.0, ui.visuals().faint_bg_color);
        let available = ui.available_width() - ui.spacing().scroll.allocated_width();
        let mut widths = fit_columns(self.cols, available);
        let mut table = TableBuilder::new(ui)
            .id_salt("cols")
            .striped(false)
            .resizable(false)
            .vscroll(true)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, false])
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for width in widths {
            table = table.column(Column::exact(width));
        }
        if self.follow
            && let Some(row) = self.selection.cursor
        {
            table = table.scroll_to_row(row, None);
            self.follow = false;
        }
        table
            .header(24.0, |mut header| {
                for (index, title) in ["名称", "路径", "类型", "大小", "修改时间"]
                    .into_iter()
                    .enumerate()
                {
                    let sorted = self.query.sort_column == index;
                    let click = event.clone();
                    header.col(|ui| {
                        let rect = ui.max_rect();
                        let resp = ui.interact(rect, ui.id().with("sort"), egui::Sense::click());
                        let bg = if sorted && !ui.visuals().dark_mode {
                            egui::Color32::from_rgb(204, 232, 255)
                        } else if resp.hovered() {
                            ui.visuals().widgets.hovered.weak_bg_fill
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(rect, 0.0, bg);
                        let (align, pos) = if index == 3 {
                            (
                                egui::Align2::RIGHT_CENTER,
                                rect.right_center() - egui::vec2(6.0, 0.0),
                            )
                        } else {
                            (
                                egui::Align2::LEFT_CENTER,
                                rect.left_center() + egui::vec2(6.0, 0.0),
                            )
                        };
                        ui.painter().text(
                            pos,
                            align,
                            title,
                            egui::FontId::new(12.0, egui::FontFamily::Proportional),
                            ui.visuals().text_color(),
                        );
                        if sorted {
                            let c = rect.center();
                            let dy = if self.query.descending { 1.5 } else { -1.5 };
                            let tip = c + egui::vec2(0.0, dy);
                            let stroke = egui::Stroke::new(1.0_f32, ui.visuals().text_color());
                            ui.painter()
                                .line_segment([c + egui::vec2(-5.0, -dy), tip], stroke);
                            ui.painter()
                                .line_segment([tip, c + egui::vec2(5.0, -dy)], stroke);
                        }
                        let line = egui::Stroke::new(
                            1.0_f32,
                            if ui.visuals().dark_mode {
                                egui::Color32::from_gray(60)
                            } else {
                                egui::Color32::from_rgb(226, 226, 226)
                            },
                        );
                        ui.painter().hline(rect.x_range(), rect.bottom(), line);
                        if index + 1 < 5 {
                            ui.painter().vline(rect.right(), rect.y_range(), line);
                        }
                        if resp.clicked() {
                            click.set(Some(Click::Sort(index)));
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, n, |mut row| {
                    let index = row.index();
                    let selected = selection.rows.contains(&index);
                    for column in 0..5 {
                        let click = event.clone();
                        row.col(|ui| {
                            self.draw_cell(ui, index, column, selected, &click);
                        });
                    }
                });
            });
        self.apply_click(event.get(), ui.ctx());
        let mut x = table_rect.left();
        for (index, width) in widths.into_iter().take(4).enumerate() {
            x += width;
            let handle = egui::Rect::from_x_y_ranges((x - 3.0)..=(x + 3.0), table_rect.y_range());
            let resp = ui.interact(
                handle,
                ui.id().with("col").with(index),
                egui::Sense::click_and_drag(),
            );
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
            }
            if resp.dragged() {
                drag_column(&mut widths, index, resp.drag_delta().x);
                self.cols = widths;
            }
        }
    }

    fn draw_cell(
        &self,
        ui: &mut egui::Ui,
        index: usize,
        column: usize,
        selected: bool,
        click: &Rc<Cell<Option<Click>>>,
    ) {
        let rect = ui.max_rect();
        if selected {
            ui.painter()
                .rect_filled(rect, 0.0, egui::Color32::from_rgb(168, 206, 242));
        }
        let entry = &self.rows[index];
        let text = match column {
            0 => entry.name.clone(),
            1 => entry.directory.clone(),
            2 => entry.kind(),
            3 => size_text(entry.size),
            _ => date_text(&entry.modified),
        };
        let color = ui.visuals().text_color();
        let text = egui::RichText::new(text).color(color);
        ui.spacing_mut().item_spacing.x = 4.0;
        if column == 3 {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(text)
                        .truncate()
                        .sense(egui::Sense::hover()),
                );
            });
        } else {
            ui.horizontal(|ui| {
                ui.add_space(if column == 0 { 2.0 } else { 4.0 });
                if column == 0 {
                    if let Some(Some(texture)) = self.textures.get(&icons::key(entry)) {
                        ui.image((texture.id(), egui::vec2(16.0, 16.0)));
                    } else {
                        ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                    }
                }
                ui.add(
                    egui::Label::new(text)
                        .truncate()
                        .sense(egui::Sense::hover()),
                );
            });
        }
        // Text is painted first. The hit target must be last or it never sees the click.
        let response = ui.interact(rect, ui.id().with("hit"), egui::Sense::click());
        if response.clicked() || response.secondary_clicked() {
            ui.memory_mut(|memory| memory.surrender_focus(egui::Id::new("search")));
        }
        if response.double_clicked() {
            click.set(Some(Click::Row {
                row: index,
                ctrl: false,
                shift: false,
                double: true,
                right: false,
            }));
        } else if response.secondary_clicked() {
            click.set(Some(Click::Row {
                row: index,
                ctrl: false,
                shift: false,
                double: false,
                right: true,
            }));
        } else if response.clicked() {
            let mods = ui.input(|input| input.modifiers);
            click.set(Some(Click::Row {
                row: index,
                ctrl: mods.command,
                shift: mods.shift,
                double: false,
                right: false,
            }));
        }
        response.context_menu(|ui| {
            if ui.button("打开").clicked() {
                click.set(Some(Click::Open));
                ui.close();
            }
            if ui.button("打开所在目录").clicked() {
                click.set(Some(Click::Reveal));
                ui.close();
            }
            if ui.button("复制完整路径").clicked() {
                click.set(Some(Click::Copy));
                ui.close();
            }
            if ui.button("搜索此目录").clicked() {
                click.set(Some(Click::Folder));
                ui.close();
            }
            ui.separator();
            if ui.button("删除到回收站").clicked() {
                click.set(Some(Click::Delete));
                ui.close();
            }
        });
    }

    fn apply_click(&mut self, click: Option<Click>, ctx: &egui::Context) {
        match click {
            Some(Click::Row {
                row,
                ctrl,
                shift,
                double,
                right,
            }) => {
                if right {
                    if !self.selection.rows.contains(&row) {
                        self.selection.select(row);
                    } else {
                        self.selection.cursor = Some(row);
                    }
                } else if ctrl {
                    self.selection.toggle(row);
                } else if shift {
                    self.selection.extend(row);
                } else {
                    self.selection.select(row);
                }
                if double && let Some(entry) = self.rows.get(row).cloned() {
                    open_path(&entry.path());
                }
            }
            Some(Click::Open) => self.open_selected(),
            Some(Click::Reveal) => self.reveal(),
            Some(Click::Copy) => self.copy_paths(ctx),
            Some(Click::Delete) => self.delete_selected(ctx),
            Some(Click::Folder) => self.search_folder(ctx),
            Some(Click::Sort(column)) => {
                if self.query.sort_column == column {
                    self.query.descending = !self.query.descending;
                } else {
                    self.query.sort_column = column;
                    self.query.descending = false;
                }
                self.restart(ctx);
            }
            None => {}
        }
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let shown = self.rows.len();
        let text = if self.loading {
            "正在搜索…".into()
        } else if self.exporting {
            "正在导出全部结果…".into()
        } else if self.deleting {
            "正在删除…".into()
        } else if !self.note.is_empty() {
            self.note.clone()
        } else if self.selection.rows.is_empty() {
            format!("{} 个对象", grouped(self.total))
        } else {
            format!(
                "{} 个对象，已选择 {}",
                grouped(self.total),
                grouped(self.selection.rows.len())
            )
        };
        ui.style_mut().visuals.override_text_color = Some(egui::Color32::BLACK);
        ui.horizontal(|ui| {
            ui.label(text);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let next =
                    !self.loading && self.query.offset.saturating_add(PAGE_SIZE) < self.total;
                let prev = !self.loading && self.query.offset > 0;
                let plain = |label: &str| egui::Button::new(label).frame(false);
                if ui.add_enabled(next, plain("下一页")).clicked() {
                    self.page(true, &ctx);
                }
                if ui.add_enabled(prev, plain("上一页")).clicked() {
                    self.page(false, &ctx);
                }
                ui.label(format!(
                    "{}–{} / {}",
                    grouped(if shown == 0 { 0 } else { self.query.offset + 1 }),
                    grouped(self.query.offset + shown),
                    grouped(self.total)
                ));
            });
        });
    }
}

impl eframe::App for Everything {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll(ctx);
        self.keys(ctx);
        let visuals = ctx.style().visuals.clone();
        let bar = egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(2, 0))
            .fill(visuals.panel_fill);
        egui::TopBottomPanel::top("menu")
            .frame(bar)
            .show(ctx, |ui| self.menus(ui));
        egui::TopBottomPanel::top("search")
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(3, 3))
                    .fill(visuals.panel_fill),
            )
            .show_separator_line(false)
            .show(ctx, |ui| self.search_bar(ui));
        if let Some(error) = self.error.clone().or(self.bookmark_error.clone()) {
            egui::TopBottomPanel::top("error")
                .frame(
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(4, 0))
                        .fill(visuals.panel_fill),
                )
                .show(ctx, |ui| {
                    ui.colored_label(egui::Color32::from_rgb(180, 35, 24), error);
                });
        }
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(6, 1))
                    .fill(egui::Color32::from_rgb(228, 228, 228)),
            )
            .show(ctx, |ui| self.status(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(visuals.panel_fill))
            .show(ctx, |ui| self.table(ui));
    }
}

fn style(ctx: &egui::Context, dark: bool) {
    let mut fonts = egui::FontDefinitions::default();
    // Everything leaves its font blank, so it uses Microsoft YaHei UI for Latin too.
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\msyh.ttc") {
        let mut data = egui::FontData::from_owned(bytes);
        data.index = 1;
        fonts.font_data.insert("yahei".into(), data.into());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, "yahei".into());
        }
    }
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        fonts
            .font_data
            .insert("segoe".into(), egui::FontData::from_owned(bytes).into());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("segoe".into());
        }
    }
    ctx.set_fonts(fonts);

    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let bg = if dark {
        egui::Color32::from_rgb(32, 32, 32)
    } else {
        egui::Color32::WHITE
    };
    let fg = if dark {
        egui::Color32::from_gray(230)
    } else {
        egui::Color32::BLACK
    };
    let border = if dark {
        egui::Color32::from_gray(90)
    } else {
        egui::Color32::from_rgb(171, 173, 179)
    };
    let header = if dark {
        egui::Color32::from_rgb(45, 45, 45)
    } else {
        egui::Color32::from_rgb(240, 240, 240)
    };
    let hover = if dark {
        egui::Color32::from_rgb(50, 70, 90)
    } else {
        egui::Color32::from_rgb(229, 243, 255)
    };
    visuals.dark_mode = dark;
    visuals.panel_fill = bg;
    visuals.window_fill = bg;
    visuals.extreme_bg_color = bg;
    visuals.faint_bg_color = header;
    visuals.code_bg_color = bg;
    visuals.window_corner_radius = egui::CornerRadius::ZERO;
    visuals.menu_corner_radius = egui::CornerRadius::ZERO;
    visuals.striped = false;
    visuals.selection.bg_fill = egui::Color32::from_rgb(0, 120, 215);
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0, 120, 215));
    visuals.window_stroke = egui::Stroke::NONE;
    visuals.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(217, 217, 217));
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::ZERO;
        widget.expansion = 0.0;
        widget.fg_stroke = egui::Stroke::new(1.0_f32, fg);
    }
    visuals.widgets.inactive.weak_bg_fill = bg;
    // The scroll handle is painted with these fills (see `foreground_color` below).
    visuals.widgets.inactive.bg_fill = if dark {
        egui::Color32::from_gray(90)
    } else {
        egui::Color32::from_gray(205)
    };
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, border);
    visuals.widgets.hovered.weak_bg_fill = hover;
    visuals.widgets.hovered.bg_fill = if dark {
        egui::Color32::from_gray(115)
    } else {
        egui::Color32::from_gray(185)
    };
    visuals.widgets.hovered.bg_stroke = visuals.selection.stroke;
    visuals.widgets.active.bg_fill = if dark {
        egui::Color32::from_gray(140)
    } else {
        egui::Color32::from_gray(160)
    };
    visuals.widgets.active.weak_bg_fill = hover;
    visuals.widgets.active.bg_stroke = visuals.selection.stroke;
    visuals.widgets.open.weak_bg_fill = bg;
    if dark {
        visuals.widgets.noninteractive.bg_stroke =
            egui::Stroke::new(1.0_f32, egui::Color32::from_gray(60));
    }
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        // 9 pt, the Windows menu / Everything UI size at 96 DPI.
        let body = egui::FontId::new(12.0, egui::FontFamily::Proportional);
        style
            .text_styles
            .insert(egui::TextStyle::Body, body.clone());
        style
            .text_styles
            .insert(egui::TextStyle::Button, body.clone());
        style
            .text_styles
            .insert(egui::TextStyle::Small, body.clone());
        style.text_styles.insert(egui::TextStyle::Heading, body);
        style.spacing.item_spacing = egui::vec2(4.0, 1.0);
        style.spacing.button_padding = egui::vec2(6.0, 1.0);
        style.spacing.interact_size.y = 20.0;
        style.spacing.menu_margin = egui::Margin::symmetric(2, 2);
        style.spacing.window_margin = egui::Margin::ZERO;
        // egui paints the scroll handle with `fg_stroke` (the near-black text color
        // here) while `foreground_color` is set, which read as a black band down the
        // right edge of the list. Use the light gray widget fills instead.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.foreground_color = false;
        style.spacing.scroll.bar_width = 12.0;
        style.spacing.scroll.bar_inner_margin = 0.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
    });
}

fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn save_csv() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        use std::{ffi::OsString, os::windows::ffi::OsStrExt};
        use windows::Win32::System::Com::{
            CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize,
        };
        use windows::Win32::UI::Shell::{
            FOS_OVERWRITEPROMPT, FileSaveDialog, IFileSaveDialog, SIGDN_FILESYSPATH,
        };
        use windows::core::{PCWSTR, w};

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let path = unsafe {
                let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let uninit = hr.is_ok();
                let path = (|| {
                    let dialog: IFileSaveDialog =
                        CoCreateInstance(&FileSaveDialog, None, CLSCTX_ALL).ok()?;
                    let name: Vec<u16> = OsString::from("everything-results.csv")
                        .encode_wide()
                        .chain([0])
                        .collect();
                    dialog.SetFileName(PCWSTR(name.as_ptr())).ok()?;
                    dialog.SetDefaultExtension(w!("csv")).ok()?;
                    dialog.SetOptions(FOS_OVERWRITEPROMPT).ok()?;
                    dialog.Show(None).ok()?;
                    let item = dialog.GetResult().ok()?;
                    let wide = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
                    let len = (0..).take_while(|i| *wide.0.add(*i) != 0).count();
                    let path = OsString::from_wide(std::slice::from_raw_parts(wide.0, len)).into();
                    CoTaskMemFree(Some(wide.0.cast()));
                    Some(path)
                })();
                if uninit {
                    CoUninitialize();
                }
                path
            };
            let _ = tx.send(path);
        });
        rx.recv().ok().flatten()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn hover_menu(
    ui: &mut egui::Ui,
    id: &'static str,
    label: &str,
    open: &Rc<Cell<Option<&'static str>>>,
    add: impl FnOnce(&mut egui::Ui),
) -> bool {
    let response = ui.add(egui::Button::new(label).frame(false));
    let mut over = response.hovered();
    if over {
        open.set(Some(id));
    }
    if open.get() == Some(id)
        && let Some(inner) = egui::Popup::menu(&response).open(true).show(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(8.0, 2.0);
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.set_min_width(220.0);
            add(ui);
        })
    {
        // Hovering an item keeps the hover on that item, never on the popup itself.
        let rect = inner.response.rect;
        over |= ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|pos| rect.contains(pos));
    }
    ui.add_space(8.0);
    over
}

fn menu_row(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let width = ui.available_width().max(220.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
    ui.painter().text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        font.clone(),
        ui.visuals().text_color(),
    );
    if !shortcut.is_empty() {
        ui.painter().text(
            rect.right_center() - egui::vec2(8.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            shortcut,
            font,
            ui.visuals().weak_text_color(),
        );
    }
    response.clicked()
}

fn menu_check(ui: &mut egui::Ui, label: &str, shortcut: &str, value: &mut bool) -> bool {
    let mark = if *value { "√  " } else { "    " };
    if menu_row(ui, &format!("{mark}{label}"), shortcut) {
        *value = !*value;
        return true;
    }
    false
}

fn menu_mark(ui: &mut egui::Ui, label: &str, on: bool) -> bool {
    let mark = if on { "●  " } else { "    " };
    menu_row(ui, &format!("{mark}{label}"), "")
}

fn size_text(size: Option<u64>) -> String {
    let Some(size) = size else {
        return String::new();
    };
    if size < 1024 {
        return search::human_size(Some(size));
    }
    format!("{} KB", grouped((size / 1024) as usize))
}

fn date_text(raw: &str) -> String {
    let text = raw.trim().replace('T', " ");
    let mut parts = text.split([' ', '-', ':']);
    let Some(year) = parts.next() else {
        return text;
    };
    let Ok(month) = parts.next().unwrap_or("").parse::<u32>() else {
        return text;
    };
    let Ok(day) = parts.next().unwrap_or("").parse::<u32>() else {
        return text;
    };
    let Ok(hour) = parts.next().unwrap_or("").parse::<u32>() else {
        return format!("{year}/{month}/{day}");
    };
    let Ok(minute) = parts.next().unwrap_or("").parse::<u32>() else {
        return format!("{year}/{month}/{day}");
    };
    format!("{year}/{month}/{day} {hour:02}:{minute:02}")
}

const COLUMN_MIN: [f32; 5] = [80.0, 120.0, 48.0, 56.0, 120.0];

fn fit_columns(mut width: [f32; 5], available: f32) -> [f32; 5] {
    // The path column takes the slack, then the wide columns give back what is missing.
    let fixed = width[0] + width[2] + width[3] + width[4];
    width[1] = (available - fixed).max(COLUMN_MIN[1]);
    let mut over = (width.iter().sum::<f32>() - available).max(0.0);
    for index in [4, 3, 2, 0] {
        let give = (width[index] - COLUMN_MIN[index]).max(0.0).min(over);
        width[index] -= give;
        over -= give;
    }
    width
}

fn drag_column(widths: &mut [f32; 5], index: usize, dx: f32) {
    // Shift the divider between `index` and `index + 1`, within both minimums.
    let dx = dx.clamp(
        COLUMN_MIN[index] - widths[index],
        widths[index + 1] - COLUMN_MIN[index + 1],
    );
    widths[index] += dx;
    widths[index + 1] -= dx;
}

fn open_path(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("cmd")
            .args(["/C", "start", "", &path.display().to_string()])
            .creation_flags(0x0800_0000)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }
}

fn reveal_path(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .creation_flags(0x0800_0000)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let parent = path.parent().unwrap_or(path);
        let _ = Command::new("xdg-open").arg(parent).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::{Selection, drag_column, fit_columns};

    #[test]
    fn everything_text_format() {
        assert_eq!(super::size_text(Some(4_294_967_296)), "4,194,304 KB");
        assert_eq!(super::size_text(Some(66)), "66 B");
        assert_eq!(super::date_text("2026-10-05T18:55:01"), "2026/10/5 18:55");
    }

    #[test]
    fn ctrl_and_shift_selection() {
        let mut selection = Selection::default();
        selection.select(1);
        selection.toggle(3);
        assert_eq!(selection.rows.iter().copied().collect::<Vec<_>>(), [1, 3]);
        selection.extend(4);
        assert_eq!(selection.cursor, Some(4));
        assert_eq!(selection.rows.iter().copied().collect::<Vec<_>>(), [3, 4]);
        selection.all(5);
        assert_eq!(selection.rows.len(), 5);
        assert_eq!(selection.cursor, Some(4));
        selection.move_to(-3, 5);
        assert_eq!(selection.cursor, Some(1));
        assert_eq!(selection.rows.iter().copied().collect::<Vec<_>>(), [1]);
        selection.nudge(2, 5);
        assert_eq!(
            selection.rows.iter().copied().collect::<Vec<_>>(),
            [1, 2, 3]
        );
        selection.clear();
        assert!(selection.rows.is_empty());
    }

    #[test]
    fn columns_fill_and_resize() {
        let widths = [280.0, 120.0, 72.0, 84.0, 148.0];
        for available in [424.0, 600.0, 1200.0, 2400.0] {
            let before = fit_columns(widths, available);
            assert_eq!(before.iter().sum::<f32>(), available);
            for index in 0..4 {
                for dx in [-10_000.0_f32, -16.0, 16.0, 10_000.0] {
                    let mut after = before;
                    drag_column(&mut after, index, dx);
                    assert_eq!(fit_columns(after, available), after);
                    let delta = dx.clamp(
                        super::COLUMN_MIN[index] - before[index],
                        before[index + 1] - super::COLUMN_MIN[index + 1],
                    );
                    assert_eq!(after[index], before[index] + delta);
                    assert_eq!(after[index + 1], before[index + 1] - delta);
                    assert_eq!(after.iter().sum::<f32>(), available);
                    for column in 0..5 {
                        assert!(after[column] >= super::COLUMN_MIN[column]);
                        if column != index && column != index + 1 {
                            assert_eq!(after[column], before[column]);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn thousands_separator() {
        assert_eq!(super::grouped(0), "0");
        assert_eq!(super::grouped(999), "999");
        assert_eq!(super::grouped(3_332_278), "3,332,278");
    }
}
