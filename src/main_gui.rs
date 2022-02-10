#![windows_subsystem = "windows"]
#![feature(async_closure)]
#![feature(once_cell)]
#![feature(slice_take)]
#![feature(const_mut_refs)]
#![feature(thread_is_running)]
#![feature(try_blocks)]

use native_windows_derive as nwd;
use native_windows_gui as nwg;

mod shared;
mod uhash;
mod ustring;

use std::{
    cell::RefCell,
    fs::canonicalize,
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};

use nwd::NwgUi;
use nwg::stretch::{
    geometry::{Rect, Size},
    style::{AlignItems, AlignSelf, Dimension as D, FlexDirection, JustifyContent},
};
use nwg::NativeUi;

use pathdiff::diff_paths;
use winapi::{
    shared::windef::HWND,
    um::{
        shellscalingapi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        wincon::{AttachConsole, FreeConsole, GetConsoleWindow, ATTACH_PARENT_PROCESS},
        winuser::MonitorFromWindow,
    },
};

use crate::shared::{analyze, Analysis, AnalyzeSource, Args};

static ICON: &[u8] = include_bytes!("../resources/book.ico");

const PT_0: D = D::Points(0.0);
const RECT_0: Rect<D> = Rect {
    start: PT_0,
    end: PT_0,
    top: PT_0,
    bottom: PT_0,
};
const PT_10: D = D::Points(10.0);
const RECT_100: Rect<D> = Rect {
    start: PT_0,
    end: PT_0,
    top: PT_10,
    bottom: PT_0,
};
const RECT_101: Rect<D> = Rect {
    start: PT_10,
    end: PT_10,
    top: PT_10,
    bottom: PT_10,
};
const RECT_102: Rect<D> = Rect {
    start: PT_0,
    end: PT_0,
    top: PT_10,
    bottom: PT_10,
};

enum Message {
    Status(String),
    Results(String),
    End,
}

