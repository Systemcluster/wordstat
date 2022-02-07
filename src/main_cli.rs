#![feature(async_closure)]
#![feature(once_cell)]
#![feature(slice_take)]
#![feature(const_mut_refs)]

mod shared;
mod uhash;
mod ustring;

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

fn print_analysis(analysis: &Analysis, top_words: usize, bottom_words: usize) {
    if analysis.word_freq.is_empty() {
        eprintln!("{}{}", Emoji("⚠️ ", ""), style("No words in file").red());
        return;
    }
    let pad = format!("{}", analysis.word_freq[0].0).len();
    println!(
        "{}Word count: {}",
        Emoji("🔢 ", ""),
        style(&format!("{}", analysis.word_count)).blue()
    );
    println!("{}Top words:", Emoji("📈 ", ""));
    for (i, (freq, string)) in analysis.word_freq.iter().enumerate() {
        if top_words > 0 && i >= top_words {
            break;
        };
        println!(
            "  {}: {}",
            style(&format!("{:width$}", freq, width = pad))
                .bold()
                .blue(),
            style(string).green(),
        );
    }
    if bottom_words > 0 && top_words != 0 {
        println!("  ...");
        for (i, (freq, string)) in analysis.word_freq.iter().rev().enumerate() {
            if bottom_words > 0 && i >= bottom_words {
                break;
            };
            println!(
                "  {}: {}",
                style(&format!("{:width$}", freq, width = pad))
                    .bold()
                    .blue(),
                style(string).green(),
            );
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
        outfile: args.outfile,
    };

    let (analyses, total) = analyze(
        &paths
            .iter()
            .map(|path| AnalyzeSource::Path(path.to_owned()))
            .collect(),
        &args,
        &pwd,
        |error| eprintln!("{}", error),
        |message| bar_progress.println(message),
        |message| bar_progress.set_message(message),
        |delta| bar_progress.inc(delta),
    );
    let analyses_count = analyses.len();

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
        print_analysis(analysis, args.top_words, args.bottom_words);
    }

    let mut analysis = total;
    if analyses_count > 1 {
        analysis
            .word_freq_map
            .iter()
            .for_each(|(path, count)| analysis.word_freq.push((*count, *path)));
        analysis.word_freq.sort_by(|(a, _), (b, _)| b.cmp(a));

        println!();
        println!(
            "{}{} {} {}",
            Emoji("📢 ", ""),
            style("Summary of").yellow(),
            style(&format!("{}", analyses_count)).bold().magenta(),
            style("files").yellow()
        );
        print_analysis(&analysis, args.top_words, args.bottom_words);
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
