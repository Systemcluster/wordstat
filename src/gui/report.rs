use std::{cell::RefCell, path::PathBuf};

use anyhow::Result;
use pathdiff::diff_paths;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use regex::{Regex, RegexBuilder};

use super::shared::{Analysis, Args};

pub fn analysis_words_to_string(
    analysis: &Analysis,
    top_words: usize,
    bottom_words: usize,
    emojis: bool,
) -> (String, String) {
    if analysis.word_freq.is_empty() {
        return ("".to_owned(), "".to_owned());
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
        if emojis {
            if let Some(e) = emojis::get_by_shortcode(&string.to_lowercase()) {
                buffer.push_str(format!(" {}", e).trim());
            }
        }
        buffer.push('\n');
    }
    let mut buffer_bottom = String::new();
    if bottom_words > 0
        && top_words != 0
        && top_words < analysis.word_count
        && top_words < analysis.word_freq.len()
    {
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
        for (i, (freq, string)) in analysis.word_freq.iter().rev().enumerate() {
            if bottom_words > 0 && i >= bottom_words {
                break;
            };
            buffer_bottom.push_str(&format!("  {:width$}", freq, width = pad));
            buffer_bottom.push_str(": ");
            buffer_bottom.push_str(string);

            if emojis {
                if let Some(e) = emojis::get_by_shortcode(&string.to_lowercase()) {
                    buffer_bottom.push_str(format!(" {}", e).trim());
                }
            }
            buffer_bottom.push('\n');
        }
    }
    (buffer, buffer_bottom)
}

pub fn analysis_to_string(
    analysis: &Analysis,
    top_words: usize,
    bottom_words: usize,
    hide_empty: bool,
    search_regex: &Option<Regex>,
    emojis: bool,
) -> (String, usize, usize) {
    let mut buffer = String::new();
    let filtered_word_count;
    let (analysis_string, analysis_string_bottom) = if search_regex.is_none() {
        filtered_word_count = analysis.word_freq.len();
        analysis_words_to_string(analysis, top_words, bottom_words, emojis)
    } else {
        let regex = search_regex.as_ref().unwrap();
        let mut tmp_analysis = analysis.clone();
        tmp_analysis.word_freq = tmp_analysis
            .word_freq
            .into_iter()
            .filter(|(_, string)| regex.is_match(string))
            .collect();
        filtered_word_count = tmp_analysis.word_freq.len();
        analysis_words_to_string(&tmp_analysis, top_words, bottom_words, emojis)
    };
    if analysis_string.is_empty() && hide_empty {
        return (buffer, 0, 0);
    }
    buffer.push_str(&format!("🔢 Word count: {}\n", analysis.word_count));
    buffer.push_str(&format!("🔢 Sentence count: {}\n", analysis.sent_count));
    buffer.push_str(&format!("🔢 Character count: {}\n", analysis.char_count));
    buffer.push_str(&format!("🔢 Paragraph count: {}\n", analysis.para_count));
    buffer.push_str(&format!("🔢 Unique words: {}\n", analysis.word_uniqs));
    buffer.push_str(&format!(
        "📊 Word frequency mean: {:.2}\n",
        analysis.word_dist_mean
    ));
    buffer.push_str(&format!(
        "📊 Word frequency standard deviation: {:.2}\n",
        analysis.word_dist_stddev
    ));
    buffer.push_str(&format!(
        "📊 Word frequency median: {:.1}\n",
        analysis.word_dist_median
    ));
    buffer.push_str(&format!(
        "📊 Word frequency mode: {:.1}\n",
        analysis.word_dist_mode
    ));
    if search_regex.is_some() {
        buffer.push_str(&format!(
            "🔎 Words matching filter: {}\n",
            filtered_word_count
        ));
    }
    if analysis_string.is_empty() {
        buffer.push_str(if search_regex.is_none() {
            "⚠️ No words in file\n"
        } else {
            "⚠️ No results in file\n"
        })
    } else {
        buffer.push_str("📈 Top words");
        if !search_regex.is_none() {
            buffer.push_str(" (filtered)")
        }
        buffer.push_str(":\n");
        buffer.push_str(&analysis_string);
        if !analysis_string_bottom.is_empty() {
            buffer.push_str("📉 Bottom words");
            if !search_regex.is_none() {
                buffer.push_str(" (filtered)")
            }
            buffer.push_str(":\n");
            buffer.push_str(&analysis_string_bottom);
        }
    };
    (
        buffer,
        analysis_string.lines().count() + analysis_string_bottom.lines().count(),
        filtered_word_count,
    )
}

