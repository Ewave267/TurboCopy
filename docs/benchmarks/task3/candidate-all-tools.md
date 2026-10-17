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
| many-1mib | auto after | 3 | 505.9 (434.7–552.9) | 505.9 | 8.0970 | 203.7 | 5.3 | 40 |
| many-1mib | auto before | 3 | 229.1 (214.7–255.8) | 229.1 | 17.8771 | 69.4 | 3.5 | 16 |
| many-1mib | cp | 3 | 304.6 (171.8–352.2) | 304.6 | 13.4488 | 31.7 | n/a | n/a |
| many-1mib | manual 16 | 3 | 524.4 (519.6–571.2) | 524.4 | 7.8103 | 200.4 | 5.3 | 40 |
| many-1mib | manual 4 | 3 | 199.1 (184.7–212.8) | 199.1 | 20.5700 | 66.7 | 3.6 | 16 |
| many-1mib | manual 8 | 3 | 364.8 (307.0–585.6) | 364.8 | 11.2294 | 141.9 | 4.2 | 24 |
| many-1mib | rsync | 3 | 165.0 (160.7–314.8) | 165.0 | 24.8303 | 29.1 | n/a | n/a |
| many-small | auto after | 3 | 71.2 (52.5–185.6) | 1140.0 | 3.5931 | 214.5 | 5.3 | 40 |
| many-small | auto before | 3 | 23.2 (21.8–65.0) | 371.5 | 11.0265 | 54.1 | 3.6 | 16 |
| many-small | cp | 3 | 39.3 (12.6–41.2) | 629.2 | 6.5097 | 38.1 | n/a | n/a |
| many-small | manual 16 | 3 | 195.5 (59.3–208.1) | 3128.6 | 1.3092 | 345.2 | 5.3 | 40 |
| many-small | manual 4 | 3 | 17.9 (15.7–69.1) | 286.8 | 14.2830 | 38.7 | 3.5 | 16 |
| many-small | manual 8 | 3 | 28.7 (27.2–107.5) | 459.5 | 8.9147 | 109.5 | 4.2 | 24 |
| many-small | rsync | 3 | 42.6 (19.4–44.3) | 681.8 | 6.0079 | 41.9 | n/a | n/a |
| mixed | auto after | 3 | 133.3 (129.0–485.1) | 741.5 | 2.9398 | 185.3 | 5.5 | 40 |
| mixed | auto before | 3 | 174.1 (93.0–190.2) | 968.5 | 2.2510 | 119.7 | 3.7 | 16 |
| mixed | cp | 3 | 37.2 (37.0–40.1) | 207.1 | 10.5240 | 15.2 | n/a | n/a |
| mixed | manual 16 | 3 | 376.9 (131.8–399.3) | 2096.3 | 1.0399 | 243.5 | 5.5 | 40 |
| mixed | manual 4 | 3 | 43.9 (42.9–187.3) | 244.2 | 8.9267 | 42.9 | 3.7 | 16 |
| mixed | manual 8 | 3 | 169.9 (77.7–258.3) | 945.0 | 2.3070 | 156.6 | 4.2 | 24 |
| mixed | rsync | 3 | 62.5 (45.7–120.5) | 347.6 | 6.2714 | 26.9 | n/a | n/a |
| one-large | auto after | 3 | 1004.9 (612.2–1007.4) | 2.0 | 0.5095 | 92.3 | 3.6 | 10 |
| one-large | auto before | 3 | 996.3 (976.5–1013.6) | 1.9 | 0.5139 | 92.9 | 3.6 | 10 |
| one-large | cp | 3 | 619.2 (603.7–1006.2) | 1.2 | 0.8269 | 59.3 | n/a | n/a |
| one-large | manual 16 | 3 | 996.1 (601.8–998.7) | 1.9 | 0.5140 | 94.5 | 5.1 | 10 |
| one-large | manual 4 | 3 | 1003.6 (1003.3–1004.2) | 2.0 | 0.5101 | 93.3 | 3.5 | 10 |
| one-large | manual 8 | 3 | 1004.4 (998.9–1004.5) | 2.0 | 0.5098 | 92.2 | 4.1 | 10 |
| one-large | rsync | 3 | 703.3 (698.9–709.6) | 1.4 | 0.7280 | 108.0 | n/a | n/a |
| several-large | auto after | 3 | 731.2 (727.6–1287.6) | 11.4 | 0.7002 | 100.0 | 3.6 | 16 |
| several-large | auto before | 3 | 1051.4 (801.5–1219.8) | 16.4 | 0.4870 | 150.5 | 3.7 | 16 |
| several-large | cp | 3 | 558.5 (552.2–931.5) | 8.7 | 0.9167 | 60.3 | n/a | n/a |
| several-large | manual 16 | 3 | 863.9 (748.2–1323.6) | 13.5 | 0.5926 | 112.5 | 5.4 | 24 |
| several-large | manual 4 | 3 | 733.9 (731.6–812.6) | 11.5 | 0.6977 | 96.4 | 3.6 | 16 |
| several-large | manual 8 | 3 | 1355.7 (746.6–1362.9) | 21.2 | 0.3777 | 167.8 | 4.4 | 24 |
| several-large | rsync | 3 | 643.5 (609.3–647.1) | 10.1 | 0.7956 | 99.3 | n/a | n/a |
| short | auto after | 3 | 0.2 (0.2–0.2) | 43.6 | 0.0229 | 32.1 | 2.7 | 6 |
| short | auto before | 3 | 0.2 (0.2–0.2) | 45.1 | 0.0222 | 33.1 | 2.7 | 6 |
| short | cp | 3 | 0.3 (0.3–0.3) | 64.3 | 0.0156 | 20.0 | n/a | n/a |
| short | manual 16 | 3 | 0.2 (0.2–0.2) | 43.9 | 0.0228 | 48.2 | 2.7 | 6 |
| short | manual 4 | 3 | 0.2 (0.2–0.2) | 46.3 | 0.0216 | 30.9 | 2.7 | 5 |
| short | manual 8 | 3 | 0.2 (0.2–0.2) | 43.8 | 0.0228 | 39.3 | 2.6 | 6 |
| short | rsync | 3 | 0.1 (0.1–0.1) | 16.1 | 0.0619 | 18.7 | n/a | n/a |
