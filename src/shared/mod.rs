mod uhash;
mod ustring;

use std::{
    collections::HashMap,
    hash::BuildHasherDefault,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use dashmap::DashMap;
use entangled::{ThreadPool, ThreadPoolDescriptor};
use futures::{executor::block_on, future::join_all};
use pathdiff::diff_paths;
use rayon::iter::{
    IndexedParallelIterator, IntoParallelRefIterator, ParallelBridge, ParallelIterator,
};
use unicode_segmentation::UnicodeSegmentation;
use walkdir::WalkDir;

use uhash::IdentityHasher;
use ustring::UniqueString;

#[derive(Default, Debug, Clone)]
pub struct Analysis {
    pub file:             Option<PathBuf>,
    pub word_count:       usize,
    pub char_count:       usize,
    pub sent_count:       usize,
    pub para_count:       usize,
    pub word_uniqs:       usize,
    pub word_freq:        Vec<(usize, UniqueString)>,
    pub word_freq_map:    DashMap<UniqueString, usize, BuildHasherDefault<IdentityHasher>>,
    pub word_dist_mean:   f64,
    pub word_dist_stddev: f64,
    pub word_dist_median: f64,
    pub word_dist_mode:   f64,
}

#[derive(Default, Debug, Clone)]
pub struct Args {
    pub lowercase:       bool,
    pub top_words:       usize,
    pub bottom_words:    usize,
    pub recursive:       bool,
    pub follow_symlinks: bool,
    pub outfile:         Option<String>,
    pub hide_empty:      bool,
    pub emojis:          bool,
    pub show_all_words:  bool,
}

fn update_dists(analysis: &mut Analysis) {
    if analysis.word_freq.is_empty() {
        return;
    }
    analysis.word_dist_mean =
        analysis.word_freq.iter().map(|a| a.0).reduce(|a, b| a + b).unwrap_or_default() as f64
            / analysis.word_uniqs as f64;
    analysis.word_dist_stddev = (analysis
        .word_freq
        .iter()
        .map(|a| (a.0 as f64 - analysis.word_dist_mean).powi(2))
        .reduce(|a, b| a + b)
        .unwrap_or_default()
        / analysis.word_uniqs as f64)
        .sqrt();
    analysis.word_dist_median = match analysis.word_freq.len() % 2 == 0 {
        true => {
            (analysis.word_freq[(analysis.word_freq.len()) / 2 - 1].0
                + analysis.word_freq[(analysis.word_freq.len()) / 2].0) as f64
                / 2.0
        }
        false => analysis.word_freq[analysis.word_freq.len() / 2].0 as f64,
    };

    let mut mode_counts = HashMap::new();
    analysis
        .word_freq
        .iter()
        .for_each(|(freq, _)| *mode_counts.entry(*freq).or_insert(0) += 1);
    let max_count = mode_counts.iter().max_by_key(|(_, &v)| v).map(|(_, &v)| v).unwrap_or(0);
    if max_count > 0 {
        let mut counts = 0;
        analysis.word_dist_mode =
            mode_counts.iter().filter(|(_, &v)| v == max_count).fold(0, |acc, (&k, _)| {
                counts += 1;
                acc + k
            }) as f64
                / counts as f64;
    }
}

async fn process(
    source: AnalyzeSource, lowercase: bool,
) -> Result<Analysis, (PathBuf, std::io::Error)> {
    let (content, file) = match source {
        AnalyzeSource::Content(content) => (content, None),
        AnalyzeSource::Path(path) => (
            std::fs::read_to_string(&path).map_err(|error| (path.clone(), error))?,
            Some(path),
        ),
    };

    let mut analysis = Analysis {
        file,
        sent_count: content.unicode_sentences().count(),
        para_count: content.replace("\r\n", "\n").split("\n\n").count(),
        char_count: content.graphemes(true).count(),
        ..Default::default()
    };

    let map = DashMap::default();
    let words = content.unicode_words().collect::<Vec<_>>();
    analysis.word_count = words
        .par_iter()
        .chunks(12500)
        .map(|words| {
            let len = words.len();
            for &word in words {
                map.entry(if lowercase {
                    UniqueString::from(word.to_lowercase())
                } else {
                    UniqueString::from(word)
                })
                .and_modify(|num| *num += 1)
                .or_insert(1);
            }
            len
        })
        .sum();
    map.iter().for_each(|item| {
        let (word, count) = (item.key(), item.value());
        analysis.word_freq.push((*count, *word));
    });
    analysis.word_freq.sort_by(|(a, _), (b, _)| b.cmp(a));
    analysis.word_freq_map = map;
    analysis.word_uniqs = analysis.word_freq.len();
    update_dists(&mut analysis);

    Ok(analysis)
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum AnalyzeSource {
    Content(String),
    Path(PathBuf),
}

pub fn analyze<
    E: Fn(String) + Sync + Send,
    P: Fn(String) + Sync + Send,
    M: Fn(String) + Sync + Send,
    I: Fn(u64) + Sync + Send,
>(
    sources: &Vec<AnalyzeSource>, args: &Args, pwd: &Path, on_error: E, on_message: M,
    on_progress: P, on_increment: I,
) -> (Vec<Analysis>, Option<Analysis>) {
    let pool = ThreadPool::new(ThreadPoolDescriptor {
        num_threads: num_cpus::get(),
        ..Default::default()
    })
    .unwrap_or_else(|error| {
        on_error(format!("{}", error));
        std::process::exit(1)
    });
    let tasks = Arc::new(Mutex::new(Vec::new()));

    sources.par_iter().for_each(|source| {
        match source {
            AnalyzeSource::Content(content) => {
                on_progress("Analyzing...".to_string());
                tasks.lock().unwrap().push(
                    pool.spawn(process(AnalyzeSource::Content(content.to_owned()), args.lowercase)),
                );
            }
            AnalyzeSource::Path(path) => {
                let walk = WalkDir::new(path)
                    .follow_links(args.follow_symlinks)
                    .max_depth(if args.recursive { std::usize::MAX } else { 1 })
                    .sort_by_file_name();
                walk.into_iter()
                    .filter_map(|path| {
                        path.map_err(|error| {
                            on_message(format!("{}", error));
                            error
                        })
                        .map_or(None, |path| {
                            if path.file_type().is_file() {
                                Some(path)
                            } else {
                                None
                            }
                        })
                    })
                    .par_bridge()
                    .for_each(|file| {
                        on_progress(format!(
                            "Analyzing {}",
                            diff_paths(file.path(), pwd)
                                .unwrap_or_else(|| file.path().to_owned())
                                .display()
                        ));
                        tasks.lock().unwrap().push(pool.spawn(process(
                            AnalyzeSource::Path(file.path().to_owned()),
                            args.lowercase,
                        )));
                    });
            }
        }
        on_increment(1);
    });

    let tasks = Arc::try_unwrap(tasks).unwrap().into_inner().unwrap();
    let analyses = block_on(join_all(tasks));

    let mut total: Option<Analysis> = None;
    for analysis in analyses.iter() {
        if let Err((path, error)) = analysis {
            on_error(format!(
                "Failed to analyze {}: {}",
                diff_paths(path, pwd).unwrap_or_else(|| path.to_owned()).display(),
                error
            ));
            continue;
        }
        let analysis = analysis.as_ref().unwrap();

        if let Some(total) = &mut total {
            total.word_count += analysis.word_count;
            total.sent_count += analysis.sent_count;
            total.char_count += analysis.char_count;
            total.para_count += analysis.para_count;
            for item in analysis.word_freq_map.iter() {
                let (word, count) = (item.key(), item.value());
                total
                    .word_freq_map
                    .entry(*word)
                    .and_modify(|num| *num += *count)
                    .or_insert(*count);
            }
        } else {
            let mut analysis = analysis.clone();
            analysis.file = None;
            analysis.word_freq.clear();
            total = Some(analysis);
        }
    }
    let analyses = analyses.into_iter().filter_map(|analysis| analysis.ok()).collect();
    if let Some(analysis) = &mut total {
        analysis.word_freq_map.iter().for_each(|item| {
            let (word, count) = (item.key(), item.value());
            analysis.word_freq.push((*count, *word))
        });
        analysis.word_freq.sort_by(|(a, _), (b, _)| b.cmp(a));
        update_dists(analysis);
    }

    (analyses, total)
}
