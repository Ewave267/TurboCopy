import tempfile
from pathlib import Path
import unittest
import signal
from unittest.mock import patch

import worker_selection as benchmark


class WorkerSelectionTests(unittest.TestCase):
    def test_configured_reserve_is_checked_before_dataset_allocation(self):
        with tempfile.TemporaryDirectory() as owned:
            root = Path(owned).resolve()
            args = ['worker_selection', '--before', '/bin/true', '--directory', str(root),
                    '--workload', 'short', '--reserve-gib', '1', '--output', str(root / 'report')]
            previous = signal.getsignal(signal.SIGTERM)
            try:
                with patch('sys.argv', args), patch.object(benchmark.phase5, 'filesystem', return_value={}), \
                     patch.object(benchmark.phase5, 'check_space', side_effect=benchmark.phase5.StopBenchmark('test stop')) as guard:
                    self.assertEqual(benchmark.main(), 1)
                    guard.assert_called_once_with([root, root], [8192, 8192], 1.0, .10)
            finally:
                signal.signal(signal.SIGTERM, previous)

    def test_wait_sampler_excludes_coordinator_and_monitor_threads(self):
        for name in ('tcopy-before', 'tcopy-after', 'tcopy-final', 'tcopy-progress', 'main'):
            self.assertFalse(benchmark.is_worker(name))
        self.assertTrue(benchmark.is_worker('tcopy-0'))
        self.assertTrue(benchmark.is_worker('tcopy-127'))

    def test_mixed_source_has_exact_sizes_and_matching_copy_manifest(self):
        with tempfile.TemporaryDirectory() as owned:
            source = Path(owned) / 'source'
            benchmark.make_source(source, [(3, 4096), (2, 8192)], lambda: None)
            self.assertEqual(sorted(p.stat().st_size for p in source.iterdir()), [4096] * 3 + [8192] * 2)
            target = Path(owned) / 'target'
            benchmark.shutil.copytree(source, target)
            self.assertEqual(benchmark.phase5.checked_manifest(source), benchmark.phase5.checked_manifest(target))
            next(target.iterdir()).write_bytes(b'corrupt')
            self.assertNotEqual(benchmark.phase5.checked_manifest(source), benchmark.phase5.checked_manifest(target))

    def test_summary_reports_median_range_and_missing_short_process_samples(self):
        samples = [dict(workload='short', tool='auto', seconds=n, mib_s=n, files_s=n,
                        cpu_percent=n, max_rss_kib=n, sampled_peak_rss_kib=None,
                        sampled_peak_fds=None) for n in (3, 1, 2)]
        row, = benchmark.summarize(samples)
        self.assertEqual(row['seconds'], dict(median=2, min=1, max=3))
        self.assertEqual(row['n'], 3)
        self.assertIsNone(row['sampled_peak_fds'])
