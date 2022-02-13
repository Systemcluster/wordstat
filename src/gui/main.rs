#![windows_subsystem = "windows"]
#![feature(async_closure)]
#![feature(once_cell)]
#![feature(slice_take)]
#![feature(const_mut_refs)]
#![feature(thread_is_running)]
#![feature(try_blocks)]

use native_windows_derive as nwd;
use native_windows_gui as nwg;

mod report;
#[path = "../shared/mod.rs"]
mod shared;

use std::{
    cell::RefCell,
    fs::canonicalize,
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

use nwd::NwgUi;
use nwg::stretch::{
    geometry::{Rect, Size},
    style::{AlignItems, AlignSelf, Dimension as D, FlexDirection, JustifyContent},
};
use nwg::NativeUi;

use winapi::{
    shared::windef::HWND,
    um::{
        shellscalingapi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        wincon::{AttachConsole, FreeConsole, GetConsoleWindow, ATTACH_PARENT_PROCESS},
        winuser::MonitorFromWindow,
    },
};

use report::*;
use shared::{analyze, Analysis, AnalyzeSource, Args};

static ICON: &[u8] = include_bytes!("../../resources/book.ico");

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
    Waiting,
    Analyses((Vec<Analysis>, Option<Analysis>)),
    Results((String, String)),
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
        text: "&Lowercase words",
        parent: menu_settings,
        check: false
    )]
    #[nwg_events(
        OnMenuItemSelected: [App::menu_settings_lowercase],
    )]
    menu_settings_lowercase: nwg::MenuItem,
    #[nwg_control(
        text: "&Hide empty sources",
        parent: menu_settings,
        check: false
    )]
    #[nwg_events(
        OnMenuItemSelected: [App::menu_settings_hide_empty],
    )]
    menu_settings_hide_empty: nwg::MenuItem,
    #[nwg_control(
        text: "Show matching &emojis",
        parent: menu_settings,
        check: false
    )]
    #[nwg_events(
        OnMenuItemSelected: [App::menu_settings_emojis],
    )]
    menu_settings_emojis: nwg::MenuItem,
    #[nwg_control(
        text: "Show summary with &all words",
        parent: menu_settings,
        check: false
    )]
    #[nwg_events(
        OnMenuItemSelected: [App::menu_settings_all_words],
    )]
    menu_settings_all_words: nwg::MenuItem,

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
    analyses: RefCell<(Vec<Analysis>, Option<Analysis>)>,

    args: RefCell<Args>,
    pwd: RefCell<PathBuf>,
    last_source: RefCell<Vec<AnalyzeSource>>,

    dpi: Arc<AtomicU32>,
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
            if !self.search.focus()
                && *key == nwg::keys::_V
                && self.control_pressed.load(Ordering::Relaxed)
            {
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
        let search_text = self.search.text();

        let (result_tx, result_tr) = flume::bounded(1);
        self.last_result_thread.replace(Some(result_tx));

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            if let Ok(true) = result_tr.try_recv() {
                return;
            }
            let _ = tx.borrow().as_ref().unwrap().send(Message::Waiting);
            let result = get_result_text(&analyses.borrow(), &args, &pwd, &search_text);
            if let Ok(true) = result_tr.try_recv() {
                return;
            }
            let result = result.unwrap_or_else(|e| "⚠️ ".to_string() + &e.to_string());
            let _ = tx
                .borrow()
                .as_ref()
                .unwrap()
                .send(Message::Results((result, search_text)));
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
        self.menu_settings_all_words
            .set_checked(args.show_all_words);
        self.menu_settings_emojis.set_checked(args.emojis);
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
    fn menu_settings_all_words(&self) {
        {
            let mut args = self.args.borrow_mut();
            args.show_all_words = !args.show_all_words;
        }
        let sources = self.last_source.borrow().clone();
        self.start_analyze(sources);
    }
    fn menu_settings_emojis(&self) {
        {
            let mut args = self.args.borrow_mut();
            args.emojis = !args.emojis;
        }
        let sources = self.last_source.borrow().clone();
        self.start_analyze(sources);
    }

    fn timertick(&self) {
        let tr = self.tr.borrow().clone().unwrap();
        while let Ok(message) = tr.try_recv() {
            match message {
                Message::Status(message) => self.status.set_text(0, &message),
                Message::Analyses(analyses) => {
                    self.analyses.replace(analyses);
                }
                Message::Waiting => self.progress.set_state(nwg::ProgressBarState::Normal),
                Message::Results((results, search_text)) => {
                    if search_text == self.search.text() {
                        self.text.set_text(&results);
                        self.progress.set_state(nwg::ProgressBarState::Paused);
                        self.search.set_enabled(true);
                        self.status.set_text(0, "");
                        self.window.invalidate();
                    }
                }
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

    pub fn start_analyze(&self, sources: Vec<AnalyzeSource>) {
        *self.last_source.borrow_mut() = sources.clone();

        self.progress.set_state(nwg::ProgressBarState::Normal);
        self.search.set_enabled(false);
        self.text.clear();
        self.window.invalidate();

        let tx = self.tx.borrow().clone().unwrap();
        let args = self.args.clone();
        let pwd = self.pwd.clone();
        let search_text = self.search.text();
        std::thread::spawn(move || {
            let mut analyses = analyze(
                &sources,
                &args.borrow(),
                &pwd.borrow(),
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
            analyses.0.sort_by_key(|analysis| analysis.file.clone());
            let _ = tx.send(Message::Status("Generating report...".to_owned()));
            let _ = tx.send(Message::Analyses(analyses.clone()));
            let _ = tx.send(Message::Results((
                get_result_text(&analyses, &args, &pwd, &search_text)
                    .unwrap_or_else(|e| "⚠️ ".to_string() + &e.to_string()),
                search_text,
            )));
        });
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
    app.window
        .set_text(&(app.window.text() + " " + env!("CARGO_PKG_VERSION")));
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
        emojis: false,
        show_all_words: true,
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
