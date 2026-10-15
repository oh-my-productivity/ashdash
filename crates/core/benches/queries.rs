//! Criterion benches: `cargo bench -p ashdash-core --bench queries`.
//!
//! Query latency over a synthetic 5,000-app index, cold-start index load and fuzzy scoring.
//! (`--bench report` prints the human-readable before/after numbers instead.)

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use ashdash_core::fuzzy::{self, Candidate, Query};
use ashdash_core::index::{AppIndex, IndexOptions, synth};
use ashdash_core::{Config, Engine, Paths};
use criterion::{Criterion, criterion_group, criterion_main};

const N_APPS: usize = 5_000;
const QUERIES: [&str; 4] = ["saf", "vsc", "visual studio", "x"];

struct Fixture {
    root: PathBuf,
    apps_dir: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let root = std::env::temp_dir().join(format!("oa-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let apps_dir = root.join("apps");
        std::fs::create_dir_all(&apps_dir).unwrap();
        synth::make_fake_apps(&apps_dir, N_APPS, true).unwrap();
        Fixture { root, apps_dir }
    }

    fn config(&self) -> Config {
        Config {
            app_dirs: vec![self.apps_dir.clone()],
            ..Config::default()
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn engine_queries(c: &mut Criterion) {
    let fx = Fixture::new();
    let e = Engine::new(Paths::under(&fx.root), fx.config());
    assert!(e.app_count() >= N_APPS);
    let mut g = c.benchmark_group("engine.query/5000-apps");
    for q in QUERIES {
        g.bench_function(q, |b| b.iter(|| std::hint::black_box(e.query(q))));
    }
    g.finish();
}

fn cold_start(c: &mut Criterion) {
    let fx = Fixture::new();
    let paths = Paths::under(&fx.root);
    drop(Engine::new(paths.clone(), fx.config())); // writes the cache
    let mut g = c.benchmark_group("cold-start/5000-apps");
    g.sample_size(20);
    g.bench_function("index load from warm cache", |b| {
        b.iter(|| {
            let live = Arc::new(RwLock::new(Vec::new()));
            let opts = IndexOptions {
                watch: false,
                refresh_on_start: false,
            };
            let idx = AppIndex::open_with(&paths, std::slice::from_ref(&fx.apps_dir), live.clone(), opts);
            assert!(idx.loaded_from_cache());
            std::hint::black_box(live.read().unwrap().len())
        })
    });
    g.bench_function("Engine::new + first query (warm cache)", |b| {
        b.iter(|| {
            let e = Engine::new(paths.clone(), fx.config());
            std::hint::black_box(e.query("saf"))
        })
    });
    g.bench_function("full scan (no cache)", |b| {
        b.iter(|| std::hint::black_box(ashdash_core::apps::scan(std::slice::from_ref(&fx.apps_dir))))
    });
    g.finish();
}

fn fuzzy_scoring(c: &mut Criterion) {
    let names = synth::synthetic_names(N_APPS);
    let cands: Vec<Candidate> = names.iter().map(|n| Candidate::new(n)).collect();
    let mut g = c.benchmark_group("fuzzy/5000-candidates");
    for q in QUERIES {
        let query = Query::new(q);
        g.bench_function(format!("precomputed {q}"), |b| {
            b.iter(|| cands.iter().filter(|c| query.score(c).is_some()).count())
        });
        g.bench_function(format!("string wrapper {q}"), |b| {
            b.iter(|| names.iter().filter(|n| fuzzy::score(q, n).is_some()).count())
        });
    }
    g.finish();
}

criterion_group!(benches, engine_queries, cold_start, fuzzy_scoring);
criterion_main!(benches);
