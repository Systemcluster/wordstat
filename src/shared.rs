use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use console::{style, Emoji};
use entangled::{ThreadPool, ThreadPoolDescriptor};
use futures::executor::block_on;
use futures::future::join_all;
use pathdiff::diff_paths;
use rayon::iter::{IntoParallelRefIterator, ParallelBridge, ParallelIterator};
use unicode_segmentation::UnicodeSegmentation;
use walkdir::WalkDir;

use crate::uhash::IdentityHashMap;
use crate::ustring::UniqueString;

#[derive(Default, Debug, Clone)]
pub struct Analysis {
    pub file: Option<PathBuf>,
    pub word_count: usize,
    pub word_freq: Vec<(usize, UniqueString)>,
    pub word_freq_map: IdentityHashMap<UniqueString, usize>,
}

#[derive(Default, Debug, Clone)]
pub struct Args {
    pub lowercase: bool,
    pub top_words: usize,
    pub bottom_words: usize,
    pub recursive: bool,
    pub follow_symlinks: bool,
    pub outfile: Option<String>,
}

async fn process(path: PathBuf, lowercase: bool) -> Result<Analysis, (PathBuf, std::io::Error)> {
    let content = std::fs::read_to_string(&path).map_err(|error| (path.clone(), error))?;
    let mut analysis = Analysis {
        file: Some(path),
        ..Default::default()
    };

    let mut map = IdentityHashMap::<UniqueString, usize>::default();
    for word in content.unicode_words().map(|word| {
        if lowercase {
            UniqueString::from(word.to_lowercase())
        } else {
            UniqueString::from(word)
        }
    }) {
        analysis.word_count += 1;
        map.entry(word).and_modify(|num| *num += 1).or_insert(1);
    }
    map.iter()
        .for_each(|(path, count)| analysis.word_freq.push((*count, *path)));
    analysis.word_freq.sort_by(|(a, _), (b, _)| b.cmp(a));
    analysis.word_freq_map = map;

    Ok(analysis)
}

pub fn analyze<
    E: Fn(String) + Sync + Send,
    P: Fn(String) + Sync + Send,
    M: Fn(String) + Sync + Send,
    I: Fn(u64) + Sync + Send,
>(
    paths: &Vec<PathBuf>,
    args: &Args,
    pwd: &Path,
    on_error: E,
    on_message: M,
    on_progress: P,
    on_increment: I,
) -> (Vec<Analysis>, Analysis) {
    let pool = ThreadPool::new(ThreadPoolDescriptor {
        num_threads: num_cpus::get(),
        ..Default::default()
    })
    .unwrap_or_else(|error| {
        on_error(format!("{}", error));
        std::process::exit(1)
    });
    let tasks = Arc::new(Mutex::new(Vec::new()));

    paths.par_iter().for_each(|path| {
        let walk = WalkDir::new(path)
            .follow_links(args.follow_symlinks)
            .max_depth(if args.recursive { std::usize::MAX } else { 1 })
            .sort_by_file_name();
        walk.into_iter()
            .filter_map(|path| {
                path.map_err(|error| {
                    on_message(format!("{}{}", Emoji("⚠️ ", ""), style(&error).red()));
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
                    diff_paths(file.path(), &pwd)
                        .unwrap_or_else(|| file.path().to_owned())
                        .display()
                ));
                tasks
                    .lock()
                    .unwrap()
                    .push(pool.spawn(process(file.path().to_owned(), args.lowercase)));
            });
        on_increment(1);
    });

    let tasks = Arc::try_unwrap(tasks).unwrap().into_inner().unwrap();
    let analyses = block_on(join_all(tasks));

    let mut total: Option<Analysis> = None;
    for analysis in analyses.iter() {
        if let Err((path, error)) = analysis {
            on_error(format!(
                "{}{}: {}",
                Emoji("⚠️ ", ""),
                style(&format!(
                    "Failed to analyze {}:",
                    diff_paths(&path, &pwd)
                        .unwrap_or_else(|| path.to_owned())
                        .display()
                ))
                .red(),
                style(&error).red()
            ));
            continue;
        }
        let analysis = analysis.as_ref().unwrap();

        if let Some(total) = &mut total {
            total.word_count += analysis.word_count;
            for (word, count) in &analysis.word_freq_map {
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
    let analyses = analyses
        .into_iter()
        .filter_map(|analysis| analysis.ok())
        .collect();

    (analyses, total.unwrap())
}
