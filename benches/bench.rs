#![feature(once_cell)]

use glassbench::*;
use std::path::PathBuf;

#[path = "../src/shared/mod.rs"]
mod wordstat;
use wordstat::*;

fn bench_examples(bencher: &mut Bench) {
    let paths: Vec<PathBuf> = Vec::from([
        "examples/Adventures in Wonderland.txt".into(),
        "examples/Pride and Prejudice.txt".into(),
    ]);
    let args = Args {
        lowercase: false,
        top_words: 10,
        bottom_words: 3,
        recursive: true,
        follow_symlinks: false,
        hide_empty: false,
        show_all_words: false,
        outfile: None,
        emojis: false,
    };
    let pwd = std::fs::canonicalize(std::env::current_dir().unwrap_or_else(|_| PathBuf::new()))
        .unwrap_or_else(|_| PathBuf::new());
    bencher.task("Examples", move |task| {
        task.iter(|| {
            let (_, _) = analyze(
                &paths
                    .iter()
                    .map(|path| AnalyzeSource::Path(path.to_owned()))
                    .collect(),
                &args,
                &pwd,
                |error| eprintln!("{}", error),
                |message| println!("{}", message),
                |message| println!("{}", message),
                |_| (),
            );
        });
    });
}

glassbench!("Analyze", bench_examples,);
