#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{self, Color32};
use regex::Regex;
use std::{path::{Path, PathBuf}, sync::{mpsc, Arc}, time::Instant, collections::BTreeMap};
use serde::{Serialize, Deserialize};

const BOOKS: &str = "C:/Cursor/Project/noveltest/books";
const PAGE_CHARS: usize = 2400;

struct Chapter { title: String, page: usize }
struct Book { text: String, pages: Vec<usize>, chapters: Vec<Chapter>, name: String, seconds: f64, word_prefix: Vec<usize> }

fn decode(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if bytes.len() % 2 != 0 { return Err("UTF-16 檔案長度不完整".into()); }
        let little = bytes[0] == 0xff;
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|b| if little { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) }).collect();
        String::from_utf16(&units).map_err(|_| "無效的 UTF-16 文字".into())
    } else {
        let mut text = String::from_utf8(bytes).map_err(|_| "請使用 UTF-8 或含 BOM 的 UTF-16 TXT".to_string())?;
        if text.starts_with('\u{feff}') { text.drain(..3); }
        Ok(text)
    }
}

fn index(text: String, name: String) -> Book {
    let heading = Regex::new(r"^(?:第[0-9０-９零〇一二三四五六七八九十百千萬万兩两壹貳參肆伍陸柒捌玖拾佰仟億亿]+[章回節节卷部篇集]|(?:序章|序言|楔子|前言|引子|尾聲|尾声|終章|终章|後記|后记|番外)(?:\s|[：:、.．（(]|$)|(?i:chapter)\s+[0-9]+)").unwrap();
    let mut pages = vec![0];
    let mut chapters = Vec::new();
    let mut offset = 0;
    let mut page_chars = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim().trim_start_matches('\u{feff}');
        if trimmed.len() <= 360 && heading.is_match(trimmed) {
            if *pages.last().unwrap() != offset { pages.push(offset); }
            page_chars = 0;
            chapters.push(Chapter { title: trimmed.to_owned(), page: pages.len() - 1 });
        }
        for (pos, ch) in line.char_indices() {
            if page_chars >= PAGE_CHARS {
                let boundary = offset + pos;
                if *pages.last().unwrap() != boundary { pages.push(boundary); }
                page_chars = 0;
            }
            page_chars += 1;
            if ch == '\n' && page_chars >= PAGE_CHARS * 3 / 4 {
                let boundary = offset + pos + 1;
                if boundary < text.len() { pages.push(boundary); }
                page_chars = 0;
            }
        }
        offset += line.len();
    }
    if chapters.first().map_or(true, |c| c.page != 0) { chapters.insert(0, Chapter { title: "開頭／前言".into(), page: 0 }); }
    let mut word_prefix = vec![0];
    for (i, start) in pages.iter().enumerate() {
        let end = pages.get(i + 1).copied().unwrap_or(text.len());
        word_prefix.push(word_prefix.last().unwrap() + text[*start..end].chars().filter(|c| !c.is_whitespace()).count());
    }
    Book { text, pages, chapters, name, seconds: 0.0, word_prefix }
}

fn display_text(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}
impl Book {
    fn title_at(&self, page: usize) -> String {
        let chapter = self.chapters.partition_point(|c| c.page <= page).saturating_sub(1);
        format!("{}({})", self.name, self.chapters[chapter].title)
    }
    fn read_count(&self, page: usize, character: usize) -> usize {
        self.word_prefix[page] + display_text(self.page(page)).chars().take(character).filter(|c| !c.is_whitespace()).count()
    }
    fn total_count(&self) -> usize { *self.word_prefix.last().unwrap() }

    fn page(&self, n: usize) -> &str { &self.text[self.pages[n]..self.pages.get(n + 1).copied().unwrap_or(self.text.len())] }
}

#[derive(PartialEq, Clone, Copy, Serialize, Deserialize)]
enum Spacing { Paragraph, EveryLine }

