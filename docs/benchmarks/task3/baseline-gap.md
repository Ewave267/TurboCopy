# Paired worker-policy benchmark

Status: completed

TurboCopy timings include its native fsync barriers; cp/rsync timings include a final
external tree fsync. All samples require source-page eviction and exact content/metadata
verification. Device/controller and metadata caches are uncontrolled. CPU includes
the final barrier. RSS from child rusage can include pre-exec Python memory;
sampled RSS/FD peaks observe the actual executable at 100 ms intervals and may miss peaks.

| Workload | Tool | n | MiB/s median (range) | Files/s | Seconds | CPU % | Sampled RSS MiB | Sampled FDs |
| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| many-1mib | auto before | 5 | 232.3 (199.1–240.6) | 232.3 | 17.6293 | 67.2 | 3.6 | 16 |
| many-1mib | cp | 5 | 175.2 (156.0–327.1) | 175.2 | 23.3793 | 19.7 | 0.0 | 0 |
| many-1mib | manual 16 | 5 | 563.0 (472.5–727.2) | 563.0 | 7.2748 | 194.0 | 5.3 | 40 |
| many-1mib | manual 4 | 5 | 232.6 (200.3–248.4) | 232.6 | 17.6117 | 66.2 | 3.6 | 16 |
| many-1mib | manual 8 | 5 | 399.2 (311.5–468.1) | 399.2 | 10.2608 | 141.7 | 4.1 | 24 |
| many-1mib | rsync | 5 | 189.9 (152.7–337.9) | 189.9 | 21.5717 | 31.7 | 0.0 | 0 |
