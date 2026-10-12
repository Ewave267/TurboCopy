# Graph Report - turboCopy  (2026-10-11)

## Corpus Check
- cluster-only mode — file stats not available

## Summary
- 182 nodes · 396 edges · 11 communities (7 shown, 4 thin omitted)
- Extraction: 100% EXTRACTED · 0% INFERRED · 0% AMBIGUOUS
- Token cost: 0 input · 0 output

## Community Hubs (Navigation)
- Community 0
- Community 1
- Community 2
- Community 3
- Community 4
- Community 5
- Community 7
- Community 8

## God Nodes (most connected - your core abstractions)
1. `main()` - 13 edges
2. `success()` - 13 edges
3. `DIRECTORY` - 13 edges
4. `Task` - 8 edges
5. `Pool` - 8 edges
6. `destination_parent_symlink_swap_cannot_redirect_opened_directory()` - 8 edges
7. `Fixture` - 7 edges
8. `Summary` - 7 edges
9. `changing_source_is_reported_without_publishing_partial_data()` - 7 edges
10. `sigint_and_sigterm_keep_old_destination_and_clean_up()` - 7 edges

## Surprising Connections (you probably didn't know these)
- `copy()` --references--> `Options`  [EXTRACTED]
  src/copy_engine.rs → src/cli.rs
- `run()` --references--> `Options`  [EXTRACTED]
  src/scheduler.rs → src/cli.rs
- `Pool` --references--> `Task`  [EXTRACTED]
  src/scheduler.rs → src/copy_engine.rs
- `run()` --references--> `Task`  [EXTRACTED]
  src/scheduler.rs → src/copy_engine.rs

## Import Cycles
- None detected.

## Communities (11 total, 4 thin omitted)

### Community 0 - "Community 0"
Cohesion: 0.16
Nodes (26): changing_source_is_reported_without_publishing_partial_data(), cli_help_version_and_invalid_arguments(), copies_file_and_atomically_overwrites_without_changing_hardlink_alias(), destination_parent_symlink_swap_cannot_redirect_opened_directory(), directory_file_conflicts_fail_without_truncating_existing_data(), dry_run_creates_nothing_and_preserves_destination_metadata(), explicit_dot_copies_contents_without_an_extra_directory(), Fixture (+18 more)

### Community 1 - "Community 1"
Cohesion: 0.10
Nodes (15): check_cancelled(), DIRECTORY, invalid(), NOFOLLOW, NONBLOCK, on_signal(), optional_metadata(), preserve_metadata() (+7 more)

### Community 2 - "Community 2"
Cohesion: 0.22
Nodes (7): Action, Copy, Help, Version, Command, HELP, parse()

### Community 3 - "Community 3"
Cohesion: 0.11
Nodes (7): handles_short_writes_interrupts_and_disk_full(), NEXT_TEMP, retries_interrupted_reads_and_accepts_partial_reads(), short_source_is_failure_and_growth_is_bounded(), ShortWriter, Temporary, transfer()

### Community 4 - "Community 4"
Cohesion: 0.14
Nodes (9): Options, copy(), Outcome, Copied, Skipped, Task, Pool, run() (+1 more)

### Community 5 - "Community 5"
Cohesion: 0.22
Nodes (3): durable_tree(), main(), manifest()

### Community 8 - "Community 8"
Cohesion: 0.20
Nodes (7): enter(), Frame, parent(), plan(), preview(), scan(), trim_slashes()

## Knowledge Gaps
- **5 isolated node(s):** `Copy`, `Help`, `Version`, `Skipped`, `turbocopy`
  These have ≤1 connection - possible missing edges. (Counts symbols only; 31 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **4 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `Options` connect `Community 4` to `Community 2`, `Community 3`?**
  _High betweenness centrality (0.028) - this node is a cross-community bridge._
- **What connects `Copy`, `Help`, `Version` to the rest of the system?**
  _5 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Community 1` be split into smaller, more focused modules?**
  _Cohesion score 0.10256410256410256 - nodes in this community are weakly interconnected._
- **Should `Community 3` be split into smaller, more focused modules?**
  _Cohesion score 0.1076923076923077 - nodes in this community are weakly interconnected._
- **Should `Community 4` be split into smaller, more focused modules?**
  _Cohesion score 0.14492753623188406 - nodes in this community are weakly interconnected._