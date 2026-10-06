#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod files;
mod icons;
mod search;

use std::{path::PathBuf, rc::Rc, time::Duration};

use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Root, Selectable, Sizable, Theme, ThemeMode,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{PopupMenu, PopupMenuItem},
    table::{ColumnSort, Table, TableEvent, TableState},
    v_flex,
};

use files::{Files, FilesEvent};
use search::{Entry, Filter, PAGE_SIZE, Query};

actions!(
    everything,
    [
        Open,
        Reveal,
        CopyPath,
        SearchFolder,
        Refresh,
        FocusSearch,
        Export,
        Quit,
        MatchCase,
        MatchPath,
        WholeWord,
        Regex,
        ToggleTheme,
        AddBookmark,
        RemoveBookmark,
        PreviousPage,
        NextPage,
        Delete,
        SelectAll,
    ]
);

struct Everything {
    focus: FocusHandle,
    input: Entity<InputState>,
    table: Entity<TableState<Files>>,
    query: Query,
    total: usize,
    loading: bool,
    exporting: bool,
    revision: u64,
    pending: Option<Task<()>>,
    error: Option<String>,
    note: String,
    bookmarks: Vec<Query>,
    bookmark_error: Option<String>,
    click: Option<Modifiers>,
    keep_selection: bool,
    deleting: bool,
    menu: Option<(&'static str, Entity<PopupMenu>)>,
    menu_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl Everything {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("搜索文件和文件夹 · 支持 ext:pdf、path:、通配符及 Everything 搜索语法")
        });
        let table = cx.new(|cx| TableState::new(Files::new(), window, cx).col_movable(false));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::Change => this.restart(cx),
                InputEvent::PressEnter { .. }
                    if !this.table.read(cx).delegate().rows.is_empty() =>
                {
                    this.table
                        .update(cx, |table, cx| table.set_selected_row(0, cx));
                    this.table.focus_handle(cx).focus(window);
                }
                _ => {}
            }),
            cx.subscribe_in(&table, window, |this, _, event, window, cx| match event {
                TableEvent::DoubleClickedRow(index) => {
                    if let Some(entry) = this.table.read(cx).delegate().rows.get(*index) {
                        cx.open_with_system(&entry.path());
                    }
                }
                TableEvent::SelectRow(index) => {
                    let click = this.click.take();
                    let keep = this.keep_selection;
                    let shift = window.modifiers().shift;
                    let index = *index;
                    this.table.update(cx, |table, cx| {
                        if keep {
                            table.delegate_mut().cursor = Some(index);
                            return;
                        }
                        let files = table.delegate_mut();
                        if let Some(modifiers) = click {
                            if modifiers.control {
                                files.toggle(index);
                            } else if modifiers.shift {
                                files.extend(index);
                            } else {
                                files.select(index);
                            }
                        } else if shift {
                            files.extend(index);
                        } else if !files.is_selected(index) {
                            files.select(index);
                        }
                        files.cursor = Some(index);
                        if !table.delegate().is_selected(index) {
                            table.clear_selection(cx);
                        }
                        cx.notify();
                    });
                }
                TableEvent::ColumnWidthsChanged(widths) => {
                    this.table.update(cx, |table, _| {
                        for (column, width) in table.delegate_mut().columns.iter_mut().zip(widths) {
                            column.width = *width;
                        }
                    });
                }
                _ => {}
            }),
            cx.subscribe(&table, |this, _, event, cx| match event {
                FilesEvent::Sort(column, descending) => {
                    this.query.sort_column = *column;
                    this.query.descending = *descending;
                    this.restart(cx);
                }
                FilesEvent::ContextRow(_) => cx.notify(),
            }),
        ];
        let (bookmarks, bookmark_error) = match search::load_bookmarks() {
            Ok(bookmarks) => (bookmarks, None),
            Err(error) => (Vec::new(), Some(format!("{error:#}"))),
        };
        input.update(cx, |input, cx| input.focus(window, cx));
        let mut view = Self {
            focus: cx.focus_handle(),
            input,
            table,
            query: Query::default(),
            total: 0,
            loading: false,
            exporting: false,
            revision: 0,
            pending: None,
            error: bookmark_error.clone(),
            note: String::new(),
            bookmarks,
            bookmark_error,
            click: None,
            keep_selection: false,
            deleting: false,
            menu: None,
            menu_subscription: None,
            _subscriptions: subscriptions,
        };
        view.run_search(cx);
        view
    }

    fn menu(
        &self,
        id: &'static str,
        label: &'static str,
        build: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let menu = self
            .menu
            .as_ref()
            .filter(|(name, _)| *name == id)
            .map(|(_, menu)| menu.clone());
        let show = Rc::new(cx.listener(move |this, hovered: &bool, window, cx| {
            if !*hovered || this.menu.as_ref().is_some_and(|(name, _)| *name == id) {
                return;
            }
            let focus = this.focus.clone();
            let menu = PopupMenu::build(window, cx, |menu, window, cx| {
                build(menu.action_context(focus), window, cx)
            });
            this.click = None;
            this.menu_subscription =
                Some(cx.subscribe(&menu, |this, menu, _: &DismissEvent, cx| {
                    if this
                        .menu
                        .as_ref()
                        .is_some_and(|(_, active)| *active == menu)
                    {
                        this.menu = None;
                        this.menu_subscription = None;
                        #[cfg(debug_assertions)]
                        eprintln!("menu:closed");
                        cx.notify();
                    }
                }));
            this.menu = Some((id, menu));
            #[cfg(debug_assertions)]
            eprintln!("menu:{id}");
            cx.notify();
        }));
        let click = show.clone();
        div()
            .id(id)
            .relative()
            .on_hover(move |hovered, window, cx| show(hovered, window, cx))
            .child(
                Button::new("menu")
                    .small()
                    .ghost()
                    .label(label)
                    .selected(menu.is_some())
                    .on_click(move |_, window, cx| click(&true, window, cx)),
            )
            .when_some(menu, |this, menu| {
                this.child(deferred(
                    anchored()
                        .anchor(Corner::TopLeft)
                        .snap_to_window_with_margin(px(8.))
                        .child(div().occlude().top_1().child(menu)),
                ))
            })
            .into_any_element()
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        self.query.offset = 0;
        self.query.text = self.input.read(cx).value().to_string();
        self.run_search(cx);
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        let revision = self.revision;
        let query = self.query.clone();
        self.loading = true;
        self.error = None;
        self.note.clear();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().rows.clear();
            table.delegate_mut().clear_selection();
            table.clear_selection(cx);
        });
        let icons = self.table.read(cx).delegate().icons.clone();
        let timer = cx.background_executor().timer(Duration::from_millis(120));
        self.pending = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let result = cx
                .background_executor()
                .spawn(async move { search::search(&query) })
                .await;
            let Ok(Some(rows)) = this.update(cx, |this, cx| {
                if revision != this.revision {
                    return None;
                }
                this.loading = false;
                let rows = match result {
                    Ok(page) => {
                        this.total = page.total;
                        if this.total == 0 {
                            this.query.offset = 0;
                        } else if this.query.offset >= this.total {
                            this.query.offset = (this.total - 1) / PAGE_SIZE * PAGE_SIZE;
                            this.run_search(cx);
                            return None;
                        }
                        let rows = page.rows.clone();
                        this.table.update(cx, |table, cx| {
                            table.delegate_mut().rows = page.rows;
                            table.scroll_to_row(0, cx);
                            cx.notify();
                        });
                        Some(rows)
                    }
                    Err(error) => {
                        this.total = 0;
                        this.error = Some(format!("{error:#}"));
                        None
                    }
                };
                cx.notify();
                rows
            }) else {
                return;
            };
            if rows.is_empty() {
                return;
            }
            let icons = cx
                .background_executor()
                .spawn(async move { icons::load(&rows, icons) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if revision == this.revision {
                    this.table.update(cx, |table, cx| {
                        table.delegate_mut().icons = icons;
                        cx.notify();
                    });
                }
            });
        }));
        cx.notify();
    }

    fn refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.table
            .update(cx, |table, _| table.delegate_mut().icons.clear());
        self.restart(cx);
    }

    fn target(&self, cx: &App) -> Option<Entry> {
        let table = self.table.read(cx);
        let files = table.delegate();
        let index = files.clicked.or(files.anchor).or(table.selected_row())?;
        files.rows.get(index).cloned()
    }

    fn open(&mut self, _: &Open, _: &mut Window, cx: &mut Context<Self>) {
        let entries = self.table.read(cx).delegate().selected_entries();
        if entries.is_empty() {
            if let Some(entry) = self.target(cx) {
                cx.open_with_system(&entry.path());
            }
            return;
        }
        for entry in entries {
            cx.open_with_system(&entry.path());
        }
    }

    fn reveal(&mut self, _: &Reveal, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.target(cx) {
            cx.reveal_path(&entry.path());
        }
    }

    fn copy_path(&mut self, _: &CopyPath, _: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<_> = self
            .table
            .read(cx)
            .delegate()
            .selected_entries()
            .iter()
            .map(|entry| entry.path().display().to_string())
            .collect();
        if paths.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(paths.join("\n")));
        self.note = if paths.len() == 1 {
            "已复制完整路径".into()
        } else {
            format!("已复制 {} 条路径", paths.len())
        };
        cx.notify();
    }

    fn nudge(&mut self, delta: isize, cx: &mut Context<Self>) {
        let current = self.table.read(cx).selected_row().unwrap_or(0);
        let count = self.table.read(cx).delegate().rows.len();
        if count == 0 {
            return;
        }
        let next = (current as isize + delta).clamp(0, count as isize - 1) as usize;
        self.keep_selection = true;
        self.table.update(cx, |table, cx| {
            if table.delegate().anchor.is_none() {
                table.delegate_mut().select(current);
            }
            table.delegate_mut().extend(next);
            table.set_selected_row(next, cx);
        });
        self.keep_selection = false;
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().select_all();
            cx.notify();
        });
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.deleting {
            return;
        }
        let entries = self.table.read(cx).delegate().selected_entries();
        if entries.is_empty() {
            return;
        }
        self.deleting = true;
        self.note = "正在删除…".into();
        cx.notify();
        let count = entries.len();
        let task = std::thread::spawn(move || search::recycle(&entries));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { task.join() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.deleting = false;
                match result {
                    Ok(Ok(true)) => {
                        this.run_search(cx);
                        this.note = format!("已移到回收站：{count} 项");
                        cx.notify();
                    }
                    Ok(Ok(false)) => {
                        this.note.clear();
                        cx.notify();
                    }
                    Ok(Err(error)) => {
                        this.error = Some(format!("{error:#}"));
                        cx.notify();
                    }
                    Err(_) => {
                        this.error = Some("删除线程异常退出".into());
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn search_folder(&mut self, _: &SearchFolder, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.target(cx) {
            let path = if entry.folder {
                entry.path()
            } else {
                PathBuf::from(entry.directory)
            };
            let text = format!(
                "\"{}\\\"",
                path.display().to_string().trim_end_matches('\\')
            );
            self.query.filter = Filter::All;
            self.query.regex = false;
            self.input.update(cx, |input, cx| {
                input.set_value(text, window, cx);
                input.focus(window, cx);
            });
            self.restart(cx);
        }
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn page(&mut self, forward: bool, cx: &mut Context<Self>) {
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
            self.run_search(cx);
        }
    }

    fn export(&mut self, _: &Export, _: &mut Window, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let directory = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let prompt = cx.prompt_for_new_path(&directory, Some("everything-results.csv"));
        let query = self.query.clone();
        self.exporting = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result: anyhow::Result<Option<PathBuf>> = async {
                let Some(path) = prompt.await?? else {
                    return Ok(None);
                };
                let destination = path.clone();
                cx.background_executor()
                    .spawn(async move { search::export(&query, &destination) })
                    .await?;
                Ok(Some(path))
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.exporting = false;
                match result {
                    Ok(Some(path)) => this.note = format!("已导出：{}", path.display()),
                    Ok(None) => {}
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn bookmark(&mut self, remove: bool, cx: &mut Context<Self>) {
        if let Some(error) = &self.bookmark_error {
            self.error = Some(format!("书签未写入：{error}；请修复 bookmarks.json 后重启"));
            cx.notify();
            return;
        }
        let mut query = self.query.clone();
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
        cx.notify();
    }

    fn use_bookmark(&mut self, query: Query, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value(query.text.clone(), window, cx)
        });
        self.query = query;
        let column = self.query.sort_column.min(4);
        let descending = self.query.descending;
        self.table.update(cx, |table, cx| {
            for (index, definition) in table.delegate_mut().columns.iter_mut().enumerate() {
                definition.sort = Some(if index != column {
                    ColumnSort::Default
                } else if descending {
                    ColumnSort::Descending
                } else {
                    ColumnSort::Ascending
                });
            }
            table.refresh(cx);
        });
        self.restart(cx);
    }
}

impl Render for Everything {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let options = self.query.clone();
        let bookmarks = self.bookmarks.clone();
        let view = cx.weak_entity();
        let focus = self.focus.clone();
        let selected = self.target(cx);
        let rows = self.table.read(cx).delegate().rows.len();
        let status = if self.loading {
            "正在搜索…".into()
        } else if self.exporting {
            "正在导出全部结果…".into()
        } else if !self.note.is_empty() {
            self.note.clone()
        } else {
            let selected = self.table.read(cx).delegate().selected.len();
            if selected == 0 {
                format!("{} 个对象", self.total)
            } else {
                format!("{} 个对象 · 已选 {selected}", self.total)
            }
        };

        v_flex()
            .size_full()
            .key_context("Everything")
            .track_focus(&self.focus)
            .font_family("Microsoft YaHei UI")
            .text_size(px(13.))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::reveal))
            .on_action(cx.listener(Self::copy_path))
            .on_action(cx.listener(Self::search_folder))
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::export))
            .on_action(cx.listener(Self::refresh))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::select_all))
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Left {
                    this.click = Some(event.modifiers);
                }
            }))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.click = None;
                let key = event.keystroke.key.as_ref();
                if event.keystroke.modifiers.shift
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt
                    && matches!(key, "up" | "down")
                    && this.table.focus_handle(cx).is_focused(window)
                {
                    this.nudge(if key == "up" { -1 } else { 1 }, cx);
                }
            }))
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &PreviousPage, _, cx| this.page(false, cx)))
            .on_action(cx.listener(|this, _: &NextPage, _, cx| this.page(true, cx)))
            .on_action(cx.listener(|this, _: &AddBookmark, _, cx| this.bookmark(false, cx)))
            .on_action(cx.listener(|this, _: &RemoveBookmark, _, cx| this.bookmark(true, cx)))
            .on_action(cx.listener(|this, _: &MatchCase, _, cx| {
                this.query.match_case = !this.query.match_case;
                this.restart(cx);
            }))
            .on_action(cx.listener(|this, _: &MatchPath, _, cx| {
                this.query.match_path = !this.query.match_path;
                this.restart(cx);
            }))
            .on_action(cx.listener(|this, _: &WholeWord, _, cx| {
                this.query.whole_word = !this.query.whole_word;
                this.restart(cx);
            }))
            .on_action(cx.listener(|this, _: &Regex, _, cx| {
                this.query.regex = !this.query.regex;
                this.restart(cx);
            }))
            .on_action(cx.listener(|_, _: &ToggleTheme, window, cx| {
                let mode = if cx.theme().is_dark() {
                    ThemeMode::Light
                } else {
                    ThemeMode::Dark
                };
                Theme::change(mode, Some(window), cx);
            }))
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(self.menu(
                        "file-menu",
                        "文件",
                        {
                            let focus = focus.clone();
                            move |menu, _, _| {
                                menu.action_context(focus.clone())
                                    .menu("打开", Box::new(Open))
                                    .menu("打开所在目录", Box::new(Reveal))
                                    .separator()
                                    .menu("导出全部结果 CSV…", Box::new(Export))
                                    .separator()
                                    .menu("退出", Box::new(Quit))
                            }
                        },
                        cx,
                    ))
                    .child(self.menu(
                        "edit-menu",
                        "编辑",
                        {
                            let focus = focus.clone();
                            move |menu, _, _| {
                                menu.action_context(focus.clone())
                                    .menu("复制完整路径", Box::new(CopyPath))
                                    .menu("搜索此目录", Box::new(SearchFolder))
                                    .separator()
                                    .menu("定位搜索框", Box::new(FocusSearch))
                            }
                        },
                        cx,
                    ))
                    .child(self.menu(
                        "search-menu",
                        "搜索",
                        {
                            let focus = focus.clone();
                            move |menu, _, _| {
                                menu.action_context(focus.clone())
                                    .menu_with_check(
                                        "区分大小写",
                                        options.match_case,
                                        Box::new(MatchCase),
                                    )
                                    .menu_with_check(
                                        "匹配完整路径",
                                        options.match_path,
                                        Box::new(MatchPath),
                                    )
                                    .menu_with_check(
                                        "全字匹配",
                                        options.whole_word,
                                        Box::new(WholeWord),
                                    )
                                    .menu_with_check("正则表达式", options.regex, Box::new(Regex))
                            }
                        },
                        cx,
                    ))
                    .child(self.menu(
                        "view-menu",
                        "视图",
                        {
                            let focus = focus.clone();
                            move |menu, _, _| {
                                menu.action_context(focus.clone())
                                    .menu("刷新", Box::new(Refresh))
                                    .menu("切换深色／浅色", Box::new(ToggleTheme))
                            }
                        },
                        cx,
                    ))
                    .child(self.menu(
                        "bookmarks-menu",
                        "书签",
                        move |menu, _, _| {
                            let mut menu = menu
                                .action_context(focus.clone())
                                .menu("收藏当前搜索", Box::new(AddBookmark))
                                .menu("移除当前书签", Box::new(RemoveBookmark))
                                .separator();
                            for saved in &bookmarks {
                                let query = saved.clone();
                                let view = view.clone();
                                let label = format!(
                                    "{} · {}",
                                    saved.filter.label(),
                                    if saved.text.is_empty() {
                                        "全部对象"
                                    } else {
                                        &saved.text
                                    }
                                );
                                menu = menu.item(PopupMenuItem::new(label).on_click(
                                    move |_, window, cx| {
                                        let _ = view.update(cx, |this, cx| {
                                            this.use_bookmark(query.clone(), window, cx);
                                        });
                                    },
                                ));
                            }
                            menu
                        },
                        cx,
                    ))
                    .child(self.menu(
                        "help-menu",
                        "帮助",
                        |menu, _, _| {
                            menu.label("Everything Rust · GPUI")
                                .link(
                                    "Everything 搜索语法",
                                    "https://www.voidtools.com/support/everything/searching/",
                                )
                                .link("GPUI", "https://gpui.rs/")
                        },
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .px_2()
                    .pt_2()
                    .gap_1()
                    .children(Filter::ALL.map(|filter| {
                        Button::new(("filter", filter as usize))
                            .small()
                            .ghost()
                            .label(filter.label())
                            .selected(filter == self.query.filter)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.query.filter = filter;
                                this.restart(cx);
                            }))
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .p_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.input).cleanable(true)),
                    )
                    .child(
                        Button::new("refresh")
                            .small()
                            .icon(IconName::Redo)
                            .tooltip("刷新 · F5")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(&Refresh, window, cx)
                            })),
                    ),
            )
            .when_some(
                self.error.clone().or(self.bookmark_error.clone()),
                |view, error| {
                    view.child(div().px_3().py_2().text_color(rgb(0xb42318)).child(error))
                },
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .text_size(px(12.))
                    .child(Table::new(&self.table).xsmall().bordered(false)),
            )
            .child(
                h_flex()
                    .gap_3()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(status)
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(cx.theme().muted_foreground)
                            .child(selected.map_or_else(String::new, |entry| {
                                entry.path().display().to_string()
                            })),
                    )
                    .child(format!(
                        "{}–{} / {}",
                        if rows == 0 { 0 } else { self.query.offset + 1 },
                        self.query.offset + rows,
                        self.total
                    ))
                    .child(
                        Button::new("previous")
                            .small()
                            .ghost()
                            .label("上一页")
                            .disabled(self.loading || self.query.offset == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.page(false, cx))),
                    )
                    .child(
                        Button::new("next")
                            .small()
                            .ghost()
                            .label("下一页")
                            .disabled(
                                self.loading
                                    || self.query.offset.saturating_add(PAGE_SIZE) >= self.total,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.page(true, cx))),
                    ),
            )
    }
}