#[derive(Clone, Serialize, Deserialize)]
struct SavedBook {
    page: usize, offset: f32,
    #[serde(default)]
    character: Option<usize>,
    #[serde(default)]
    row_offset: f32,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct LibraryState { last: Option<PathBuf>, folder: Option<PathBuf>, books: BTreeMap<String, SavedBook>, settings: Option<Settings>, format_version: u32 }

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    font: f32, font_choice: usize, background: [u8; 4], foreground: [u8; 4],
    spacing: Spacing, strength: f32, pinned: bool, left_margin: f32, right_margin: f32, line_spacing: f32, paragraph_gap: f32, no_margins: bool,
}
impl Default for Settings {
    fn default() -> Self { Self { font: 24.0, font_choice: 2, background: [246,240,225,255], foreground: [48,43,37,255], spacing: Spacing::Paragraph, strength: 1.0, pinned: false, left_margin: 28.0, right_margin: 28.0, line_spacing: 1.45, paragraph_gap: 34.8, no_margins: false } }
}
fn settings_from_value(value: &serde_json::Value) -> Result<Settings, serde_json::Error> {
    let mut settings: Settings = serde_json::from_value(value.clone())?;
    if value.get("line_spacing").is_none() { settings.line_spacing = if settings.spacing == Spacing::EveryLine { 2.4 } else { 1.45 }; }
    if value.get("paragraph_gap").is_none() { settings.paragraph_gap = if settings.spacing == Spacing::EveryLine { 0.0 } else { settings.font * settings.line_spacing }; }
    Ok(settings)
}
fn load_state(bytes: &[u8]) -> Result<LibraryState, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let mut state: LibraryState = serde_json::from_value(value.clone())?;
    if state.settings.is_none() {
        let legacy = state.last.as_ref().and_then(|p| value.get("books")?.get(key(p)));
        state.settings = Some(match legacy { Some(v) => settings_from_value(v)?, None => Settings::default() });
    }
    if let Some(v) = value.get("settings").filter(|v| !v.is_null()) { state.settings = Some(settings_from_value(v)?); }
    if state.format_version < 2 {
        // v0.2 display strings inserted an extra newline between paragraphs.
        // Keep its block/pixel position instead of reusing incompatible character offsets.
        for book in state.books.values_mut() { book.character = None; }
    }
    state.format_version = 2;
    Ok(state)
}
fn reading_margins(width: f32, left: f32, right: f32) -> (f32, f32) {
    let left = left.clamp(0.0, 500.0); let right = right.clamp(0.0, 500.0);
    let budget = (width - 160.0).max(0.0);
    let scale = if left + right > budget { budget / (left + right).max(1.0) } else { 1.0 };
    (left * scale, right * scale)
}

fn key(path: &Path) -> String {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()).to_string_lossy().to_lowercase()
}

fn save_state(path: &Path, state: &LibraryState) -> Result<(), String> {
    use std::io::Write;
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(tmp, path).map_err(|e| e.to_string())
}

// Keep the top of the viewport anchored to a bounded text block.
fn move_position(page: &mut usize, offset: &mut f32, delta: f32, count: usize, mut height: impl FnMut(usize) -> f32) {
    *offset += delta;
    while *offset < 0.0 && *page > 0 {
        *page -= 1;
        *offset += height(*page).max(1.0);
    }
    *offset = offset.max(0.0);
    loop {
        let h = height(*page).max(1.0);
        if *offset < h { break; }
        if *page + 1 == count { *offset = (h - 1.0).max(0.0); break; }
        *offset -= h; *page += 1;
    }
}

fn layout_page(book: &Book, page: usize, width: f32, font: f32, line_spacing: f32, paragraph_gap: f32, ctx: &egui::Context) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    let display = display_text(book.page(page));
    job.append(&display, 0.0, egui::TextFormat { font_id: egui::FontId::proportional(font), color: Color32::WHITE, line_height: Some(font * line_spacing), ..Default::default() });
    let mut galley = (*ctx.fonts(|f| f.layout_job(job))).clone();
    let mut shift = 0.0;
    let mut bounds = egui::Rect::NOTHING;
    for row in &mut galley.rows {
        let delta = egui::vec2(0.0, shift);
        row.rect = row.rect.translate(delta);
        for glyph in &mut row.glyphs { glyph.pos += delta; }
        for vertex in &mut row.visuals.mesh.vertices { vertex.pos += delta; }
        row.visuals.mesh_bounds = row.visuals.mesh_bounds.translate(delta);
        bounds = bounds.union(row.visuals.mesh_bounds);
        if row.ends_with_newline { shift += paragraph_gap; }
    }
    galley.rect.max.y += shift;
    galley.mesh_bounds = bounds;
    Arc::new(galley)
}

struct Reader {
    files: Vec<PathBuf>, folder: PathBuf, book: Option<Book>, page: usize,
    receiver: Option<mpsc::Receiver<Result<Book, String>>>, status: String,
    search: String, matches: Vec<usize>, font: f32, font_choice: usize,
    background: Color32, foreground: Color32, spacing: Spacing, jump: usize,
    left_margin: f32, right_margin: f32, line_spacing: f32, paragraph_gap: f32, no_margins: bool, read_chars: usize,
    offset: f32, strength: f32, pinned: bool, top_visible: bool, left_visible: bool,
    state: LibraryState, state_path: PathBuf, active_path: Option<PathBuf>, pending_path: Option<PathBuf>,
    last_save: Instant, saved_json: String, save_error: String, state_readable: bool,
    character: usize, row_offset: f32, pending_anchor: Option<(usize, f32)>,
}

