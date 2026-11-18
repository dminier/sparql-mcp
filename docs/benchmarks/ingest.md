# Benchmark: the write path and ingestion jobs

`crates/sparql-mcp-core/examples/ingest_bench.rs` measures what the write
path costs on a real RocksDB store. It exists for three reasons:

*   to **confirm or lower the provisional limits** of
    [`sparql-mcp-batch-ingestion.md`](../designs/sparql-mcp-batch-ingestion.md)
    (`max_job_triples_staged`, `max_replace_triples`): a staged commit is one
    in-memory transaction, so these limits are bounded by memory and by how
    long the write gate is held;
*   to check the promise of
    [`sparql-mcp-write-path-hardening.md`](../designs/sparql-mcp-write-path-hardening.md)
    that a ground write does not get slower as the store grows;
*   to give a baseline, so a later change that makes ingestion slower or
    hungrier is noticed.

## Run it

Always in **release**. A debug build is 10 to 50 times slower, and the
benchmark prints a warning in its header when it is one.

```text
cargo run --release -p sparql-mcp --example ingest_bench
cargo run --release -p sparql-mcp --example ingest_bench -- --sizes 100000,250000,500000 --markdown out.md
```

The first release build compiles RocksDB and takes several minutes. Use a
separate target directory (`CARGO_TARGET_DIR=target/bench`) so it does not
block your normal builds. A full run takes a few minutes and writes a few
hundred MiB under the system temporary directory (a throwaway store per step).
Do not run other heavy work at the same time: it changes the timings.

| Flag | Default | Meaning |
|---|---|---|
| `--batch-triples` | 5,000 | Triples in each `ingest_batch` |
| `--batches` | 40 | Batches in the latency step |
| `--sizes` | 100000,250000,500000 | Staged job sizes whose commit and abort are measured |
| `--replace-sizes` | 100000,250000,500000 | Target size = staged size for the replace step |
| `--store-size` | 1,000,000 | Store size for the "does a write slow down" step |
| `--markdown <file>` | none | Also write the tables to a file |

## What it measures

The benchmark calls the ingestion service directly, so MCP and JSON overhead
are **not** included: the numbers are the cost of the store and of the
service, which is what the limits depend on. Test data is N-Triples of about
100 bytes per triple, mixing typed nodes, labels, links and integers.

Memory is the process working set, sampled every 10 ms. "Peak memory" is the
highest sample during the step minus the sample taken just before it.

1.  **Parsing.** One 1 MiB batch through `parse_rdf_raw`, with no write: time,
    peak memory, and memory per input byte. The design assumes about 10x.
2.  **`ingest_batch`.** `--batches` batches of `--batch-triples` triples into a
    staged job: median, 95th percentile and maximum latency, and throughput.
    Each call includes the parse, the blank-node mapping, the checks, the
    transaction with the batch record and the job counters.
3.  **Staged commit and abort.** For each size, a job of that many triples is
    committed (and, in a fresh store, aborted). **Commit time is how long the
    write gate is held**: every other writer waits, and gets `write_busy` after
    `write_wait_timeout_secs`. Peak memory is the cost of the in-memory
    transaction.
4.  **Replace commit.** A target of N triples replaced by a staged job of N
    triples. A replace drops the target, copies the staged triples and drops
    the staging graph in one transaction, so its cost grows with both.
5.  **Ground write cost versus store size.** The time of a one-triple
    `INSERT DATA`, in an empty store and in a store of `--store-size` triples.
    The write path no longer scans the store on a ground write, so the two
    should be close.

## How the numbers set the limits

A limit is lowered when a step breaks one of these rules:

| Rule | Why |
|---|---|
| A staged commit takes at most **10 s** | The write gate is held for the whole commit. 10 s is a third of the default `write_wait_timeout_secs` (30), so a concurrent writer is not refused with `write_busy` |
| A staged commit's peak memory growth is at most **1 GiB** | The core is spawned per agent session, next to the agent and the viewer |
| A replace commit follows the same two rules | Its transaction is larger than an append's |
| `ingest_batch` of 5,000 triples takes at most **300 ms** (95th percentile) | The target in the design; an agent sends many batches |
| A ground write in the large store costs at most **2x** the empty store | The promise of the hardening design |

`max_job_triples_staged` is the largest tested size that passes the first two
rules. `max_replace_triples` is the largest target size plus staged size that
passes the replace rule. Round down to a round figure, and keep a margin: the
benchmark machine is not the user's machine.

## Results

Results depend on the machine. Record the commit, the machine and the date
with them, and rerun after a change to the write path, to the store port, or
to the Oxigraph or RocksDB version.

### Machine and build

| | |
|---|---|
| Date | 2026-11-18 |
| Code | commit `cd71fb7`, plus the benchmark itself |
| Build | release (`rustc 1.100.0`), Windows 11, native |
| CPU | Intel Core i7-14700K (20 cores, 28 threads) |
| Memory | 63.8 GiB |
| Disk | Samsung 990 PRO 2 TB (NVMe) |
| Store | RocksDB, a fresh directory per step |

The run was done twice (a first run, then one after fixing the benchmark's
last step); the two agree to within a few percent on every figure they share,
and the tables below are from the second.

