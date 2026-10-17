# Paired worker-policy benchmark

Status: completed

TurboCopy timings include its native fsync barriers; cp/rsync timings include a final
external tree fsync. Source cache preparation and residency are recorded per sample.
Every sample requires exact content/metadata verification. Device/controller and metadata
caches are uncontrolled. CPU includes
the final barrier. RSS from child rusage can include pre-exec Python memory;
sampled RSS/FD peaks observe the actual executable at 100 ms intervals and may miss peaks.

| Workload | Tool | n | MiB/s median (range) | Files/s | Seconds | CPU % | Sampled RSS MiB | Sampled FDs |
| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| mixed | auto after | 9 | 7069.7 (5380.9–7407.1) | 39316.3 | 0.0554 | 400.5 | 2.7 | 6 |
| mixed | auto before | 9 | 7160.8 (5308.8–7578.3) | 39822.8 | 0.0547 | 399.6 | 2.7 | 6 |
