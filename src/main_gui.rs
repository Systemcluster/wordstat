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
        atomic::{AtomicBool, Ordering},
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
    um::wincon::{AttachConsole, FreeConsole, GetConsoleWindow, ATTACH_PARENT_PROCESS},
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
const PT_20: D = D::Points(20.0);
const RECT_100: Rect<D> = Rect {
    start: PT_10,
    end: PT_10,
    top: PT_10,
    bottom: PT_0,
};
const RECT_102: Rect<D> = Rect {
    start: PT_10,
    end: PT_10,
    top: PT_10,
    bottom: PT_20,
};

enum Message {
    Text(String),
    End,
}

#[derive(Default, NwgUi)]
pub struct App {
    #[nwg_control(
        title: "WordStat",
        accept_files: true,
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

    #[nwg_layout(
        parent: window,
        flex_direction: FlexDirection::Column,
        padding: RECT_0,
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
        margin: RECT_100,
        flex_grow: 0.1,
        min_size: Size { width: D::Auto, height: D::Points(20.0) },
    )]
    progress: nwg::ProgressBar,

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
        flex_grow: 0.1,
        min_size: Size { width: D::Auto, height: D::Points(20.0) },
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

    tx: RefCell<Option<flume::Sender<Message>>>,
    tr: RefCell<Option<flume::Receiver<Message>>>,
    control_pressed: Arc<AtomicBool>,
    #[allow(clippy::type_complexity)]
    thread: RefCell<Option<JoinHandle<(Vec<Analysis>, Analysis)>>>,

    args: RefCell<Args>,
    pwd: RefCell<PathBuf>,
}

impl App {
    fn setup(&self) {
        let mut icon = nwg::Icon::default();
        let _ = nwg::Icon::builder().source_bin(Some(ICON)).build(&mut icon);
        self.window.set_icon(Some(&icon));

        let mut font = nwg::Font::default();
        let _ = nwg::Font::builder()
            .size(28)
            .family("Segoe UI Emoji")
            .build(&mut font);
        self.text.set_font(Some(&font));

        self.progress.set_state(nwg::ProgressBarState::Paused);

        let args = std::env::args();
        if args.len() > 1 {
            self.start_analyze(
                args.collect::<Vec<String>>()[1..]
                    .iter()
                    .map(|path| AnalyzeSource::Path(path.into()))
                    .collect(),
            );
        }
    }

    fn resize(&self, data: &nwg::EventData) {
        let data = data.on_min_max();
        data.set_min_size(820, 620);
    }

    fn keypress(&self, data: &nwg::EventData) {
        if let nwg::EventData::OnKey(key) = data {
            if *key == nwg::keys::CONTROL {
                self.control_pressed.store(true, Ordering::Relaxed);
            }
            if *key == nwg::keys::_V && self.control_pressed.load(Ordering::Relaxed) {
                if let Some(text) = nwg::Clipboard::data_text(&self.window) {
                    self.start_analyze(Vec::from([AnalyzeSource::Content(text)]));
                }
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

    fn timertick(&self) {
        if let Ok(thread) = self.thread.try_borrow() {
            if thread.is_none() {
                self.progress.set_state(nwg::ProgressBarState::Paused);
            }
        }
        let tr = self.tr.borrow().clone().unwrap();
        while let Ok(message) = tr.try_recv() {
            match message {
                Message::Text(message) => self.status.set_text(0, &message),
                Message::End => self.complete_analyze(),
            };
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
        let analyses_count = analyses.len();

        let args = self.args.borrow().clone();
        let pwd = self.pwd.borrow().clone();

        let mut buffer = String::new();

        for analysis in analyses {
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
            buffer.push_str(&format!("🔢 Word count: {}\n", analysis.word_count));
            buffer.push_str(&format!("🔢 Sentence count: {}\n", analysis.sent_count));
            buffer.push_str(&format!("🔢 Character count: {}\n", analysis.char_count));
            buffer.push_str(&format!("🔢 Paragraph count: {}\n", analysis.para_count));
            buffer.push_str("📈 Top words:\n");
            buffer.push_str(&analysis_to_string(
                &analysis,
                args.top_words,
                args.bottom_words,
            ));
            buffer.push('\n');
        }
        let mut analysis = total;
        analysis
            .word_freq_map
            .iter()
            .for_each(|(path, count)| analysis.word_freq.push((*count, *path)));
        analysis.word_freq.sort_by(|(a, _), (b, _)| b.cmp(a));

        if analyses_count > 1 {
            buffer.push_str(&format!("📢 Summary of {} files\n", analyses_count));
            buffer.push_str(&format!("🔢 Word count: {}\n", analysis.word_count));
            buffer.push_str(&format!("🔢 Sentence count: {}\n", analysis.sent_count));
            buffer.push_str(&format!("🔢 Character count: {}\n", analysis.char_count));
            buffer.push_str(&format!("🔢 Paragraph count: {}\n", analysis.para_count));
            buffer.push_str("📈 Top words:\n");
            buffer.push_str(&analysis_to_string(
                &analysis,
                args.top_words,
                args.bottom_words,
            ));
        }

        buffer.push('\n');
        buffer.push_str(&format!(
            "📢 Summary of {} files (all words)\n",
            analyses_count
        ));
        buffer.push_str(&format!("🔢 Word count: {}\n", analysis.word_count));
        buffer.push_str(&format!("🔢 Sentence count: {}\n", analysis.sent_count));
        buffer.push_str(&format!("🔢 Character count: {}\n", analysis.char_count));
        buffer.push_str(&format!("🔢 Paragraph count: {}\n", analysis.para_count));
        buffer.push_str("📈 Top words:\n");
        buffer.push_str(&analysis_to_string(&analysis, 0, 0));

        self.text.set_text(&buffer);
        self.progress.set_state(nwg::ProgressBarState::Paused);
    }

    pub fn start_analyze(&self, sources: Vec<AnalyzeSource>) {
        let thread = self.thread.try_borrow_mut();
        if thread.is_err() {
            return;
        }
        let mut thread = thread.unwrap();

        self.progress.set_state(nwg::ProgressBarState::Normal);
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
                    let _ = tx.send(Message::Text(error));
                },
                |message| {
                    let _ = tx.send(Message::Text(message));
                },
                |message| {
                    let _ = tx.send(Message::Text(message));
                },
                |_| (),
            );
            let _ = tx.send(Message::End);
            result
        }));
    }
}

fn analysis_to_string(analysis: &Analysis, top_words: usize, bottom_words: usize) -> String {
    if analysis.word_freq.is_empty() {
        return "No words in file".to_owned();
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
    if bottom_words > 0 && top_words != 0 {
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

fn main() {
    let mut console = std::ptr::null::<HWND>() as HWND;
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            console = GetConsoleWindow();
        }
    }

    nwg::init().expect("Failed to init Native Windows GUI");
    let _ = nwg::Font::set_global_family("Segoe UI");

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
        outfile: None,
    };
    (*app.pwd.borrow_mut()) =
        canonicalize(std::env::current_dir().unwrap_or_else(|_| PathBuf::new()))
            .unwrap_or_else(|_| PathBuf::new());

    nwg::dispatch_thread_events();

    unsafe {
        if !console.is_null() {
            let _ = std::io::stdout().flush();
        }
        FreeConsole();
    }
}
