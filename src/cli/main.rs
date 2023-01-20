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
    time::Duration,
};

use clap::{ErrorKind, IntoApp, Parser};
use console::{style, Emoji};
use indicatif::{ProgressBar, ProgressStyle};
use pathdiff::diff_paths;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use regex::{Regex, RegexBuilder};

use crate::shared::{analyze, Analysis, AnalyzeSource, Args};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct CliArgs {
    /// Path to one or multiple files or directories of files to analyze
    path: Vec<String>,
    /// Normalize casing by lowercasing each occuring word
    #[clap(short, long)]
    lowercase: bool,
    /// Number of top words to show per file (0 = all)
    #[clap(short, long, default_value_t = 10)]
    top_words: usize,
    /// Number of least occuring words to show per file
    #[clap(short, long, default_value_t = 3)]
    bottom_words: usize,
    /// Show matching emojis for words
    #[clap(short, long)]
    emojis: bool,
    /// Print combined analysis with all words found in files
    #[clap(short, long)]
    show_all_words: bool,
    /// Filter printed words by string or regex
    #[clap(short, long)]
    word_filter: Option<String>,
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

fn print_analysis(
    analysis: &Analysis,
    top_words: usize,
    bottom_words: usize,
    emojis: bool,
    regex: &Option<Regex>,
) -> (usize, usize) {
    if analysis.word_freq.is_empty() {
        eprintln!("{}{}", Emoji("⚠️ ", ""), style("No words in file").red());
        return (0, 0);
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

    let filtered_word_count = if let Some(regex) = regex {
        analysis
            .word_freq
            .par_iter()
            .filter(|(_, word)| regex.is_match(word))
            .count()
    } else {
        analysis.word_freq.len()
    };
    if filtered_word_count == 0 {
        eprintln!(
            "{}{}",
            Emoji("⚠️ ", ""),
            style("No words in file matching filter").red()
        );
        return (0, 0);
    } else {
        println!(
            "{}Words matching filter: {}",
            Emoji("🔎 ", ""),
            style(&format!("{}", filtered_word_count)).blue()
        );
    }

    println!(
        "{}Top words{}",
        Emoji("📈 ", ""),
        if regex.is_some() { " (filtered):" } else { ":" }
    );
    let regex = regex.as_ref();
    let pad = format!("{}", analysis.word_freq[0].0).len();
    let mut printed_top = 0;
    for (freq, string) in analysis.word_freq.iter() {
        if top_words > 0 && printed_top >= top_words {
            break;
        };
        if regex.is_none() || regex.unwrap().is_match(string) {
            printed_top += 1;
            print!(
                "  {}: {}",
                style(&format!("{:width$}", freq, width = pad))
                    .bold()
                    .blue(),
                style(string).green(),
            );
            if emojis {
                if let Some(e) = emojis::get_by_shortcode(&string.to_lowercase()) {
                    print!(" {}", e);
                }
            }
            println!();
        }
    }

    let mut printed_bottom = 0;
    if bottom_words > 0 && top_words != 0 && printed_top < filtered_word_count {
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
        println!(
            "{}Bottom words{}",
            Emoji("📉 ", ""),
            if regex.is_some() { " (filtered):" } else { ":" }
        );

        for (freq, string) in analysis.word_freq.iter().rev() {
            if bottom_words > 0 && printed_bottom >= bottom_words {
                break;
            };
            if regex.is_none() || regex.unwrap().is_match(string) {
                printed_bottom += 1;
                print!(
                    "  {}: {}",
                    style(&format!("{:width$}", freq, width = pad))
                        .bold()
                        .blue(),
                    style(string).green(),
                );
                if emojis {
                    if let Some(e) = emojis::get_by_shortcode(&string.to_lowercase()) {
                        print!(" {}", e);
                    }
                }
                println!();
            }
        }
    }

    (printed_top + printed_bottom, filtered_word_count)
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

    let word_filter = args.word_filter.as_ref();
    let regex = match word_filter {
        None => None,
        Some(search_text) if search_text.is_empty() => None,
        Some(search_text) => {
            let search_is_regex = search_text.len() >= 3
                && search_text.starts_with('/')
                && (search_text.ends_with('/') || search_text.ends_with("/i"));
            let is_insensitive = search_text.ends_with("/i");
            if search_is_regex {
                Some(
                    RegexBuilder::new(
                        &search_text[1..search_text.len() - if is_insensitive { 2 } else { 1 }],
                    )
                    .case_insensitive(search_text.ends_with("/i"))
                    .multi_line(false)
                    .build()
                    .unwrap_or_else(|error| {
                        app.lock()
                            .unwrap()
                            .error(
                                ErrorKind::Io,
                                format!("Could not create filter regex: {}", error),
                            )
                            .exit()
                    }),
                )
            } else {
                Some(
                    RegexBuilder::new(&regex::escape(search_text))
                        .case_insensitive(true)
                        .multi_line(false)
                        .build()
                        .unwrap_or_else(|error| {
                            app.lock()
                                .unwrap()
                                .error(
                                    ErrorKind::Io,
                                    format!("Could not create filter regex: {}", error),
                                )
                                .exit()
                        }),
                )
            }
        }
    };

    println!(
        "{}checking {} {}",
        Emoji("🔍 ", ""),
        paths.len(),
        if paths.len() > 1 { "paths" } else { "path" }
    );

    let bar_progress =
        ProgressBar::new(0).with_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} {elapsed_precise} [{wide_bar:.green}] {pos}/{len}\n{spinner:.green} {wide_msg}")
                .unwrap(),
        );
    bar_progress.set_length(paths.len() as u64);
    bar_progress.set_position(0);
    bar_progress.enable_steady_tick(Duration::from_millis(12));

    let args = Args {
        lowercase: args.lowercase,
        top_words: args.top_words,
        bottom_words: args.bottom_words,
        recursive: args.recursive,
        follow_symlinks: args.follow_symlinks,
        hide_empty: false,
        outfile: args.outfile,
        emojis: args.emojis,
        show_all_words: args.show_all_words,
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
        print_analysis(
            analysis,
            args.top_words,
            args.bottom_words,
            args.emojis,
            &regex,
        );
    }

    if let Some(analysis) = total {
        let (printed_total, filtered_word_count) = if analyses_count > 1 {
            println!();
            println!(
                "{}{} {} {}",
                Emoji("📢 ", ""),
                style("Summary of").yellow(),
                style(&format!("{}", analyses_count)).bold().magenta(),
                style("files").yellow()
            );
            print_analysis(
                &analysis,
                args.top_words,
                args.bottom_words,
                args.emojis,
                &regex,
            )
        } else {
            (0, 0)
        };
        if args.show_all_words && (printed_total < filtered_word_count || analyses_count == 1) {
            println!();
            println!(
                "{}{} {} {}",
                Emoji("📢 ", ""),
                style("Summary of").yellow(),
                style(&format!("{}", analyses_count)).bold().magenta(),
                style("files (all words)").yellow()
            );
            print_analysis(&analysis, 0, 0, args.emojis, &regex);
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