#[derive(Default, NwgUi)]
pub struct App {
    #[nwg_control(
        title: "WordStat",
        accept_files: true,
        flags: "MAIN_WINDOW|VISIBLE|RESIZABLE",
        center: true,
        size: (820, 620)
    )]
    #[nwg_events(
        OnInit: [App::setup],
        OnWindowClose: [nwg::stop_thread_dispatch()],
        OnFileDrop: [App::drop(SELF, EVT_DATA)],
        OnMinMaxInfo: [App::resize(SELF, EVT_DATA)],
        OnKeyPress: [App::keypress(SELF, EVT_DATA)],
        OnKeyRelease: [App::keyrelease(SELF, EVT_DATA)],
    )]
    window: nwg::Window,

    #[nwg_control(
        text: "&Settings"
    )]
    #[nwg_events(
        OnMenuOpen: [App::menu_settings],
    )]
    menu_settings: nwg::Menu,
    #[nwg_control(
        text: "&Lowercase Words",
        parent: menu_settings,
        check: false
    )]
    #[nwg_events(
        OnMenuItemSelected: [App::menu_settings_lowercase],
    )]
    menu_settings_lowercase: nwg::MenuItem,
    #[nwg_control(
        text: "&Hide Empty Sources",
        parent: menu_settings,
        check: false
    )]
    #[nwg_events(
        OnMenuItemSelected: [App::menu_settings_hide_empty],
    )]
    menu_settings_hide_empty: nwg::MenuItem,

    #[nwg_layout(
        parent: window,
        flex_direction: FlexDirection::Column,
        padding: RECT_101,
        min_size: Size { width: D::Points(420.0), height: D::Points(520.0) },
        align_items: AlignItems::Stretch,
        justify_content: JustifyContent::Center,
    )]
    layout: nwg::FlexboxLayout,

    #[nwg_control(
        flags: "VISIBLE|MARQUEE",
        marquee: true,
        marquee_update: 1,
    )]
    #[nwg_layout_item(
        layout: layout,
        align_self: AlignSelf::Stretch,
        margin: RECT_0,
        flex_grow: 0.0,
        min_size: Size { width: D::Auto, height: D::Points(15.0) },
    )]
    progress: nwg::ProgressBar,

    #[nwg_control(
        flags: "VISIBLE",
        placeholder_text: Some("Search..."),
    )]
    #[nwg_layout_item(
        layout: layout,
        align_self: AlignSelf::Stretch,
        margin: RECT_100,
        flex_grow: 0.1,
        size: Size { width: D::Auto, height: D::Points(20.0) },
    )]
    #[nwg_events(
        OnKeyPress: [App::keypress(SELF, EVT_DATA)],
        OnTextInput: [App::search(SELF)],
    )]
    search: nwg::TextInput,

    #[nwg_control(
        readonly: true,
        text: "Drop a file here to analyze it.",
        flags: "VISIBLE|VSCROLL|AUTOVSCROLL|AUTOHSCROLL",
        focus: false,
    )]
    #[nwg_layout_item(
        layout: layout,
        align_self: AlignSelf::Stretch,
        margin: RECT_102,
        flex_grow: 100.0,
    )]
    #[nwg_events(
        OnKeyPress: [App::keypress(SELF, EVT_DATA)]
    )]
    text: nwg::RichTextBox,

    #[nwg_control(
        text: ""
    )]
    #[nwg_layout_item(
        layout: layout,
        align_self: AlignSelf::Stretch,
        flex_grow: 0.0,
        min_size: Size { width: D::Auto, height: D::Points(10.0) },
    )]
    status: nwg::StatusBar,

    #[nwg_control(
        parent: window,
        interval: Duration::from_millis(10),
        active: true,
    )]
    #[nwg_events(
        OnTimerTick: [App::timertick(SELF)],
    )]
    timer: nwg::AnimationTimer,

    #[nwg_control(
        parent: window,
        interval: Duration::from_millis(50),
        max_tick: Some(1),
        active: true,
    )]
    #[nwg_events(
        OnTimerTick: [App::matchdpi(SELF)],
    )]
    dpitimer: nwg::AnimationTimer,

    tx: RefCell<Option<flume::Sender<Message>>>,
    tr: RefCell<Option<flume::Receiver<Message>>>,

    last_result_thread: RefCell<Option<flume::Sender<bool>>>,

    control_pressed: Arc<AtomicBool>,
    #[allow(clippy::type_complexity)]
    thread: RefCell<Option<JoinHandle<(Vec<Analysis>, Option<Analysis>)>>>,
    analyses: RefCell<(Vec<Analysis>, Option<Analysis>)>,

    args: RefCell<Args>,
    pwd: RefCell<PathBuf>,
    last_source: RefCell<Vec<AnalyzeSource>>,

    dpi: Arc<AtomicU32>,
}

fn analysis_words_to_string(analysis: &Analysis, top_words: usize, bottom_words: usize) -> String {
    if analysis.word_freq.is_empty() {
        return "".to_owned();
    }
    let pad = format!("{}", analysis.word_freq[0].0).len();
    let mut buffer = String::new();
    for (i, (freq, string)) in analysis.word_freq.iter().enumerate() {
        if top_words > 0 && i >= top_words {
            break;
        };
        buffer.push_str(&format!("  {:width$}", freq, width = pad));
        buffer.push_str(": ");
        buffer.push_str(string);
        buffer.push('\n');
    }
    if bottom_words > 0
        && top_words != 0
        && top_words < analysis.word_count
        && top_words < analysis.word_freq.len()
    {
        buffer.push_str("  ...\n");
        for (i, (freq, string)) in analysis.word_freq.iter().rev().enumerate() {
            if bottom_words > 0 && i >= bottom_words {
                break;
            };
            buffer.push_str(&format!("  {:width$}", freq, width = pad));
            buffer.push_str(": ");
            buffer.push_str(string);
            buffer.push('\n');
        }
    }
    buffer
}