impl Reader {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        if let Ok(bytes) = std::fs::read("C:/Windows/Fonts/msjh.ttc") {
            fonts.font_data.insert("chinese".into(), egui::FontData::from_owned(bytes).into());
            fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "chinese".into());
        }
        cc.egui_ctx.set_fonts(fonts);
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        let mut style = (*cc.egui_ctx.style()).clone();
        for id in style.text_styles.values_mut() { id.size = 16.0; }
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        cc.egui_ctx.set_style(style);
        let state_path = std::env::current_exe().unwrap_or_default().with_file_name("reader-state.json");
        let (state, save_error) = match std::fs::read(&state_path) {
            Ok(bytes) => match load_state(&bytes) { Ok(s) => (s, String::new()), Err(e) => (LibraryState::default(), format!("閱讀紀錄無法解析：{e}")) },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (LibraryState::default(), String::new()),
            Err(e) => (LibraryState::default(), format!("閱讀紀錄讀取失敗：{e}")),
        };
        let mut app = Self { files: vec![], folder: state.folder.clone().unwrap_or_else(|| BOOKS.into()), book: None, page: 0, receiver: None, status: "滑鼠移到左邊或上邊開啟功能欄，選擇小說開始閱讀".into(), search: String::new(), matches: vec![], font: 24.0, font_choice: 2, background: Color32::from_rgb(246, 240, 225), foreground: Color32::from_rgb(48, 43, 37), spacing: Spacing::Paragraph, jump: 1, left_margin: 28.0, right_margin: 28.0, line_spacing: 1.45, paragraph_gap: 34.8, no_margins: false, read_chars: 0, offset: 0.0, strength: 1.0, pinned: false, top_visible: false, left_visible: false, state, state_path, active_path: None, pending_path: None, last_save: Instant::now(), saved_json: String::new(), state_readable: save_error.is_empty(), save_error, character: 0, row_offset: 0.0, pending_anchor: None };
        app.apply_settings();
        app.refresh();
        if let Some(path) = std::env::args_os().nth(1).map(PathBuf::from).or_else(|| app.state.last.clone()) { app.open(path, cc.egui_ctx.clone()); }
        app
    }

    fn apply_settings(&mut self) {
        let settings = self.state.settings.clone().unwrap_or_default();
        self.font = settings.font.clamp(12.0, 96.0); self.font_choice = settings.font_choice.min(4);
        self.background = Color32::from_rgba_unmultiplied(settings.background[0],settings.background[1],settings.background[2],settings.background[3]);
        self.foreground = Color32::from_rgba_unmultiplied(settings.foreground[0],settings.foreground[1],settings.foreground[2],settings.foreground[3]);
        self.spacing = settings.spacing; self.strength = settings.strength.clamp(0.2, 5.0); self.pinned = settings.pinned;
        self.no_margins = settings.no_margins;
        self.line_spacing = settings.line_spacing.clamp(1.0, 3.5); self.paragraph_gap = settings.paragraph_gap.clamp(0.0, 120.0);
        self.left_margin = settings.left_margin.clamp(0.0, 500.0); self.right_margin = settings.right_margin.clamp(0.0, 500.0);
    }

    fn persist(&mut self) {
        // Do not overwrite a corrupt/unreadable state file silently.
        if !self.state_readable { return; }
        if let Some(path) = &self.active_path {
            self.state.books.insert(key(path), SavedBook { page: self.page, offset: self.offset, character: Some(self.character), row_offset: self.row_offset });
            self.state.last = Some(path.clone());
        }
        self.state.settings = Some(Settings { font: self.font, font_choice: self.font_choice, background: self.background.to_array(), foreground: self.foreground.to_array(), spacing: self.spacing, strength: self.strength, pinned: self.pinned, left_margin: self.left_margin, right_margin: self.right_margin, line_spacing: self.line_spacing, paragraph_gap: self.paragraph_gap, no_margins: self.no_margins });
        self.state.folder = Some(self.folder.clone());
        self.state.format_version = 2;
        let json = serde_json::to_string(&self.state).unwrap_or_default();
        if json != self.saved_json {
            match save_state(&self.state_path, &self.state) {
                Ok(()) => { self.saved_json = json; self.save_error.clear(); }
                Err(e) => self.save_error = format!("保存失敗：{e}"),
            }
        }
        self.last_save = Instant::now();
    }

    fn restore(&mut self) {
        self.page = 0; self.offset = 0.0; self.read_chars = 0; self.pending_anchor = None;
        if let Some(saved) = self.active_path.as_ref().and_then(|p| self.state.books.get(&key(p))) {
            self.page = saved.page.min(self.book.as_ref().unwrap().pages.len() - 1);
            self.offset = if saved.offset.is_finite() { saved.offset.max(0.0) } else { 0.0 };
            self.pending_anchor = saved.character.map(|c| (c, saved.row_offset));
        }
        self.jump = self.page + 1;
    }

    fn refresh(&mut self) {
        match std::fs::read_dir(&self.folder) {
            Ok(items) => {
                self.files = items.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("txt"))).collect();
                self.files.sort();
            }
            Err(e) => { self.files.clear(); self.status = format!("無法列出書架：{e}"); }
        }
    }

    fn open(&mut self, path: PathBuf, ctx: egui::Context) {
        if self.receiver.is_some() { return; }
        self.persist();
        self.active_path = None;
        self.pending_path = Some(path.clone());
        self.status = format!("背景讀取及分章：{}", path.file_name().unwrap_or_default().to_string_lossy());
        // Release the previous large book before loading the next one.
        self.book = None;
        self.matches.clear();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        std::thread::spawn(move || {
            let start = Instant::now();
            let result = std::fs::read(&path).map_err(|e| e.to_string()).and_then(decode).map(|text| {
                let mut b = index(text, path.file_name().unwrap_or_default().to_string_lossy().into());
                b.seconds = start.elapsed().as_secs_f64(); b
            });
            let _ = tx.send(result); ctx.request_repaint();
        });
    }

    fn filter(&mut self) {
        let query = self.search.trim().to_lowercase();
        self.matches = self.book.as_ref().map(|b| b.chapters.iter().enumerate().filter(|(_, c)| query.is_empty() || c.title.to_lowercase().contains(&query)).map(|(i, _)| i).collect()).unwrap_or_default();
    }
}

