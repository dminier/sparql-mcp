//! Benchmark of the write path and of ingestion jobs, on a real RocksDB store.
//!
//! It exists to confirm (or lower) the provisional limits of
//! `docs/designs/sparql-mcp-batch-ingestion.md` and to check the promise of
//! `docs/designs/sparql-mcp-write-path-hardening.md` that a ground write does
//! not get slower as the store grows. Always run it in **release**: debug
//! builds are 10-50x slower and prove nothing.
//!
//! ```text
//! cargo run --release -p sparql-mcp --example ingest_bench
//! cargo run --release -p sparql-mcp --example ingest_bench -- --sizes 100000,250000,500000 --markdown out.md
//! ```
//!
//! Memory is the process's working set, sampled every 10 ms; "peak" is the
//! highest sample during the step minus the sample taken just before it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use sparql_mcp::application::event_sink::NullEventSink;
use sparql_mcp::application::ingest::FileRequest;
use sparql_mcp::application::ingest::{
    BatchRequest, BeginRequest, CommitRequest, IngestService, Mode,
};
use sparql_mcp::application::{build_stores, WriteGate};
use sparql_mcp::config::IngestConfig;
use sparql_mcp::domain::{InputFormat, SparqlStore, StoredQuad};
use sparql_mcp::infrastructure::OxigraphAdapter;

const ACTIVE: &str = "urn:project:bench";

#[derive(Parser)]
struct Args {
    /// Triples in each `ingest_batch`.
    #[arg(long, default_value_t = 5_000)]
    batch_triples: usize,
    /// Batches sent in the latency step.
    #[arg(long, default_value_t = 40)]
    batches: usize,
    /// Staged job sizes (triples) whose commit and abort are measured.
    #[arg(long, value_delimiter = ',', default_values_t = [100_000usize, 150_000, 250_000, 500_000])]
    sizes: Vec<usize>,
    /// Target sizes (triples) for the replace measurements.
    #[arg(long, value_delimiter = ',', default_values_t = [100_000usize, 250_000, 500_000])]
    replace_targets: Vec<usize>,
    /// Size (triples) of the staged job that replaces each target.
    #[arg(long, default_value_t = 100_000)]
    replace_staged: usize,
    /// Store size for the "does a write slow down" step.
    #[arg(long, default_value_t = 1_000_000)]
    store_size: usize,
    /// Triples in the generated file of the `ingest_file` step (0 skips it).
    #[arg(long, default_value_t = 1_000_000)]
    file_triples: usize,
    /// `file_batch_triples` for the `ingest_file` step.
    #[arg(long, default_value_t = 5_000)]
    file_batch_triples: usize,
    /// Also write the results as a markdown table to this file.
    #[arg(long)]
    markdown: Option<PathBuf>,
}

/// Samples the process working set until dropped.
struct Peak {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicUsize>,
    base: usize,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn rss() -> usize {
    memory_stats::memory_stats().map_or(0, |m| m.physical_mem)
}

impl Peak {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let base = rss();
        let peak = Arc::new(AtomicUsize::new(base));
        let (s, p) = (stop.clone(), peak.clone());
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                p.fetch_max(rss(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            stop,
            peak,
            base,
            thread: Some(thread),
        }
    }