### Parsing (parse_rdf_raw, no write)

| input | triples | time | peak memory | memory per input byte |
|---|---|---|---|---|
| 1.11 MiB | 10000 | 12 ms | 6.4 MiB | 5.8x |

### ingest_batch (staged, includes parse, skolemize, write and the batch record)

| batches | triples/batch | p50 | p95 | max | throughput | design target |
|---|---|---|---|---|---|---|
| 40 | 5000 | 117 ms | 134 ms | 136 ms | 43866 triples/s | 5,000 in <= 300 ms |

### Staged commit and abort (the commit holds the write gate for its whole duration)

| staged triples | commit time | commit peak memory | abort time | abort peak memory |
|---|---|---|---|---|
| 100000 | 6.6 s | 323 MiB | 1.5 s | 128 MiB |
| 150000 | 11.0 s | 483 MiB | 2.3 s | 193 MiB |
| 250000 | 20.1 s | 797 MiB | 4.4 s | 316 MiB |
| 500000 | 48.4 s | 1602 MiB | 11.1 s | 640 MiB |

### Replace commit (a target of N triples replaced by a staged job)

| target triples | staged triples | commit time | peak memory |
|---|---|---|---|
| 100000 | 100000 | 9.4 s | 424 MiB |
| 250000 | 100000 | 12.8 s | 642 MiB |
| 500000 | 100000 | 17.8 s | 958 MiB |

### Cost of one ground write (INSERT DATA of one triple) versus store size

| store | per write |
|---|---|
| empty | 0.02 ms |
| 1000000 triples | 0.03 ms |

### Reading the results

| Question | Result | Verdict |
|---|---|---|
| Does an `ingest_batch` of 5,000 triples take at most 300 ms (95th percentile)? | 134 ms, about 44,000 triples/s | Yes |
| Does parsing need about 10x its input in memory? | 5.8x | Better than assumed |
| Does a staged commit take at most 10 s, and at most 1 GiB? | 100,000 triples: **6.6 s**, 323 MiB. 150,000: 11.0 s, 483 MiB. 250,000: 20.1 s, 797 MiB. 500,000: **48.4 s**, **1.6 GiB** | Only up to 100,000 triples |
| Does a replace follow the same rules? | A 100,000-triple target replaced by 100,000 staged: 9.4 s, 424 MiB. With a 250,000-triple target: 12.8 s. With 500,000: 17.8 s, 958 MiB | Only up to 100,000 + 100,000 |
| Does a ground write stay cheap as the store grows? | 0.02 ms in an empty store, 0.03 ms with 1,000,000 triples (1.5x) | Yes: the hardening promise holds |
| Is an abort bounded? | 100,000 staged triples: 1.5 s. 500,000: 11.1 s | Yes under the new ceiling |

**Consequences, applied.** The provisional defaults were wrong, in the
direction the review of the design had feared:

*   `max_job_triples_staged` was **500,000**. A commit of that size holds the
    write gate for 48 s, longer than the default `write_wait_timeout_secs` (30),
    so every other writer would have been refused with `write_busy` during it,
    and it needs 1.6 GiB. It is now **100,000**.
*   `max_replace_triples` was **1,000,000**. It is now **200,000** (the target's
    size plus the staged ceiling), which keeps a replace within the same
    10 s.

Larger loads are still possible: send several jobs, or use `mode: "direct"`,
whose ceiling is 10,000,000 triples and which has no commit transaction (its
cost is the batches, at about 44,000 triples/s).

**What the figures say about the cost.** A commit takes about 66 microseconds
and 3.2 KiB of memory per triple, which is high. Oxigraph keeps every change
of a transaction in memory, and `ADD` followed by `DROP` writes each triple's
index entries twice (once into the target, once removed from the staging graph).
A commit that moves the quads directly, without the SPARQL update machinery,
might be faster; that is a possible optimisation, not done. The ceiling would
then be raised only after rerunning this benchmark.

**Not measured.** `ingest_file` (it does not exist yet), so the design's target
of 50,000 triples/s for a file job is not checked; and reads during an
ingestion (the design's target of a 95th percentile under 200 ms for
`query_sparql` while a file job runs).


## Limits of this benchmark

*   **In-process.** No MCP transport, JSON, or agent. A real call adds a small,
    constant cost per batch.
*   **Warm.** The store is created for the step; RocksDB's cache is cold at the
    start and warms as it goes. A store that has been used for weeks will
    behave differently, especially for reads.
*   **One writer.** The benchmark does not measure contention. The concurrency
    behaviour (retries, races with a commit) is covered by the tests in
    `crates/sparql-mcp-core/tests/ingest_jobs.rs`, which are about correctness,
    not speed.
*   **Memory is sampled,** every 10 ms, from the process working set. A short
    spike between two samples is missed, so treat the peaks as lower bounds.
    Memory is read with the `memory-stats` crate (a development dependency
    only).
*   **Synthetic data.** Real data has different term sizes and more repeated
    IRIs. The bytes per triple (about 100) are in line with the code and
    ontology graphs this project loads, but check before relying on a limit for
    very different data.