fn main() {
    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(|cx| {
            gpui_component::init(cx);
            Theme::change(ThemeMode::Light, None, cx);
            cx.bind_keys([
                KeyBinding::new("ctrl-f", FocusSearch, Some("Everything")),
                KeyBinding::new("ctrl-l", FocusSearch, Some("Everything")),
                KeyBinding::new("f5", Refresh, Some("Everything")),
                KeyBinding::new("enter", Open, Some("Everything && Table")),
                KeyBinding::new("alt-enter", Reveal, Some("Everything && Table")),
                KeyBinding::new("ctrl-c", CopyPath, Some("Everything && Table")),
                KeyBinding::new("ctrl-shift-c", CopyPath, Some("Everything")),
                KeyBinding::new("ctrl-e", Export, Some("Everything")),
                KeyBinding::new("ctrl-d", AddBookmark, Some("Everything")),
                KeyBinding::new("pageup", PreviousPage, Some("Everything && Table")),
                KeyBinding::new("pagedown", NextPage, Some("Everything && Table")),
                KeyBinding::new("ctrl-q", Quit, Some("Everything")),
                KeyBinding::new("delete", Delete, Some("Everything && Table")),
                KeyBinding::new("ctrl-a", SelectAll, Some("Everything && Table")),
            ]);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1200.), px(760.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(800.), px(480.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Everything — Rust".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| Everything::new(window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("无法创建 GPUI 窗口");
            cx.activate(true);
        });
}
