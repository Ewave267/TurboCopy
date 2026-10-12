# Graph Report - turboCopy  (2026-10-11)

## Corpus Check
- cluster-only mode — file stats not available

## Summary
- 288 nodes · 665 edges · 18 communities (9 shown, 9 thin omitted)
- Extraction: 99% EXTRACTED · 1% INFERRED · 0% AMBIGUOUS · INFERRED: 4 edges (avg confidence: 0.85)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `c063fd0e`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- Community 0
- Community 1
- Community 2
- Community 3
- Community 4
- Community 5
- Community 6
- Community 7
- Community 8
- Community 14

## God Nodes (most connected - your core abstractions)
1. `Summary` - 22 edges
2. `Options` - 19 edges
3. `success()` - 16 edges
4. `DIRECTORY` - 14 edges
5. `Concurrency` - 14 edges
6. `main()` - 13 edges
7. `main()` - 13 edges
8. `copy()` - 12 edges
9. `Task` - 11 edges
10. `Pool` - 11 edges

## Surprising Connections (you probably didn't know these)
- `Summary` --references--> `Options`  [EXTRACTED]
  src/progress.rs → src/cli.rs
- `Concurrency` --references--> `Summary`  [EXTRACTED]
  src/scheduler.rs → src/progress.rs
- `Pool` --references--> `Summary`  [EXTRACTED]
  src/scheduler.rs → src/progress.rs
- `run()` --references--> `Summary`  [EXTRACTED]
  src/scheduler.rs → src/progress.rs
- `storage_json()` --references--> `Storage`  [EXTRACTED]
  src/progress.rs → src/storage_detect.rs

## Import Cycles
- 2-file cycle: `src/copy_engine.rs -> src/progress.rs -> src/copy_engine.rs`
- 2-file cycle: `src/copy_engine.rs -> src/scheduler.rs -> src/copy_engine.rs`
- 3-file cycle: `src/copy_engine.rs -> src/scheduler.rs -> src/progress.rs -> src/copy_engine.rs`

## Communities (18 total, 9 thin omitted)

### Community 0 - "Community 0"
Cohesion: 0.09
Nodes (17): check_cancelled(), DIRECTORY, fstatvfs(), invalid(), NOFOLLOW, NONBLOCK, on_signal(), optional_metadata() (+9 more)

### Community 1 - "Community 1"
Cohesion: 0.07
Nodes (18): Hardware, count(), storage_json(), StoragePair, Summary, Timer, Timer<'a>, Storage (+10 more)

### Community 2 - "Community 2"
Cohesion: 0.15
Nodes (29): adaptive_flags_preserve_manual_controls_and_dry_run(), changing_source_is_reported_without_publishing_partial_data(), chunk_controls_copy_partial_tail_and_share_budget_across_files(), cli_help_version_and_invalid_arguments(), copies_file_and_atomically_overwrites_without_changing_hardlink_alias(), destination_parent_symlink_swap_cannot_redirect_opened_directory(), directory_file_conflicts_fail_without_truncating_existing_data(), dry_run_creates_nothing_and_preserves_destination_metadata() (+21 more)

### Community 3 - "Community 3"
Cohesion: 0.10
Nodes (15): Chunks, copy(), handles_short_writes_interrupts_and_disk_full(), NEXT_TEMP, Outcome, Copied, Skipped, positioned_ranges_preserve_cursors_and_reject_short_sources() (+7 more)

### Community 4 - "Community 4"
Cohesion: 0.13
Nodes (16): Action, Copy, Help, Version, Command, HELP, Options, parse() (+8 more)

### Community 5 - "Community 5"
Cohesion: 0.15
Nodes (7): Task, Concurrency, Permit, permit_budget_bounds_concurrent_work_and_returns_slots(), Pool, run(), shrinking_live_budget_drains_existing_tasks_and_growth_wakes_waiters()

### Community 6 - "Community 6"
Cohesion: 0.24
Nodes (6): durable_tree(), main(), manifest(), digest(), main(), transfer()

### Community 7 - "Community 7"
Cohesion: 0.21
Nodes (9): cgroup_mount_roots_map_process_paths_without_parent_traversal(), Detector, device_id(), Mount, mount_escapes_longest_match_and_unknown_are_conservative(), parse_mounts(), topology(), topology_resolves_partition_mapper_and_rotational_leaf() (+1 more)

### Community 8 - "Community 8"
Cohesion: 0.24
Nodes (8): ancestor_cgroup_limits_use_remaining_memory_and_smallest_quota(), cgroup(), cpu_quota(), current_pressure(), memory_available(), physical_cores(), pressure(), read()

## Knowledge Gaps
- **6 isolated node(s):** `Timer<'a>`, `turbocopy`, `Skipped`, `Copy`, `Help` (+1 more)
  These have ≤1 connection - possible missing edges. (Counts symbols only; 61 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **9 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `Summary` connect `Community 1` to `Community 3`, `Community 4`, `Community 5`?**
  _High betweenness centrality (0.096) - this node is a cross-community bridge._
- **What connects `Timer<'a>`, `turbocopy`, `Skipped` to the rest of the system?**
  _6 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Community 0` be split into smaller, more focused modules?**
  _Cohesion score 0.08686868686868687 - nodes in this community are weakly interconnected._
- **Why does `Options` connect `Community 4` to `Community 1`, `Community 3`, `Community 5`?**
  _High betweenness centrality (0.088) - this node is a cross-community bridge._
- **Should `Community 1` be split into smaller, more focused modules?**
  _Cohesion score 0.07474747474747474 - nodes in this community are weakly interconnected._
- **Should `Community 3` be split into smaller, more focused modules?**
  _Cohesion score 0.10338680926916222 - nodes in this community are weakly interconnected._
- **Should `Community 4` be split into smaller, more focused modules?**
  _Cohesion score 0.12807881773399016 - nodes in this community are weakly interconnected._