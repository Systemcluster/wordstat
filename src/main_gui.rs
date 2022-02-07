#![windows_subsystem = "windows"]
#![feature(async_closure)]
#![feature(once_cell)]
#![feature(slice_take)]
#![feature(const_mut_refs)]
#![feature(thread_is_running)]

use native_windows_derive as nwd;
use native_windows_gui as nwg;

mod shared;
mod uhash;
mod ustring;

use std::fs::canonicalize;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use nwd::NwgUi;
use nwg::stretch::{
    geometry::{Rect, Size},
    style::{AlignItems, AlignSelf, Dimension as D, FlexDirection, JustifyContent},
};
use nwg::EventData::OnKey;
use nwg::NativeUi;
use nwg::{Font, Icon};
use pathdiff::diff_paths;

use shared::{analyze, AnalyzeSource, Args};

use self::shared::Analysis;

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

// use crate::shared::{analyze, Analysis, Args};

#[derive(Default, NwgUi)]
pub struct App {
    #[nwg_control(
        title: "WordStat",
        accept_files: true,
        center: true,
        size: (820, 620)
    )]
    #[nwg_events(
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
        marquee_update: 20,
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
        text: "Drop a file here to analyze it",
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

    control_pressed: Arc<AtomicBool>,
}

impl App {
    fn resize(&self, data: &nwg::EventData) {
        let data = data.on_min_max();
        data.set_min_size(820, 620);
    }

    pub fn keypress(&self, data: &nwg::EventData) {
        if let OnKey(key) = data {
            if *key == nwg::keys::CONTROL {
                self.control_pressed.store(true, Ordering::Relaxed);
            }
            if *key == nwg::keys::_V && self.control_pressed.load(Ordering::Relaxed) {
                if let Some(text) = nwg::Clipboard::data_text(&self.window) {
                    self.analyze(Vec::from([AnalyzeSource::Content(text)]));
                }
            }
        }
    }
    pub fn keyrelease(&self, data: &nwg::EventData) {
        if let OnKey(key) = data {
            if *key == nwg::keys::CONTROL {
                self.control_pressed.store(false, Ordering::Relaxed);
            }
        }
    }

    pub fn drop(&self, data: &nwg::EventData) {
        let drop = data.on_file_drop();
        self.analyze(
            drop.files()
                .iter()
                .map(|path| AnalyzeSource::Path(path.into()))
                .collect(),
        );
    }

    pub fn analyze(&self, sources: Vec<AnalyzeSource>) {
        self.progress.set_state(nwg::ProgressBarState::Normal);
        self.window.invalidate();

        enum Message {
            Text(String),
            End,
        }

        let args = Args {
            lowercase: false,
            top_words: 10,
            bottom_words: 3,
            recursive: true,
            follow_symlinks: false,
            outfile: None,
        };

        let pwd = canonicalize(std::env::current_dir().unwrap_or_else(|_| PathBuf::new()))
            .unwrap_or_else(|_| PathBuf::new());
        let (tx, tr) = flume::unbounded();
        let _args = args.clone();
        let _pwd = pwd.clone();
        let thread = std::thread::spawn(move || {
            let result = analyze(
                &sources,
                &_args,
                &_pwd,
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
        });

        loop {
            if !thread.is_running() {
                break;
            }
            if let Ok(message) = tr.try_recv() {
                match message {
                    Message::Text(message) => self.status.set_text(0, &message),
                    Message::End => break,
                };
            }
            self.window.invalidate();
        }

        let (analyses, total) = thread.join().unwrap();
        let analyses_count = analyses.len();

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
    nwg::init().expect("Failed to init Native Windows GUI");
    let _ = nwg::Font::set_global_family("Segoe UI");
    let app = App::build_ui(Default::default()).expect("Failed to build UI");
    let mut icon = Icon::default();
    let _ = Icon::builder().source_bin(Some(ICON)).build(&mut icon);
    app.window.set_icon(Some(&icon));
    let mut font = Font::default();
    let _ = Font::builder()
        .size(28)
        .family("Segoe UI Emoji")
        .build(&mut font);
    app.text.set_font(Some(&font));

    let args = std::env::args();
    app.text.set_text(&format!("{:?}", args));
    if args.len() > 1 {
        app.analyze(
            args.collect::<Vec<String>>()[1..]
                .iter()
                .map(|path| AnalyzeSource::Path(path.into()))
                .collect(),
        );
    }

    nwg::dispatch_thread_events();
}