fn analysis_to_string(
    analysis: &Analysis,
    top_words: usize,
    bottom_words: usize,
    hide_empty: bool,
    search_text: &str,
) -> String {
    let mut buffer = String::new();
    let analysis_string = if search_text.is_empty() {
        analysis_words_to_string(analysis, top_words, bottom_words)
    } else {
        let mut tmp_analysis = analysis.clone();
        tmp_analysis.word_freq = tmp_analysis
            .word_freq
            .into_iter()
            .filter(|(_, string)| string.to_lowercase().contains(&search_text.to_lowercase()))
            .collect();
        let analysis_string = analysis_words_to_string(&tmp_analysis, top_words, bottom_words);
        analysis_string
            .lines()
            .filter(|line| line.to_lowercase().contains(&search_text.to_lowercase()))
            .map(String::from)
            .reduce(|a, b| format!("{}\n{}", a, b))
            .unwrap_or_default()
    };
    if analysis_string.is_empty() && hide_empty {
        return buffer;
    }
    buffer.push_str(&format!("🔢 Word count: {}\n", analysis.word_count));
    buffer.push_str(&format!("🔢 Sentence count: {}\n", analysis.sent_count));
    buffer.push_str(&format!("🔢 Character count: {}\n", analysis.char_count));
    buffer.push_str(&format!("🔢 Paragraph count: {}\n", analysis.para_count));
    buffer.push_str(&format!("🔢 Unique words: {}\n", analysis.word_uniqs));
    buffer.push_str(&format!(
        "🔢 Word frequency mean: {:.2}\n",
        analysis.word_dist_mean
    ));
    buffer.push_str(&format!(
        "🔢 Word frequency standard deviation: {:.2}\n",
        analysis.word_dist_stddev
    ));
    buffer.push_str("📈 Top words:\n");
    buffer.push_str(if analysis_string.is_empty() {
        if search_text.is_empty() {
            "⚠️ No words in file"
        } else {
            "⚠️ No results in file"
        }
    } else {
        &analysis_string
    });
    buffer
}

fn get_result_text(
    analyses: &RefCell<(Vec<Analysis>, Option<Analysis>)>,
    args: &RefCell<Args>,
    pwd: &RefCell<PathBuf>,
    search_text: &str,
) -> String {
    let analyses = analyses.borrow();
    let (analyses, total) = (&analyses.0, &analyses.1);
    let mut buffer = String::new();
    let args = args.borrow().clone();
    let pwd = pwd.borrow().clone();
    let analyses_count = analyses.len();

    let mut results_count = 0;
    for analysis in analyses {
        let analysis_string = analysis_to_string(
            analysis,
            args.top_words,
            args.bottom_words,
            args.hide_empty,
            search_text,
        );
        if !analysis_string.is_empty() {
            results_count += 1;
            buffer.push_str(&format!(
                "📁 File: {}\n",
                analysis
                    .file
                    .as_ref()
                    .map(|file| diff_paths(file, &pwd)
                        .unwrap_or_else(|| file.clone())
                        .display()
                        .to_string())
                    .unwrap_or_else(|| "<none>".to_string())
            ));
            buffer.push_str(&analysis_string);
            if !search_text.is_empty() {
                buffer.push('\n');
            }
            buffer.push('\n');
        }
    }

    if let Some(ref analysis) = total {
        if results_count > 1 {
            let analysis_string = analysis_to_string(
                analysis,
                args.top_words,
                args.bottom_words,
                args.hide_empty,
                search_text,
            );
            if !analysis_string.is_empty() {
                buffer.push_str(&format!("📢 Summary of {} files\n", analyses_count));
                buffer.push_str(&analysis_string);
                buffer.push('\n');
            }
            buffer.push('\n');
        }

        let analysis_string = analysis_to_string(analysis, 0, 0, args.hide_empty, search_text);
        if !analysis_string.is_empty() {
            buffer.push_str(&format!(
                "📢 Summary of {} files (all words)\n",
                analyses_count
            ));
            buffer.push_str(&analysis_string);
        }
    }

    if buffer.is_empty() {
        buffer.push_str(if search_text.is_empty() {
            "⚠️ No words in files"
        } else {
            "⚠️ No results in files"
        })
    }

    buffer.replace("\r\n", "\n").replace('\n', "\r\n")
}

