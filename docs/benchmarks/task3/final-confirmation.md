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
| many-1mib | auto after | 5 | 567.1 (433.3–579.1) | 567.1 | 7.2231 | 201.6 | 5.3 | 40 |
| many-1mib | auto before | 5 | 209.3 (197.2–215.0) | 209.3 | 19.5664 | 64.3 | 3.5 | 16 |
| many-small | auto after | 5 | 63.3 (52.0–101.9) | 1013.5 | 4.0415 | 220.5 | 5.2 | 40 |
| many-small | auto before | 5 | 20.5 (16.3–32.6) | 327.8 | 12.4958 | 46.3 | 3.5 | 16 |
| mixed | auto after | 5 | 479.8 (134.2–488.9) | 2668.2 | 0.8170 | 291.3 | 5.5 | 40 |
| mixed | auto before | 5 | 48.0 (43.4–190.0) | 266.8 | 8.1714 | 42.4 | 3.8 | 16 |
| one-large | auto after | 5 | 1006.4 (985.6–1016.6) | 2.0 | 0.5087 | 93.2 | 3.6 | 10 |
| one-large | auto before | 5 | 958.0 (922.2–999.6) | 1.9 | 0.5344 | 90.0 | 3.6 | 10 |
| several-large | auto after | 5 | 1417.5 (1388.8–1434.5) | 22.1 | 0.3612 | 201.7 | 3.7 | 16 |
| several-large | auto before | 5 | 1408.9 (1376.7–1433.0) | 22.0 | 0.3634 | 201.7 | 3.7 | 16 |
| short | auto after | 5 | 0.6 (0.5–0.7) | 143.9 | 0.0070 | 75.7 | 2.7 | 6 |
| short | auto before | 5 | 0.6 (0.5–0.7) | 156.5 | 0.0064 | 74.9 | 2.8 | 6 |
