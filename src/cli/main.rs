#![feature(async_closure)]
#![feature(once_cell)]
#![feature(slice_take)]
#![feature(const_mut_refs)]

#[path = "../shared/mod.rs"]
mod shared;

use std::{
    fs::{canonicalize, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use clap::{ErrorKind, IntoApp, Parser};
use console::{style, Emoji};
use indicatif::{ProgressBar, ProgressStyle};
use pathdiff::diff_paths;

use crate::shared::{analyze, Analysis, AnalyzeSource, Args};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct CliArgs {
    /// Path to one or multiple files or directories of files to analyze
    path: Vec<String>,
    /// Normalize casing by lowercasing each occuring word
    #[clap(short, long)]
    lowercase: bool,
    /// Number of top words to show (0 = all)
    #[clap(short, long, default_value_t = 10)]
    top_words: usize,
    /// Number of least occuring words to show
    #[clap(short, long, default_value_t = 3)]
    bottom_words: usize,
    /// Show matching emojis for words
    #[clap(short, long)]
    emojis: bool,
    /// Iterate through subdirectories
    #[clap(short, long)]
    recursive: bool,
    /// Follow symlinks
    #[clap(short, long)]
    follow_symlinks: bool,
    /// The path to a file that the results will be written to, will overwrite if it already exists
    #[clap(short, long)]
    outfile: Option<String>,
}

fn print_analysis(analysis: &Analysis, top_words: usize, bottom_words: usize, emojis: bool) {
    if analysis.word_freq.is_empty() {
        eprintln!("{}{}", Emoji("⚠️ ", ""), style("No words in file").red());
        return;
    }
    println!(
        "{}Word count: {}",
        Emoji("🔢 ", ""),
        style(&format!("{}", analysis.word_count)).blue()
    );
    println!(
        "{}Sentence count: {}",
        Emoji("🔢 ", ""),
        style(&format!("{}", analysis.sent_count)).blue()
    );
    println!(
        "{}Character count: {}",
        Emoji("🔢 ", ""),
        style(&format!("{}", analysis.char_count)).blue()
    );
    println!(
        "{}Paragraph count: {}",
        Emoji("🔢 ", ""),
        style(&format!("{}", analysis.para_count)).blue()
    );
    println!(
        "{}Unique words: {}",
        Emoji("🔢 ", ""),
        style(&format!("{}", analysis.word_uniqs)).blue()
    );
    println!(
        "{}Word frequency mean: {}",
        Emoji("📊 ", ""),
        style(&format!("{:.2}", analysis.word_dist_mean)).blue()
    );
    println!(
        "{}Word frequency standard deviation: {}",
        Emoji("📊 ", ""),
        style(&format!("{:.2}", analysis.word_dist_stddev)).blue()
    );
    println!(
        "{}Word frequency median: {}",
        Emoji("📊 ", ""),
        style(&format!("{:.1}", analysis.word_dist_median)).blue()
    );
    println!(
        "{}Word frequency mode: {}",
        Emoji("📊 ", ""),
        style(&format!("{:.1}", analysis.word_dist_mode)).blue()
    );
    println!("{}Top words:", Emoji("📈 ", ""));
    let pad = format!("{}", analysis.word_freq[0].0).len();
    for (i, (freq, string)) in analysis.word_freq.iter().enumerate() {
        if top_words > 0 && i >= top_words {
            break;
        };
        print!(
            "  {}: {}",
            style(&format!("{:width$}", freq, width = pad))
                .bold()
                .blue(),
            style(string).green(),
        );
        if emojis {
            if let Some(e) = emojis::lookup(&string.to_lowercase()) {
                print!(" {}", e);
            }
        }
        println!();
    }
    if bottom_words > 0 && top_words != 0 && top_words < analysis.word_count {
        let pad = format!(
            "{}",
            analysis
                .word_freq
                .iter()
                .nth_back(0)
                .map(|n| n.0)
                .unwrap_or(0)
        )
        .len();
        println!("{}Bottom words:", Emoji("📉 ", ""));
        for (i, (freq, string)) in analysis.word_freq.iter().rev().enumerate() {
            if bottom_words > 0 && i >= bottom_words {
                break;
            };
            print!(
                "  {}: {}",
                style(&format!("{:width$}", freq, width = pad))
                    .bold()
                    .blue(),
                style(string).green(),
            );
            if emojis {
                if let Some(e) = emojis::lookup(&string.to_lowercase()) {
                    print!(" {}", e);
                }
            }
            println!();
        }
    }
}

fn print_analysis_file(analysis: &Analysis, path: &Path) {
    let file = File::create(path);
    if let Err(error) = file {
        eprintln!(
            "{}{} {}: {}",
            Emoji("⚠️ ", ""),
            style("Could not open output file").red(),
            style(&path.display()).blue(),
            style(&error).red()
        );
        return;
    };
    let file = file.unwrap();
    let mut writer = BufWriter::new(file);
    for (freq, string) in analysis.word_freq.iter() {
        writeln!(writer, "{}: {}", string, freq).unwrap_or_else(|error| {
            eprintln!("{}{}", Emoji("⚠️ ", ""), style(&error).red());
        });
    }
}

fn main() {
    let args = CliArgs::parse_from(wild::args());
    let app = Arc::new(Mutex::new(CliArgs::into_app()));

    let pwd = canonicalize(std::env::current_dir().unwrap_or_else(|error| {
        app.lock()
            .unwrap()
            .error(ErrorKind::Io, format!("{}", error))
            .exit()
    }))
    .unwrap_or_else(|error| {
        app.lock()
            .unwrap()
            .error(ErrorKind::Io, format!("{}", error))
            .exit()
    });

    let mut paths = Vec::new();
    for path in &args.path {
        paths.push(canonicalize(path).unwrap_or_else(|error| {
            app.lock()
                .unwrap()
                .error(
                    ErrorKind::Io,
                    format!("Could not resolve {}: {}", path, error),
                )
                .exit()
        }))
    }

    if paths.is_empty() {
        app.lock()
            .unwrap()
            .error(ErrorKind::InvalidValue, "No files or directories specified")
            .exit()
    }

    println!(
        "{}checking {} {}",
        Emoji("🔍 ", ""),
        paths.len(),
        if paths.len() > 1 { "paths" } else { "path" }
    );

    let bar_progress =
        ProgressBar::new(0).with_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} {elapsed_precise} [{wide_bar:.green}] {pos}/{len}\n{spinner:.green} {wide_msg}"),
        );
    bar_progress.set_length(paths.len() as u64);
    bar_progress.set_position(0);
    bar_progress.enable_steady_tick(12);

    let args = Args {
        lowercase: args.lowercase,
        top_words: args.top_words,
        bottom_words: args.bottom_words,
        recursive: args.recursive,
        follow_symlinks: args.follow_symlinks,
        hide_empty: false,
        outfile: args.outfile,
        emojis: args.emojis,
    };

    let (mut analyses, total) = analyze(
        &paths
            .iter()
            .map(|path| AnalyzeSource::Path(path.to_owned()))
            .collect(),
        &args,
        &pwd,
        |error| eprintln!("{}{}", Emoji("⚠️ ", ""), style(&error).red()),
        |message| bar_progress.println(message),
        |message| bar_progress.set_message(message),
        |delta| bar_progress.inc(delta),
    );
    let analyses_count = analyses.len();
    analyses.sort_by_key(|analysis| analysis.file.clone());

    bar_progress.finish_and_clear();

    for analysis in analyses.iter() {
        println!();
        println!(
            "{}File: {}",
            Emoji("📁 ", ""),
            style(
                analysis
                    .file
                    .as_ref()
                    .map(|file| diff_paths(file, &pwd)
                        .unwrap_or_else(|| file.clone())
                        .display()
                        .to_string())
                    .unwrap_or_else(|| "<none>".to_string())
            )
            .blue()
        );
        print_analysis(analysis, args.top_words, args.bottom_words, args.emojis);
    }

    if let Some(analysis) = total {
        if analyses_count > 1 {
            println!();
            println!(
                "{}{} {} {}",
                Emoji("📢 ", ""),
                style("Summary of").yellow(),
                style(&format!("{}", analyses_count)).bold().magenta(),
                style("files").yellow()
            );
            print_analysis(&analysis, args.top_words, args.bottom_words, args.emojis);
        }

        if let Some(path) = args.outfile {
            println!();
            let outfile = PathBuf::from(&path);
            println!(
                "{}Writing results to {}",
                Emoji("🖥️ ", ""),
                style(
                    diff_paths(&outfile, &pwd)
                        .unwrap_or_else(|| outfile.clone())
                        .display()
                )
                .blue()
            );
            print_analysis_file(&analysis, &outfile);
        }
    }
}