impl App {
    fn setup(&self) {
        let mut icon = nwg::Icon::default();
        let _ = nwg::Icon::builder().source_bin(Some(ICON)).build(&mut icon);
        self.window.set_icon(Some(&icon));

        self.dpi.store(96, Ordering::Relaxed);
        let mut font = nwg::Font::default();
        let _ = nwg::Font::builder()
            .size(16)
            .family("Segoe UI Emoji")
            .build(&mut font);
        self.text.set_font(Some(&font));
        self.status.set_font(Some(&font));
        self.search.set_font(Some(&font));
        let _ = self.layout.fit();

        self.matchdpi();
    }

    fn resize(&self, data: &nwg::EventData) {
        let data = data.on_min_max();
        data.set_min_size(820, 620);
        self.dpitimer.start();
    }

    fn keypress(&self, data: &nwg::EventData) {
        if let nwg::EventData::OnKey(key) = data {
            if *key == nwg::keys::CONTROL {
                self.control_pressed.store(true, Ordering::Relaxed);
            }
            if *key == nwg::keys::ESCAPE {
                self.window.set_focus();
                self.progress.set_focus();
            }
            if *key == nwg::keys::_V && self.control_pressed.load(Ordering::Relaxed) {
                if let Some(text) = nwg::Clipboard::data_text(&self.window) {
                    self.start_analyze(Vec::from([AnalyzeSource::Content(text)]));
                }
            }
            if *key == nwg::keys::_F && self.control_pressed.load(Ordering::Relaxed) {
                self.search.set_focus();
            }
        }
    }
    fn keyrelease(&self, data: &nwg::EventData) {
        if let nwg::EventData::OnKey(key) = data {
            if *key == nwg::keys::CONTROL {
                self.control_pressed.store(false, Ordering::Relaxed);
            }
        }
    }