    /// Peak growth over the starting point, in MiB.
    fn finish(mut self) -> f64 {
        self.peak.fetch_max(rss(), Ordering::Relaxed);
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.peak.load(Ordering::Relaxed).saturating_sub(self.base) as f64 / (1024.0 * 1024.0)
    }
}

/// N-Triples with a realistic mix: typed nodes, labels, and links.
fn payload(batch: usize, triples: usize) -> String {
    let mut out = String::with_capacity(triples * 110);
    for i in 0..triples {
        let n = batch * triples + i;
        let s = format!("<https://bench.example/entity/{n}>");
        match i % 4 {
            0 => out.push_str(&format!("{s} <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://bench.example/Class{}> .\n", n % 50)),
            1 => out.push_str(&format!("{s} <http://www.w3.org/2000/01/rdf-schema#label> \"Entity number {n} of the benchmark\" .\n")),
            2 => out.push_str(&format!("{s} <https://bench.example/dependsOn> <https://bench.example/entity/{}> .\n", n / 2)),
            _ => out.push_str(&format!("{s} <https://bench.example/size> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n", n % 997)),
        }
    }
    out
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

struct Fixture {
    svc: IngestService,
    raw: Arc<dyn SparqlStore>,
    _dir: tempfile::TempDir,
}

fn fixture(cfg: IngestConfig) -> Result<Fixture> {
    let dir = tempfile::tempdir()?;
    let raw: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open(&dir.path().join("store"))?);
    let (_gated, privileged) = build_stores(raw.clone(), WriteGate::new(Duration::from_secs(600)));
    let svc = IngestService::new(
        privileged,
        Arc::new(RwLock::new(ACTIVE.to_string())),
        cfg,
        Arc::new(NullEventSink),
    );
    Ok(Fixture {
        svc,
        raw,
        _dir: dir,
    })
}

fn begin(svc: &IngestService, replace: bool) -> Result<(String, String)> {
    let b = svc.begin(&BeginRequest {
        target_graph: None,
        mode: Mode::Staged,
        replace,
        format: "ntriples".into(),
        expected_batches: None,
        label: None,
        blank_nodes: "skolemize".into(),
        file: None,
    })?;
    Ok((
        b["job_id"].as_str().unwrap().to_string(),
        b["staging_graph"].as_str().unwrap().to_string(),
    ))
}

/// Send `triples` triples to a job in batches; returns per-batch latencies (ms).
/// `salt` makes the data of one job different from another's, so jobs that
/// are meant to add to the store really do. Sequence numbers always start at 1.
fn send(
    svc: &IngestService,
    id: &str,
    triples: usize,
    per_batch: usize,
    salt: usize,
) -> Result<Vec<f64>> {
    let batches = triples.div_ceil(per_batch);
    let mut ms = Vec::with_capacity(batches);
    for b in 0..batches {
        let n = per_batch.min(triples - b * per_batch);
        let data = payload(salt * 1_000_000 + b, n);
        let t = Instant::now();
        svc.batch(&BatchRequest {
            job_id: id.to_string(),
            seq: 1 + b as u32,
            data,
            format: None,
        })?;
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(ms)
}

fn big_cfg() -> IngestConfig {
    IngestConfig {
        max_job_triples_staged: 5_000_000,
        max_replace_triples: 50_000_000,
        max_batch_triples: 20_000,
        max_job_batches: 100_000,
        ..IngestConfig::default()
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut report: Vec<String> = Vec::new();
    let mut say = |line: String| {
        println!("{line}");
        report.push(line);
    };
    say(format!(
        "# ingest_bench ({} build)\n",
        if cfg!(debug_assertions) {
            "DEBUG: numbers are meaningless"
        } else {
            "release"
        }
    ));

    // ── 1. Parsing memory per input byte ────────────────────────────────────
    {
        let f = fixture(IngestConfig::default())?;
        let data = payload(0, 10_000); // about 1 MiB
        let peak = Peak::start();
        let t = Instant::now();
        let quads: Vec<StoredQuad> = f
            .raw
            .parse_rdf_raw(data.as_bytes(), InputFormat::NTriples)?;
        let secs = t.elapsed().as_secs_f64();
        let mb = peak.finish();
        say("## Parsing (parse_rdf_raw, no write)\n".to_string());
        say(format!(
            "| input | triples | time | peak memory | memory per input byte |\n|---|---|---|---|---|\n| {:.2} MiB | {} | {:.0} ms | {:.1} MiB | {:.1}x |\n",
            data.len() as f64 / 1_048_576.0,
            quads.len(),
            secs * 1000.0,
            mb,
            mb * 1_048_576.0 / data.len() as f64
        ));
    }

    // ── 2. ingest_batch latency and throughput ──────────────────────────────
    {
        let f = fixture(big_cfg())?;
        let (id, _) = begin(&f.svc, false)?;
        let total = args.batch_triples * args.batches;
        let t = Instant::now();
        let mut ms = send(&f.svc, &id, total, args.batch_triples, 0)?;
        let secs = t.elapsed().as_secs_f64();
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        say(
            "## ingest_batch (staged, includes parse, skolemize, write and the batch record)\n"
                .to_string(),
        );
        say(format!(
            "| batches | triples/batch | p50 | p95 | max | throughput | design target |\n|---|---|---|---|---|---|---|\n| {} | {} | {:.0} ms | {:.0} ms | {:.0} ms | {:.0} triples/s | 5,000 in <= 300 ms |\n",
            args.batches, args.batch_triples,
            percentile(&ms, 0.5), percentile(&ms, 0.95), ms[ms.len() - 1],
            total as f64 / secs,
        ));
    }

    // ── 3. A staged commit: duration (the gate hold time) and memory ────────
    say(
        "## Staged commit and abort (the commit holds the write gate for its whole duration)\n"
            .to_string(),
    );
    say("| staged triples | commit time | commit peak memory | abort time | abort peak memory |\n|---|---|---|---|---|".to_string());
    for &size in &args.sizes {
        let f = fixture(big_cfg())?;
        let (id, _) = begin(&f.svc, false)?;
        send(&f.svc, &id, size, args.batch_triples, 0)?;
        let peak = Peak::start();
        let t = Instant::now();
        f.svc.commit(&CommitRequest {
            job_id: id,
            require_validation: false,
            allow_empty_replace: false,
        })?;
        let commit_s = t.elapsed().as_secs_f64();
        let commit_mb = peak.finish();

        let f = fixture(big_cfg())?;
        let (id, _) = begin(&f.svc, false)?;
        send(&f.svc, &id, size, args.batch_triples, 0)?;
        let peak = Peak::start();
        let t = Instant::now();
        f.svc.abort(&id)?;
        let abort_s = t.elapsed().as_secs_f64();
        let abort_mb = peak.finish();
        say(format!(
            "| {size} | {:.1} s | {commit_mb:.0} MiB | {:.1} s | {abort_mb:.0} MiB |",
            commit_s, abort_s
        ));
    }
    say(String::new());

    // ── 4. A replace commit: a bigger target replaced by a smaller job ──────
    say("## Replace commit (a target of N triples replaced by a staged job)\n".to_string());
    say(
        "| target triples | staged triples | commit time | peak memory |\n|---|---|---|---|"
            .to_string(),
    );
    for &target in &args.replace_targets {
        let f = fixture(big_cfg())?;
        // Fill the target through an ordinary job, then replace it.
        let (id, _) = begin(&f.svc, false)?;
        send(&f.svc, &id, target, args.batch_triples, 0)?;
        f.svc.commit(&CommitRequest {
            job_id: id,
            require_validation: false,
            allow_empty_replace: false,
        })?;
        let (id, _) = begin(&f.svc, true)?;
        send(&f.svc, &id, args.replace_staged, args.batch_triples, 1)?;
        let peak = Peak::start();
        let t = Instant::now();
        f.svc.commit(&CommitRequest {
            job_id: id,
            require_validation: false,
            allow_empty_replace: false,
        })?;
        let s = t.elapsed().as_secs_f64();
        say(format!(
            "| {target} | {} | {s:.1} s | {:.0} MiB |",
            args.replace_staged,
            peak.finish()
        ));
    }
    say(String::new());

    // ── 5. Does a ground write slow down as the store grows? ───────────────
    {
        let f = fixture(big_cfg())?;
        let time_writes = |f: &Fixture, label: &str| -> f64 {
            let t = Instant::now();
            let n = 200;
            for i in 0..n {
                f.raw
                    .update_with_outcome(&format!(
                        "INSERT DATA {{ GRAPH <{ACTIVE}> {{ <urn:probe:{label}:{i}> <urn:p> <urn:o> }} }}"
                    ))
                    .unwrap();
            }
            t.elapsed().as_secs_f64() * 1000.0 / n as f64
        };
        let empty_ms = time_writes(&f, "empty");
        // Grow the store through ordinary jobs.
        let mut have = 0usize;
        let mut seq_job = 0;
        while have < args.store_size {
            let step = (args.store_size - have).min(250_000);
            let (id, _) = begin(&f.svc, false)?;
            send(&f.svc, &id, step, args.batch_triples, 10 + seq_job)?;
            f.svc.commit(&CommitRequest {
                job_id: id,
                require_validation: false,
                allow_empty_replace: false,
            })?;
            have += step;
            seq_job += 1;
        }
        let big_ms = time_writes(&f, "big");
        say(
            "## Cost of one ground write (INSERT DATA of one triple) versus store size\n"
                .to_string(),
        );
        say(format!(
            "| store | per write |\n|---|---|\n| empty | {empty_ms:.2} ms |\n| {} triples | {big_ms:.2} ms |\n",
            args.store_size
        ));
    }

    // ── 6. ingest_file: throughput, memory, and what other users feel ───────
    if args.file_triples > 0 {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("bench.nt");
        {
            use std::io::Write;
            let mut out = std::io::BufWriter::new(std::fs::File::create(&file)?);
            let per = 10_000;
            for b in 0..args.file_triples.div_ceil(per) {
                let n = per.min(args.file_triples - b * per);
                out.write_all(payload(b, n.max(1)).as_bytes())?;
                let _ = n;
            }
        }
        let size = std::fs::metadata(&file)?.len();
        let store_dir = tempfile::tempdir()?;
        let raw: Arc<dyn SparqlStore> =
            Arc::new(OxigraphAdapter::open(&store_dir.path().join("store"))?);
        let (gated, privileged) =
            build_stores(raw.clone(), WriteGate::new(Duration::from_secs(600)));
        let svc = Arc::new(IngestService::new(
            privileged,
            Arc::new(RwLock::new(ACTIVE.to_string())),
            IngestConfig {
                file_roots: Some(vec![dir.path().to_path_buf()]),
                max_job_triples_direct: 100_000_000,
                max_job_batches: 100_000,
                file_batch_triples: args.file_batch_triples,
                max_batch_triples: args.file_batch_triples.max(10_000),
                ..IngestConfig::default()
            },
            Arc::new(NullEventSink),
        ));

        let base_rss = rss();
        let peak = Peak::start();
        let started = Instant::now();
        let b = svc.start_file(&FileRequest {
            path: file.display().to_string(),
            target_graph: None,
            mode: Mode::Direct,
            format: None,
            resume_job: None,
            force: false,
        })?;
        let hash_secs = started.elapsed().as_secs_f64();
        let id = b["job_id"].as_str().unwrap().to_string();

        // While the job runs: a reader and a writer, as other users would be.
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let (raw, stop) = (raw.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut ms = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    let t = Instant::now();
                    let _ = raw.query(&format!(
                        "ASK {{ GRAPH <{ACTIVE}> {{ <https://bench.example/entity/1> ?p ?o }} }}"
                    ));
                    ms.push(t.elapsed().as_secs_f64() * 1000.0);
                    std::thread::sleep(Duration::from_millis(20));
                }
                ms
            })
        };
        let writer = {
            let (gated, stop) = (gated.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut ms = Vec::new();
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let t = Instant::now();
                    let _ = gated.update(&format!(
                        "INSERT DATA {{ GRAPH <{ACTIVE}> {{ <urn:probe:{n}> <urn:p> <urn:o> }} }}"
                    ));
                    ms.push(t.elapsed().as_secs_f64() * 1000.0);
                    n += 1;
                    std::thread::sleep(Duration::from_millis(100));
                }
                ms
            })
        };
        let run_started = Instant::now();
        let status = loop {
            let s = svc.status(Some(&id))?;
            if s["state"] != "running" {
                break s;
            }
            std::thread::sleep(Duration::from_millis(250));
        };
        let run_secs = run_started.elapsed().as_secs_f64();
        stop.store(true, Ordering::Relaxed);
        let mut reads = reader.join().unwrap();
        let mut writes = writer.join().unwrap();
        reads.sort_by(|a, b| a.partial_cmp(b).unwrap());
        writes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mb = peak.finish();
        // Is the memory given back? Growth over the start, right after the job,
        // after the store has been idle, and after it is closed.
        let mib = |bytes: usize| bytes.saturating_sub(base_rss) as f64 / 1_048_576.0;
        let just_after = mib(rss());
        std::thread::sleep(Duration::from_secs(5));
        let after_5s = mib(rss());
        std::thread::sleep(Duration::from_secs(10));
        let after_15s = mib(rss());
        drop(svc);
        drop(gated);
        drop(raw);
        std::thread::sleep(Duration::from_secs(3));
        let after_close = mib(rss());
        say("## Memory after an ingest_file job (growth over the start, MiB)\n".to_string());
        say(format!(
            "| triples | peak | right after | after 5 s idle | after 15 s idle | after the store is closed |\n|---|---|---|---|---|---|\n| {} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} |\n",
            status["triples"].as_u64().unwrap_or(0), mb, just_after, after_5s, after_15s, after_close
        ));
        let triples = status["triples"].as_u64().unwrap_or(0);
        say(
            "## ingest_file (direct, RocksDB), with a reader and a writer running meanwhile\n"
                .to_string(),
        );
        say(format!(
            "| file | state | triples | hash + start | run | throughput | peak memory | read p95 / max | write p95 / max | design target |\n|---|---|---|---|---|---|---|---|---|---|\n| {:.0} MiB | {} | {} | {:.1} s | {:.1} s | {:.0} triples/s | {:.0} MiB | {:.0} / {:.0} ms | {:.0} / {:.0} ms | >= 50,000 triples/s, < 512 MiB, read p95 < 200 ms |\n",
            size as f64 / 1_048_576.0,
            status["state"].as_str().unwrap_or("?"),
            triples,
            hash_secs,
            run_secs,
            triples as f64 / run_secs.max(0.001),
            mb,
            percentile(&reads, 0.95), reads[reads.len() - 1],
            percentile(&writes, 0.95), writes[writes.len() - 1],
        ));
    }

    if let Some(path) = &args.markdown {
        std::fs::write(path, report.join("\n"))?;
        println!("written to {}", path.display());
    }
    Ok(())
}
