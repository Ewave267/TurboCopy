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
| many-small | auto after | 3 | 3628.9 (3321.0–3832.7) | 58062.3 | 0.0705 | 415.8 | 2.5 | 5 |
| many-small | auto before | 3 | 3230.1 (3103.4–3353.7) | 51681.1 | 0.0793 | 409.1 | 2.4 | 6 |
| mixed | auto after | 3 | 6559.7 (6417.4–7686.9) | 36480.0 | 0.0598 | 401.8 | 2.7 | 6 |
| mixed | auto before | 3 | 7207.5 (7118.1–7380.1) | 40082.3 | 0.0544 | 400.5 | 2.7 | 6 |
| short | auto after | 3 | 2.3 (2.3–2.3) | 589.5 | 0.0017 | 113.5 | 2.7 | 5 |
| short | auto before | 3 | 2.3 (1.9–2.3) | 584.4 | 0.0017 | 115.8 | 2.7 | 6 |