pub fn get_result_text(
    analyses: &(Vec<Analysis>, Option<Analysis>),
    args: &RefCell<Args>,
    pwd: &RefCell<PathBuf>,
    search_text: &str,
) -> Result<String> {
    let (analyses, total) = (&analyses.0, &analyses.1);
    let mut buffer = String::new();
    let args = args.borrow().clone();
    let pwd = pwd.borrow().clone();
    let analyses_count = analyses.len();

    let search_is_regex = search_text.len() >= 3
        && search_text.starts_with('/')
        && (search_text.ends_with('/') || search_text.ends_with("/i"));
    let is_insensitive = search_text.ends_with("/i");
    let regex = if search_text.is_empty() {
        None
    } else if search_is_regex {
        Some(
            RegexBuilder::new(
                &search_text[1..search_text.len() - if is_insensitive { 2 } else { 1 }],
            )
            .case_insensitive(search_text.ends_with("/i"))
            .multi_line(false)
            .build()?,
        )
    } else {
        Some(
            RegexBuilder::new(&regex::escape(search_text))
                .case_insensitive(true)
                .multi_line(false)
                .build()?,
        )
    };
    let mut results_texts = analyses
        .into_par_iter()
        .filter_map(|analysis| {
            let (analysis_string, _, _) = analysis_to_string(
                analysis,
                args.top_words,
                args.bottom_words,
                args.hide_empty,
                &regex,
                args.emojis,
            );
            if !analysis_string.is_empty() {
                Some((
                    analysis.file.as_ref(),
                    format!(
                        "📁 File: {}\n",
                        analysis
                            .file
                            .as_ref()
                            .map(|file| diff_paths(file, &pwd)
                                .unwrap_or_else(|| file.clone())
                                .display()
                                .to_string())
                            .unwrap_or_else(|| "<none>".to_string())
                    ) + &analysis_string
                        + "\n",
                ))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    results_texts.sort_by_key(|(file, _)| *file);
    let results_count = results_texts.len();
    for (_, texts) in results_texts.into_iter() {
        buffer.push_str(&texts);
    }

    if let Some(ref analysis) = total {
        let (mut printed_total, mut filtered_word_count) = (0, 0);
        if results_count > 1 {
            let (analysis_string, _printed_total, _filtered_word_count) = analysis_to_string(
                analysis,
                args.top_words,
                args.bottom_words,
                args.hide_empty,
                &regex,
                args.emojis,
            );
            printed_total = _printed_total;
            filtered_word_count = _filtered_word_count;
            if !analysis_string.is_empty() {
                buffer.push_str(&format!("📢 Summary of {} files\n", analyses_count));
                buffer.push_str(&analysis_string);
                buffer.push('\n');
            }
            buffer.push('\n');
        }

        if args.show_all_words && (printed_total < filtered_word_count || results_count == 1) {
            let (analysis_string, _, _) =
                analysis_to_string(analysis, 0, 0, args.hide_empty, &regex, args.emojis);
            if !analysis_string.is_empty() {
                buffer.push_str(&format!(
                    "📢 Summary of {} files (all words)\n",
                    analyses_count
                ));
                buffer.push_str(&analysis_string);
            }
        }
    }

    if buffer.is_empty() {
        buffer.push_str(if search_text.is_empty() {
            "⚠️ No words in files\n"
        } else {
            "⚠️ No results in files\n"
        })
    }

    Ok(buffer.trim().replace("\r\n", "\n").replace('\n', "\r\n") + "\n")
}
