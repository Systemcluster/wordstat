use std::{cell::RefCell, path::PathBuf};

use pathdiff::diff_paths;
use rayon::iter::{IntoParallelIterator, ParallelIterator};

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
            if let Some(e) = emojis::lookup(&string.to_lowercase()) {
                buffer.push_str(&format!(" {}", e));
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
            if let Some(e) = emojis::lookup(&string.to_lowercase()) {
                buffer.push_str(&format!(" {}", e));
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
    search_text: &str,
    emojis: bool,
) -> String {
    let mut buffer = String::new();
    let (analysis_string, analysis_string_bottom) = if search_text.is_empty() {
        analysis_words_to_string(analysis, top_words, bottom_words, emojis)
    } else {
        let mut tmp_analysis = analysis.clone();
        tmp_analysis.word_freq = tmp_analysis
            .word_freq
            .into_iter()
            .filter(|(_, string)| string.to_lowercase().contains(&search_text.to_lowercase()))
            .collect();
        analysis_words_to_string(&tmp_analysis, top_words, bottom_words, emojis)
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
    if analysis_string.is_empty() {
        buffer.push_str(if search_text.is_empty() {
            "⚠️ No words in file"
        } else {
            "⚠️ No results in file"
        })
    } else {
        buffer.push_str("📈 Top words");
        if !search_text.is_empty() {
            buffer.push_str(" (filtered)")
        }
        buffer.push_str(":\n");
        buffer.push_str(&analysis_string);
        if !analysis_string_bottom.is_empty() {
            buffer.push_str("📉 Bottom words");
            if !search_text.is_empty() {
                buffer.push_str(" (filtered)")
            }
            buffer.push_str(":\n");
            buffer.push_str(&analysis_string_bottom);
        }
    };
    buffer
}

pub fn get_result_text(
    analyses: &(Vec<Analysis>, Option<Analysis>),
    args: &RefCell<Args>,
    pwd: &RefCell<PathBuf>,
    search_text: &str,
) -> String {
    let (analyses, total) = (&analyses.0, &analyses.1);
    let mut buffer = String::new();
    let args = args.borrow().clone();
    let pwd = pwd.borrow().clone();
    let analyses_count = analyses.len();

    let mut results_texts = analyses
        .into_par_iter()
        .filter_map(|analysis| {
            let analysis_string = analysis_to_string(
                analysis,
                args.top_words,
                args.bottom_words,
                args.hide_empty,
                search_text,
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
        if results_count > 1 {
            let analysis_string = analysis_to_string(
                analysis,
                args.top_words,
                args.bottom_words,
                args.hide_empty,
                search_text,
                args.emojis,
            );
            if !analysis_string.is_empty() {
                buffer.push_str(&format!("📢 Summary of {} files\n", analyses_count));
                buffer.push_str(&analysis_string);
                buffer.push('\n');
            }
            buffer.push('\n');
        }

        let analysis_string =
            analysis_to_string(analysis, 0, 0, args.hide_empty, search_text, args.emojis);
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
            "⚠️ No words in files\n"
        } else {
            "⚠️ No results in files\n"
        })
    }

    buffer.replace("\r\n", "\n").replace('\n', "\r\n")
}
