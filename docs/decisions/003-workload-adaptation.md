# 003 — Calibrate on real work, adjust admission

Phase 3 keeps hardware adaptation opt-in and reuses the safe Phase 2 copy paths.
Hardware classification establishes a conservative ceiling; explicit worker and
chunk overrides retain precedence. Unknown topology never establishes physical
independence. Per-file checks also protect nested mounts from automatic chunking.

`--tune` calibrates while copying actual user-requested data. One-second windows
compare a fixed score: attempted bytes plus 4 KiB per committed file, divided by
elapsed seconds. The file weight makes small-file metadata work visible without
switching units between windows. Saturation and minimum-work gates avoid tuning
idle or unchanged-sync runs. Increasing candidates stop at a 5% improvement
threshold, the hardware ceiling, or eight sample windows. Unmeasured candidates
are discarded when a short transfer ends. This introduces no synthetic writes,
duplicate calibration reads, or destructive storage operations.

Pressure feedback halves admission and restores one slot after three quiet,
saturated windows, up to the best observed limit. Manual workers and `--no-tune`
disable runtime resizing. The scheduler retains a fixed thread/buffer ceiling
and changes permit admission under its existing mutex. Shrinking can temporarily
leave more active tasks than the new limit; no new permits are admitted until
those tasks drain. Helpers still acquire only idle slots and never block on
nested permits. Existing files are neither restarted nor repartitioned.

This approach favors bounded resource use and unchanged filesystem guarantees
over aggressive mid-file adaptation. Mixed workloads, warm cache, durability
stalls and other processes can bias short comparisons. Ten-second PSI averages
also react slowly. The tuner does not promise an optimum. Persistent calibration
caches are deferred until storage/workload comparability can be validated.

Hardware inputs follow the kernel [cgroup v2 interfaces](https://docs.kernel.org/admin-guide/cgroup-v2.html)
and [pressure format](https://docs.kernel.org/accounting/psi.html). Free-space
reporting uses the Linux 64-bit [statvfs layout](https://github.com/bminor/glibc/blob/master/sysdeps/unix/sysv/linux/bits/statvfs.h)
on a retained directory descriptor and remains advisory; rollback-safe ENOSPC
handling is authoritative.