    fn search(&self) {
        let last_result_thread = self.last_result_thread.take();
        if let Some(last_result_thread) = last_result_thread {
            let _ = last_result_thread.send(true);
        }

        let tx = self.tx.clone();
        let analyses = self.analyses.clone();
        let args = self.args.clone();
        let pwd = self.pwd.clone();
        let search = self.search.text();

        let (result_tx, result_tr) = flume::bounded(1);
        self.last_result_thread.replace(Some(result_tx));

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            if let Ok(true) = result_tr.try_recv() {
                return;
            }
            let result = get_result_text(&analyses, &args, &pwd, &search);
            if let Ok(true) = result_tr.try_recv() {
                return;
            }
            let _ = tx.borrow().as_ref().unwrap().send(Message::Results(result));
        });
    }

    fn drop(&self, data: &nwg::EventData) {
        let drop = data.on_file_drop();
        self.start_analyze(
            drop.files()
                .iter()
                .filter_map(|path| canonicalize(path).ok())
                .map(AnalyzeSource::Path)
                .collect(),
        );
    }

    fn menu_settings(&self) {
        let args = self.args.borrow();
        self.menu_settings_lowercase.set_checked(args.lowercase);
        self.menu_settings_hide_empty.set_checked(args.hide_empty);
    }
    fn menu_settings_lowercase(&self) {
        {
            let mut args = self.args.borrow_mut();
            args.lowercase = !args.lowercase;
        }
        let sources = self.last_source.borrow().clone();
        self.start_analyze(sources);
    }
    fn menu_settings_hide_empty(&self) {
        {
            let mut args = self.args.borrow_mut();
            args.hide_empty = !args.hide_empty;
        }
        let sources = self.last_source.borrow().clone();
        self.start_analyze(sources);
    }

    fn timertick(&self) {
        if let Ok(thread) = self.thread.try_borrow() {
            if thread.is_none() {
                self.progress.set_state(nwg::ProgressBarState::Paused);
            }
        }
        let tr = self.tr.borrow().clone().unwrap();
        while let Ok(message) = tr.try_recv() {
            match message {
                Message::Status(message) => self.status.set_text(0, &message),
                Message::Results(results) => self.text.set_text(&results),
                Message::End => self.complete_analyze(),
            };
        }
    }

    pub fn matchdpi(&self) {
        unsafe {
            let dpi = self.dpi.load(Ordering::SeqCst);
            let hwnd = self.window.handle.hwnd().unwrap();
            let monitor = MonitorFromWindow(hwnd, 0);
            let mut x = 0;
            let mut y = 0;
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y);
            if x != dpi {
                let ratio = x as f64 / 96.0;
                let size = (12.0 * ratio).to_int_unchecked();
                self.dpi.store(x, Ordering::SeqCst);
                let mut font = nwg::Font::default();
                let _ = nwg::Font::builder()
                    .size(size)
                    .family("Segoe UI Emoji")
                    .build(&mut font);
                self.status.set_font(Some(&font));
                self.search.set_font(Some(&font));
                let _ = self.layout.fit();
                self.window.invalidate();
            }
        }
    }

    pub fn complete_analyze(&self) {
        let thread = self.thread.try_borrow_mut();
        if thread.is_err() {
            return;
        }
        let mut thread = thread.unwrap();
        if thread.is_none() {
            return;
        }
        let thread = thread.take().unwrap();

        let (analyses, total) = thread.join().unwrap();
        (*self.analyses.borrow_mut()) = (analyses, total);

        self.text.set_text(&get_result_text(
            &self.analyses,
            &self.args,
            &self.pwd,
            &self.search.text(),
        ));
        self.status.set_text(0, "");

        self.progress.set_state(nwg::ProgressBarState::Paused);
        self.search.set_enabled(true);
        self.window.invalidate();
    }

    pub fn start_analyze(&self, sources: Vec<AnalyzeSource>) {
        let thread = self.thread.try_borrow_mut();
        if thread.is_err() {
            return;
        }
        *self.last_source.borrow_mut() = sources.clone();
        let mut thread = thread.unwrap();

        self.progress.set_state(nwg::ProgressBarState::Normal);
        self.search.set_enabled(false);
        self.text.clear();
        self.window.invalidate();

        let tx = self.tx.borrow().clone().unwrap();
        let args = self.args.borrow().clone();
        let pwd = self.pwd.borrow().clone();
        *thread = Some(std::thread::spawn(move || {
            let result = analyze(
                &sources,
                &args,
                &pwd,
                |error| {
                    let _ = tx.send(Message::Status(error));
                },
                |message| {
                    let _ = tx.send(Message::Status(message));
                },
                |message| {
                    let _ = tx.send(Message::Status(message));
                },
                |_| (),
            );
            let _ = tx.send(Message::End);
            result
        }));
    }
}

fn main() {
    let mut console = std::ptr::null::<HWND>() as HWND;
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            console = GetConsoleWindow();
        }
    }

    nwg::init().expect("Failed to init Native Windows GUI");
    let _ = nwg::Font::set_global_family("Segoe UI");
    let mut font = nwg::Font::default();
    let _ = nwg::Font::builder()
        .size(16)
        .family("Segoe UI Emoji")
        .build(&mut font);
    let _ = nwg::Font::set_global_default(Some(font));

    let app = App::build_ui(Default::default()).expect("Failed to build UI");
    let (tx, tr) = flume::unbounded();
    (*app.tx.borrow_mut()) = Some(tx);
    (*app.tr.borrow_mut()) = Some(tr);
    (*app.args.borrow_mut()) = Args {
        lowercase: false,
        top_words: 10,
        bottom_words: 3,
        recursive: true,
        follow_symlinks: false,
        hide_empty: true,
        outfile: None,
    };
    (*app.pwd.borrow_mut()) =
        canonicalize(std::env::current_dir().unwrap_or_else(|_| PathBuf::new()))
            .unwrap_or_else(|_| PathBuf::new());

    let args = std::env::args();
    if args.len() > 1 {
        app.start_analyze(
            args.collect::<Vec<String>>()[1..]
                .iter()
                .map(|path| AnalyzeSource::Path(path.into()))
                .collect(),
        );
    } else {
        app.progress.set_state(nwg::ProgressBarState::Paused);
    }

    nwg::dispatch_thread_events();

    unsafe {
        if !console.is_null() {
            let _ = std::io::stdout().flush();
        }
        FreeConsole();
    }
}