impl eframe::App for Reader {
    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) { self.persist(); }
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if let Some(rx) = &self.receiver {
            match rx.try_recv() {
                Ok(result) => {
                    self.receiver = None;
                    match result {
                        Ok(book) => {
                            self.status = format!("{:.1} MB · {} 個章節項目 · 載入 {:.2} 秒", book.text.len() as f64 / 1048576.0, book.chapters.len(), book.seconds);
                            self.book = Some(book); self.active_path = self.pending_path.take(); self.restore(); self.search.clear(); self.filter();
                        }
                        Err(e) => self.status = format!("無法開啟：{e}"),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => { self.receiver = None; self.status = "背景載入中斷，請重新開啟".into(); }
                Err(mpsc::TryRecvError::Empty) => { ctx.request_repaint_after(std::time::Duration::from_millis(100)); }
            }
        }
        let mut open = None;
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let popup = ctx.memory(|m| m.any_popup_open());
        self.top_visible = self.pinned || popup || pointer.is_some_and(|p| p.y < if self.top_visible { 175.0 } else { 18.0 });
        self.left_visible = self.pinned || pointer.is_some_and(|p| p.x < if self.left_visible { 300.0 } else { 18.0 });
        if self.top_visible { egui::Window::new("settings").title_bar(false).resizable(false).fixed_pos([8.0, 8.0]).default_width(820.0).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("靜讀 · TXT 閱讀器");
                if ui.add_enabled(self.receiver.is_none(), egui::Button::new("開啟 TXT")).clicked() { open = rfd::FileDialog::new().add_filter("文字檔", &["txt"]).set_directory(&self.folder).pick_file(); }
                if ui.button("書架資料夾").clicked() { if let Some(p) = rfd::FileDialog::new().set_directory(&self.folder).pick_folder() { self.folder = p; self.refresh(); } }
                if ui.button("重新整理").clicked() { self.refresh(); }
                ui.checkbox(&mut self.pinned, "固定功能欄");
                if self.receiver.is_some() { ui.spinner(); }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("配色");
                for (name, bg, fg) in [("紙張", [246,240,225], [48,43,37]), ("夜讀", [28,31,36], [218,222,228]), ("護眼綠", [215,232,210], [36,57,40]), ("純白", [255,255,255], [30,30,30]), ("灰霧", [224,227,232], [40,45,53])] {
                    if ui.button(name).clicked() { self.background = Color32::from_rgb(bg[0],bg[1],bg[2]); self.foreground = Color32::from_rgb(fg[0],fg[1],fg[2]); }
                }
                ui.label("背景"); ui.color_edit_button_srgba(&mut self.background);
                ui.label("文字"); ui.color_edit_button_srgba(&mut self.foreground);
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("字級");
                for (i, (name, size)) in [("特大",40.0),("大",32.0),("中",24.0),("小",18.0),("自訂",24.0)].iter().enumerate() {
                    if ui.selectable_label(self.font_choice == i, *name).clicked() { self.font_choice = i; if i != 4 { self.font = *size; } }
                }
                if ui.add(egui::DragValue::new(&mut self.font).range(12.0..=96.0).suffix(" px")).changed() { self.font_choice = 4; }
                ui.separator(); ui.label("隔行");
                if ui.selectable_value(&mut self.spacing, Spacing::Paragraph, "按段隔行").clicked() { self.line_spacing = 1.45; self.paragraph_gap = self.font * 1.45; self.pending_anchor = Some((self.character, self.row_offset)); }
                if ui.selectable_value(&mut self.spacing, Spacing::EveryLine, "全部隔1行").clicked() { self.line_spacing = 2.4; self.paragraph_gap = 0.0; self.pending_anchor = Some((self.character, self.row_offset)); }
                ui.label("捲動強度");
                ui.add(egui::DragValue::new(&mut self.strength).range(0.2..=5.0).speed(0.1).suffix(" ×"));
            });
            ui.horizontal(|ui| {
                ui.label("行距");
                let line = ui.add(egui::DragValue::new(&mut self.line_spacing).range(1.0..=3.5).speed(0.05).suffix(" 倍")).changed();
                ui.label("段落間距");
                let paragraph = ui.add(egui::DragValue::new(&mut self.paragraph_gap).range(0.0..=120.0).speed(1.0).suffix(" px")).changed();
                if line || paragraph { self.pending_anchor = Some((self.character, self.row_offset)); }
            });
            ui.horizontal(|ui| {
                ui.label("全域 · 左留白");
                let left = ui.add(egui::DragValue::new(&mut self.left_margin).range(0.0..=500.0).speed(2.0).suffix(" px")).changed();
                ui.label("右留白");
                let right = ui.add(egui::DragValue::new(&mut self.right_margin).range(0.0..=500.0).speed(2.0).suffix(" px")).changed();
                if left || right { self.pending_anchor = Some((self.character, self.row_offset)); }
                if ui.checkbox(&mut self.no_margins, "不留白").on_hover_text("覆蓋左右留白；取消後恢復原設定").changed() { self.pending_anchor = Some((self.character, self.row_offset)); }
            });
        }); }
        egui::TopBottomPanel::bottom("navigation").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some(book) = &self.book {
                    let total = book.pages.len();
                    if ui.add_enabled(self.page > 0, egui::Button::new("◀ 上一頁")).clicked() { self.page -= 1; self.offset = 0.0; }
                    if ui.add_enabled(self.page + 1 < total, egui::Button::new("下一頁 ▶")).clicked() { self.page += 1; self.offset = 0.0; }
                    ui.label(format!("第 {} / {} 頁", self.page + 1, total));
                    let count = book.total_count();
                    let read = self.read_chars.min(count);
                    ui.label(format!("已讀 {read} / {count} 字 · {:.2}%", if count == 0 { 0.0 } else { read as f64 * 100.0 / count as f64 })).on_hover_text("截至畫面底部的閱讀位置；含標點，不計空白。向上捲動時進度也會回退。");
                    ui.add(egui::DragValue::new(&mut self.jump).range(1..=total));
                    if ui.button("跳頁").clicked() { self.page = self.jump.saturating_sub(1).min(total - 1); self.offset = 0.0; }
                    if !ctx.wants_keyboard_input() {
                        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) { self.page = (self.page + 1).min(total - 1); self.offset = 0.0; }
                        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) { self.page = self.page.saturating_sub(1); self.offset = 0.0; }
                    }
                }
                ui.label(&self.status);
                ui.checkbox(&mut self.pinned, "固定功能欄").on_hover_text("取消固定後，滑鼠移到視窗上緣或左緣即可顯示");
                if !self.save_error.is_empty() { ui.colored_label(Color32::RED, &self.save_error); }
            });
        });
        if self.left_visible { egui::Window::new("library").title_bar(false).fixed_pos([8.0, 185.0]).fixed_size([270.0, (ctx.screen_rect().height()-255.0).max(200.0)]).resizable(false).show(ctx, |ui| {
            ui.heading("書架");
            ui.small(self.folder.to_string_lossy());
            egui::ScrollArea::vertical().id_salt("books").max_height(160.0).show_rows(ui, 25.0, self.files.len(), |ui, range| {
                for n in range { let path = &self.files[n]; if ui.add_enabled(self.receiver.is_none(), egui::Button::new(path.file_stem().unwrap_or_default().to_string_lossy()).truncate()).clicked() { open = Some(path.clone()); } }
            });
            ui.separator(); ui.heading("章節");
            if ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("搜尋章節標題")).changed() { self.filter(); }
            ui.small(format!("{} 筆結果", self.matches.len()));
            if let Some(book) = &self.book {
                let current = book.chapters.partition_point(|c| c.page <= self.page).saturating_sub(1);
                egui::ScrollArea::vertical().id_salt("chapters").show_rows(ui, 27.0, self.matches.len(), |ui, range| {
                    for n in range {
                        let i = self.matches[n]; let chapter = &book.chapters[i];
                        if ui.add(egui::Button::new(&chapter.title).selected(current == i).truncate()).on_hover_text(&chapter.title).clicked() { self.page = chapter.page; self.offset = 0.0; }
                    }
                });
            }
        }); }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(self.background).inner_margin(0.0)).show(ctx, |ui| {
            let mut rect = ui.available_rect_before_wrap();
            rect.min.y += 28.0; rect.max.y -= 28.0;
            if self.pinned { rect.min.x += 300.0; rect.min.y += 175.0; }
            let (left, right) = reading_margins(rect.width(), if self.no_margins { 0.0 } else { self.left_margin }, if self.no_margins { 0.0 } else { self.right_margin });
            rect.min.x += left; rect.max.x -= right;
            if let Some(book) = &self.book {
                let title_pos = rect.min;
                rect.min.y += 30.0;
                let width = rect.width().max(80.0);
                let mut cache = BTreeMap::new();
                let mut get = |page| cache.entry(page).or_insert_with(|| layout_page(book, page, width, self.font, self.line_spacing, self.paragraph_gap, ctx)).clone();
                let hovered = pointer.is_some_and(|p| rect.contains(p)) && ctx.layer_id_at(pointer.unwrap_or_default()) == Some(ui.layer_id());
                let delta = if hovered { -ctx.input(|i| i.smooth_scroll_delta.y) * self.strength } else { 0.0 };
                if let Some((character, row_offset)) = self.pending_anchor.take() {
                    let galley = get(self.page);
                    let cursor = galley.from_ccursor(egui::text::CCursor::new(character));
                    self.offset = (galley.pos_from_cursor(&cursor).top() + row_offset).max(0.0);
                }
                move_position(&mut self.page, &mut self.offset, delta, book.pages.len(), |p| get(p).size().y.max(self.font * self.line_spacing) + if book.page(p).ends_with('\n') { self.paragraph_gap } else { 0.0 });
                // Near EOF, keep the final line visible without an empty screen.
                if self.page + 1 == book.pages.len() {
                    self.offset = self.offset.min((get(self.page).size().y - rect.height()).max(0.0));
                }
                ui.painter().with_clip_rect(ui.max_rect()).text(title_pos, egui::Align2::LEFT_TOP, book.title_at(self.page), egui::FontId::proportional(16.0), self.foreground);
                let top = get(self.page);
                let cursor = top.cursor_from_pos(egui::vec2(0.0, self.offset));
                self.character = cursor.ccursor.index;
                self.row_offset = self.offset - top.pos_from_cursor(&cursor).top();
                let painter = ui.painter().with_clip_rect(rect);
                let mut y = rect.top() - self.offset;
                let mut page = self.page;
                let mut read = book.word_prefix[page];
                while y < rect.bottom() && page < book.pages.len() {
                    let galley = get(page);
                    let end = galley.cursor_from_pos(egui::vec2(width, rect.bottom() - y));
                    read = book.read_count(page, end.ccursor.index);
                    painter.galley_with_override_text_color(egui::pos2(rect.left(), y), galley.clone(), self.foreground);
                    y += galley.size().y.max(self.font * self.line_spacing) + if book.page(page).ends_with('\n') { self.paragraph_gap } else { 0.0 };
                    page += 1;
                }
                if read != self.read_chars { self.read_chars = read; ctx.request_repaint(); }
                ui.allocate_rect(rect, egui::Sense::hover());
            } else {
                ui.painter().text(rect.min, egui::Align2::LEFT_TOP, &self.status, egui::FontId::proportional(22.0), self.foreground);
            }
        });
        if self.last_save.elapsed().as_secs_f32() >= 1.0 { self.persist(); }
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        if let Some(path) = open { self.open(path, ctx.clone()); }
    }
}

fn main() -> eframe::Result {
    eframe::run_native("靜讀 · 大型 TXT 小說閱讀器", eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).expect("embedded icon")).with_inner_size([1250.0, 900.0]).with_min_inner_size([850.0, 600.0]), ..Default::default() }, Box::new(|cc| Ok(Box::new(Reader::new(cc)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chapter_title_and_unicode_reading_counts_follow_source_order() {
        let book = index("第1章 A\n  中文🙂。 \n\n第2章 B\n終點！".into(), "abc.txt".into());
        assert_eq!(book.title_at(0), "abc.txt(第1章 A)");
        assert_eq!(book.title_at(1), "abc.txt(第2章 B)");
        assert_eq!(book.total_count(), book.text.chars().filter(|c| !c.is_whitespace()).count());
        assert_eq!(book.read_count(0, 0), 0);
        assert_eq!(book.read_count(0, usize::MAX), book.word_prefix[1]);
        assert_eq!(book.read_count(1, usize::MAX), book.total_count());
        assert_eq!(index(String::new(), "empty".into()).total_count(), 0);
    }

    #[test]
    fn icon_and_global_margin_override_are_valid_and_persisted() {
        let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap();
        assert_eq!((icon.width, icon.height, icon.rgba.len()), (256, 256, 256 * 256 * 4));
        let settings = Settings { no_margins: true, left_margin: 123.0, right_margin: 234.0, ..Settings::default() };
        let restored: Settings = serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert!(restored.no_margins);
        assert_eq!((restored.left_margin, restored.right_margin), (123.0, 234.0));
        assert!(!serde_json::from_str::<Settings>("{}").unwrap().no_margins);
    }
    #[test]
    fn line_and_paragraph_spacing_are_independent_and_cursor_geometry_matches() {
        let b = index("第一段文字\n第二段文字\n第三段文字".into(), "test".into());
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let normal = layout_page(&b, 0, 600.0, 24.0, 1.5, 0.0, ctx);
            let paragraphs = layout_page(&b, 0, 600.0, 24.0, 1.5, 20.0, ctx);
            let lines = layout_page(&b, 0, 600.0, 24.0, 2.5, 0.0, ctx);
            assert_eq!(normal.rows.len(), 3);
            assert!((paragraphs.rows[1].rect.top() - normal.rows[1].rect.top() - 20.0).abs() < 0.01);
            assert!((paragraphs.size().y - normal.size().y - 40.0).abs() < 0.01);
            assert!(lines.size().y > normal.size().y);
            let cursor = paragraphs.cursor_from_pos(egui::vec2(0.0, paragraphs.rows[1].rect.center().y));
            assert!((paragraphs.pos_from_cursor(&cursor).top() - paragraphs.rows[1].rect.top()).abs() < 0.01);
        });
    }
    #[test]
    fn legacy_last_book_migrates_to_global_and_new_global_takes_precedence() {
        let path = PathBuf::from("legacy-book.txt");
        let mut books = serde_json::Map::new();
        books.insert(key(&path), serde_json::json!({"page":12,"offset":45.0,"font":40.0,"font_choice":0,"background":[1,2,3,255],"foreground":[200,201,202,255],"spacing":"EveryLine","strength":2.0,"pinned":true}));
        let mut value = serde_json::json!({"last":path,"books":books});
        let state = load_state(&serde_json::to_vec(&value).unwrap()).unwrap();
        let settings = state.settings.unwrap();
        assert_eq!(settings.font, 40.0);
        assert_eq!(settings.left_margin, 28.0);
        assert_eq!(state.books[&key(&path)].page, 12);
        value["settings"] = serde_json::json!({"font":18.0,"left_margin":130.0,"right_margin":210.0});
        let state = load_state(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(state.settings.as_ref().unwrap().font, 18.0);
        let serialized = serde_json::to_value(state).unwrap();
        assert!(serialized["books"][key(&path)].get("font").is_none());
    }

    #[test]
    fn independent_margins_preserve_readable_width() {
        assert_eq!(reading_margins(1000.0, 100.0, 200.0), (100.0, 200.0));
        let (left, right) = reading_margins(400.0, 200.0, 400.0);
        assert!((left - 80.0).abs() < 0.01);
        assert!((right - 160.0).abs() < 0.01);
        assert_eq!(reading_margins(100.0, 500.0, 500.0), (0.0, 0.0));
    }
    #[test]
    fn unicode_pages_reconstruct_source_and_chapters_keep_original_order() {
        let source = format!("前言\n介紹\n第十二章 測試\n{}\n第2章 重設章號\n終章\n完結", "中文🙂".repeat(4000));
        let b = index(source.clone(), "test".into());
        assert_eq!((0..b.pages.len()).map(|n| b.page(n)).collect::<String>(), source);
        assert!(b.pages.windows(2).all(|p| p[0] < p[1]));
        assert!((0..b.pages.len()).all(|n| b.page(n).chars().count() <= PAGE_CHARS));
        assert_eq!(b.chapters.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), vec!["前言", "第十二章 測試", "第2章 重設章號", "終章"]);
    }
    #[test]
    fn encodings_and_empty_text() {
        assert_eq!(decode(b"\xef\xbb\xbftext".to_vec()).unwrap(), "text");
        for little in [true, false] {
            let mut bytes = if little { vec![255,254] } else { vec![254,255] };
            for u in "中文🙂".encode_utf16() { bytes.extend(if little { u.to_le_bytes() } else { u.to_be_bytes() }); }
            assert_eq!(decode(bytes).unwrap(), "中文🙂");
        }
        assert!(decode(vec![255,254,0]).is_err());
        assert_eq!(index(String::new(), "empty".into()).page(0), "");
    }
    #[test]
    fn continuous_scroll_crosses_both_boundaries_without_losing_distance() {
        let heights = [120.0, 240.0, 180.0];
        let (mut page, mut offset) = (1, 0.0);
        move_position(&mut page, &mut offset, -30.0, 3, |p| heights[p]);
        assert_eq!((page, offset), (0, 90.0));
        move_position(&mut page, &mut offset, 300.0, 3, |p| heights[p]);
        assert_eq!((page, offset), (2, 30.0));
        move_position(&mut page, &mut offset, -1000.0, 3, |p| heights[p]);
        assert_eq!((page, offset), (0, 0.0));
        move_position(&mut page, &mut offset, 1000.0, 3, |p| heights[p]);
        assert_eq!((page, offset), (2, 179.0));
    }

    #[test]
    fn global_settings_and_separate_positions_survive_atomic_replacement() {
        let folder = PathBuf::from("verification/reader-state-test");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("state.json");
        let mut state = LibraryState::default();
        let saved = SavedBook { page: 51, offset: 325.5, character: Some(120), row_offset: 3.0 };
        state.books.insert("first".into(), saved.clone());
        state.books.insert("second".into(), SavedBook { page: 2, ..saved });
        state.settings = Some(Settings { font: 40.0, left_margin: 100.0, right_margin: 200.0, ..Settings::default() });
        state.last = Some("second.txt".into());
        save_state(&path, &state).unwrap();
        state.books.get_mut("first").unwrap().offset = 400.0;
        save_state(&path, &state).unwrap();
        let loaded: LibraryState = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(serde_json::to_string(&loaded).unwrap(), serde_json::to_string(&state).unwrap());
        assert_eq!(loaded.books["first"].offset, 400.0);
        assert_eq!(loaded.books["second"].page, 2);
        assert_eq!(loaded.settings.unwrap().right_margin, 200.0);
    }

    #[test]
    fn rendered_chapter_boundary_and_reflow_anchor_are_stable() {
        let b = index(format!("第25章 前章\n{}\n第26章 本章\n{}\n第27章 後章\n正文", "前章正文。\n".repeat(50), "本章正文。\n".repeat(50)), "test".into());
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let chapter = b.chapters.iter().find(|c| c.title.starts_with("第26章")).unwrap();
            let mut page = chapter.page;
            let mut offset = 0.0;
            let height = |p| layout_page(&b, p, 600.0, 24.0, 1.45, 34.8, ctx).size().y + 14.4;
            move_position(&mut page, &mut offset, -80.0, b.pages.len(), height);
            assert_eq!(page, chapter.page - 1);
            assert!((offset - (height(page) - 80.0)).abs() < 0.1);
            move_position(&mut page, &mut offset, 100.0, b.pages.len(), height);
            assert_eq!(page, chapter.page);
            assert!((offset - 20.0).abs() < 0.1);
            let original = layout_page(&b, page, 600.0, 24.0, 1.45, 34.8, ctx);
            let anchor = original.cursor_from_pos(egui::vec2(0.0, 150.0));
            let narrow = layout_page(&b, page, 300.0, 24.0, 1.45, 34.8, ctx);
            let restored = narrow.from_ccursor(anchor.ccursor);
            assert_eq!(restored.ccursor.index, anchor.ccursor.index);
            assert!(narrow.pos_from_cursor(&restored).top().is_finite());
        });
    }
    #[test]
    fn over_100mb_index_and_render_budget() {
        let start = Instant::now();
        let mut text = String::with_capacity(125_000_000);
        let paragraph = "這是一段大型小說測試正文，用來驗證背景分章及分頁效能。\n".repeat(100);
        let mut n = 0;
        while text.len() < 120_000_000 { n += 1; text.push_str(&format!("第{n}章 測試\n")); text.push_str(&paragraph); }
        let bytes = text.len();
        let folder = PathBuf::from("verification/reader");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("large-120mb.txt");
        std::fs::write(&path, &text).unwrap();
        drop(text);
        let load_start = Instant::now();
        let b = index(decode(std::fs::read(&path).unwrap()).unwrap(), "large".into());
        println!("Disk read + UTF-8 decode + index: {:.3}s", load_start.elapsed().as_secs_f64());
        assert!(bytes > 100 * 1024 * 1024);
        assert_eq!(b.chapters.len(), n);
        assert!((0..b.pages.len()).all(|p| b.page(p).chars().count() <= PAGE_CHARS));
        assert_eq!((0..b.pages.len()).map(|p| b.page(p).len()).sum::<usize>(), bytes);
        println!("{bytes} bytes, {} pages, {n} chapters, {:.3}s including generation and validation", b.pages.len(), start.elapsed().as_secs_f64());
    }
}
