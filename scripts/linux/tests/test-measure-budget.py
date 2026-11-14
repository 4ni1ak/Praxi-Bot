#!/usr/bin/env python3
"""Hermetic fixture tests for scripts/linux/measure-budget.py."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "measure-budget.py"
MIB = 1024 * 1024
GIB = 1024 * 1024 * 1024


def write_proc_entry(
    proc_root: Path,
    pid: int,
    comm: str,
    ppid: int,
    uid: int,
    utime: int,
    stime: int,
    starttime: int,
    rss_kib: int,
    pss_kib: int | None,
    cmdline_secret: str = "https://secret.example.invalid/token=XYZ123",
    cgroup_v2: str | None = None,
) -> None:
    pdir = proc_root / str(pid)
    pdir.mkdir(parents=True, exist_ok=True)
    # 52 fields total after (comm); indices after ')':
    # 0=state, 1=ppid, ..., 11=utime, 12=stime, ..., 19=starttime
    rest = ["S", str(ppid)] + ["0"] * 9 + [str(utime), str(stime)] + ["0"] * 6 + [str(starttime)] + ["0"] * 10
    (pdir / "stat").write_text(f"{pid} ({comm}) {' '.join(rest)}\n", encoding="utf-8")
    (pdir / "status").write_text(
        f"Name:\t{comm}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nVmRSS:\t{rss_kib} kB\nVmHWM:\t{rss_kib + 512} kB\n",
        encoding="utf-8",
    )
    if pss_kib is not None:
        (pdir / "smaps_rollup").write_text(
            f"Rss:\t{rss_kib} kB\nPss:\t{pss_kib} kB\nPss_Anon:\t{pss_kib // 2} kB\nPss_File:\t{pss_kib // 2} kB\nPss_Shmem:\t0 kB\n",
            encoding="utf-8",
        )
    if cgroup_v2 is not None:
        (pdir / "cgroup").write_text(f"0::{cgroup_v2}\n", encoding="utf-8")
    # Plant a secret in cmdline & environ to assert measure-budget NEVER reads or outputs them
    (pdir / "cmdline").write_text(f"{comm}\0{cmdline_secret}\0", encoding="utf-8")
    (pdir / "environ").write_text(f"BOT_SECRETS_PASSPHRASE=SUPERSECRET\0", encoding="utf-8")


def write_meminfo_vmstat(
    proc_root: Path,
    mem_total_kib: int,
    mem_avail_kib: int,
    swap_total_kib: int = 4 * 1024 * 1024,
    swap_free_kib: int = 4 * 1024 * 1024,
    pswpin: int = 0,
    pswpout: int = 0,
    oom_kill: int = 0,
) -> None:
    proc_root.mkdir(parents=True, exist_ok=True)
    (proc_root / "meminfo").write_text(
        f"MemTotal:       {mem_total_kib} kB\n"
        f"MemFree:        {mem_avail_kib // 2} kB\n"
        f"MemAvailable:   {mem_avail_kib} kB\n"
        f"SwapTotal:      {swap_total_kib} kB\n"
        f"SwapFree:       {swap_free_kib} kB\n",
        encoding="utf-8",
    )
    (proc_root / "vmstat").write_text(
        f"pswpin {pswpin}\npswpout {pswpout}\noom_kill {oom_kill}\n",
        encoding="utf-8",
    )


def write_cgroup(
    cg_root: Path,
    rel: str,
    mem_cur: int,
    mem_peak: int | None,
    usage_usec: int,
    pids: list[int],
    oom_kill: int = 0,
    high_events: int = 0,
    max_events: int = 0,
) -> None:
    d = cg_root / rel.lstrip("/")
    d.mkdir(parents=True, exist_ok=True)
    (d / "memory.current").write_text(f"{mem_cur}\n", encoding="utf-8")
    if mem_peak is not None:
        (d / "memory.peak").write_text(f"{mem_peak}\n", encoding="utf-8")
    (d / "memory.high").write_text(f"{3 * GIB}\n", encoding="utf-8")
    (d / "memory.max").write_text(f"{4 * GIB}\n", encoding="utf-8")
    (d / "memory.events").write_text(
        f"high {high_events}\nmax {max_events}\noom 0\noom_kill {oom_kill}\noom_group_kill 0\n",
        encoding="utf-8",
    )
    (d / "cpu.stat").write_text(f"usage_usec {usage_usec}\nuser_usec {usage_usec}\nsystem_usec 0\nnr_throttled 0\nthrottled_usec 0\n", encoding="utf-8")
    (d / "cgroup.procs").write_text("\n".join(str(p) for p in pids) + ("\n" if pids else ""), encoding="utf-8")


class MeasureBudgetTests(unittest.TestCase):
    def run_cli(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_dry_run(self) -> None:
        res = self.run_cli("--dry-run", "--label", "smoke")
        self.assertEqual(res.returncode, 0, res.stderr)
        data = json.loads(res.stdout)
        self.assertTrue(data["dry_run"])
        self.assertEqual(data["label"], "smoke")

    def test_multi_tick_pid_reuse_vanished_and_privacy(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fix = Path(tmp)
            (fix / "meta.json").write_text(
                json.dumps(
                    {
                        "arch": "aarch64",
                        "kernel": "6.8.0-fixture",
                        "os": "Ubuntu 24.04 Fixture",
                        "browser_version": "Google Chrome for Testing 157.0.8103.38",
                        "clk_tck": 100,
                        "praxi_uid": 999,
                    }
                ),
                encoding="utf-8",
            )
            # Tick 0: pid 100 (bot), pid 200 (short-lived child starttime=1000)
            t0 = fix / "samples" / "tick_000"
            (t0 / "tick.json").parent.mkdir(parents=True, exist_ok=True)
            (t0 / "tick.json").write_text('{"elapsed_s": 0.0}', encoding="utf-8")
            write_meminfo_vmstat(t0 / "proc", 6 * 1024 * 1024, 5 * 1024 * 1024)
            write_proc_entry(t0 / "proc", 100, "bot", 1, 999, 10, 5, 100, 60_000, 45_000)
            write_proc_entry(t0 / "proc", 200, "chrome", 100, 999, 500, 100, 1000, 200_000, 150_000)
            # Vanished PID stub: directory with missing stat
            (t0 / "proc" / "250").mkdir(parents=True, exist_ok=True)
            write_cgroup(t0 / "cgroup", "/praxi.slice", 300 * MIB, 320 * MIB, 100_000, [100, 200])
            write_cgroup(t0 / "cgroup", "/praxi.slice/praxi.service", 60 * MIB, 60 * MIB, 20_000, [100])

            # Tick 1 (1.0s later): pid 200 reused by new process with starttime=5000 and smaller utime!
            t1 = fix / "samples" / "tick_001"
            (t1 / "tick.json").parent.mkdir(parents=True, exist_ok=True)
            (t1 / "tick.json").write_text('{"elapsed_s": 1.0}', encoding="utf-8")
            write_meminfo_vmstat(t1 / "proc", 6 * 1024 * 1024, 4800 * 1024)
            write_proc_entry(t1 / "proc", 100, "bot", 1, 999, 15, 5, 100, 62_000, 46_000)
            write_proc_entry(t1 / "proc", 200, "xfce4-terminal", 100, 999, 2, 1, 5000, 30_000, 20_000)
            write_cgroup(t1 / "cgroup", "/praxi.slice", 350 * MIB, 400 * MIB, 150_000, [100, 200])
            write_cgroup(t1 / "cgroup", "/praxi.slice/praxi.service", 62 * MIB, 65 * MIB, 30_000, [100])

            res = self.run_cli(
                "--fixture-dir", str(fix),
                "--label", "reuse-test",
                "--check-budget",
                "--require-slice-containment",
                "--format", "json",
            )
            self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
            self.assertNotIn("SUPERSECRET", res.stdout)
            self.assertNotIn("secret.example.invalid", res.stdout)
            data = json.loads(res.stdout)
            self.assertEqual(data["sampling"]["vanished_pid_events"], 1)
            self.assertTrue(data["budget"]["passed"])
            # Pid 200 appeared twice with two different starttimes -> two distinct process entries
            p200s = [p for p in data["processes"] if p["pid"] == 200]
            self.assertEqual(len(p200s), 2)
            self.assertEqual({p["starttime"] for p in p200s}, {1000, 5000})
            # Cgroup CPU delta: (150000 - 100000) usec over 1.0s = 5.0%
            self.assertEqual(data["praxi_tree"]["cpu_pct_of_one_core"], 5.0)
            self.assertTrue(data["sampling"]["pss_complete"])
            self.assertEqual(data["sampling"]["pss_incomplete_events"], 0)
            # Window max of memory.current is 350 MiB; lifetime memory.peak watermark is 400 MiB
            eff = data["praxi_tree"]["effective_peak"]
            self.assertEqual(eff["bytes"], 350 * MIB)
            self.assertEqual(eff["mib"], 350.0)
            self.assertEqual(data["praxi_tree"]["effective_peak_source"], "praxi.slice/memory.current_max")
            life = data["praxi_tree"]["lifetime_cgroup_memory_peak"]
            self.assertEqual(life["bytes"], 400 * MIB)
            self.assertEqual(life["mib"], 400.0)
            self.assertEqual(life["gb_decimal"], 0.419)
            self.assertTrue(any("double-counts" in c for c in data["caveats"]))

    def test_missing_pss_and_missing_memory_peak_fallback_and_bounded_samples(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fix = Path(tmp)
            for i in range(5):
                t = fix / "samples" / f"tick_{i:03d}"
                (t / "tick.json").parent.mkdir(parents=True, exist_ok=True)
                (t / "tick.json").write_text(json.dumps({"elapsed_s": i * 0.5}), encoding="utf-8")
                write_meminfo_vmstat(t / "proc", 6 * 1024 * 1024, 5 * 1024 * 1024)
                # No smaps_rollup (pss_kib=None)
                write_proc_entry(t / "proc", 10, "bot", 1, 999, 10 + i, 0, 10, 50_000, None)
                # No memory.peak (mem_peak=None)
                write_cgroup(t / "cgroup", "/praxi.slice", (100 + i * 10) * MIB, None, 10_000 * i, [10])

            res = self.run_cli(
                "--fixture-dir", str(fix),
                "--max-samples", "2",
                "--format", "json",
            )
            self.assertEqual(res.returncode, 0, res.stderr)
            data = json.loads(res.stdout)
            self.assertFalse(data["sampling"]["pss_available"])
            self.assertFalse(data["cgroups"]["praxi.slice"]["memory_peak_available"])
            self.assertEqual(data["praxi_tree"]["effective_peak"]["bytes"], 140 * MIB)
            self.assertEqual(data["praxi_tree"]["effective_peak_source"], "praxi.slice/memory.current_max")
            self.assertTrue(data["sampling"]["samples_truncated"])
            self.assertEqual(len(data["samples"]), 2)

    def test_over_budget_and_oom_exits_nonzero(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fix = Path(tmp)
            write_meminfo_vmstat(fix / "proc", 6 * 1024 * 1024, 100 * 1024, oom_kill=1)
            write_proc_entry(fix / "proc", 10, "chrome", 1, 999, 10, 0, 10, 2_600_000, 2_500_000)
            # 2.6 GB decimal (> 2.5 GB budget)
            write_cgroup(fix / "cgroup", "/praxi.slice", 2_600_000_000, 2_600_000_000, 10_000, [10], oom_kill=1)

            res = self.run_cli(
                "--fixture-dir", str(fix),
                "--check-budget",
                "--format", "json",
            )
            self.assertEqual(res.returncode, 2, res.stdout)
            data = json.loads(res.stdout)
            self.assertFalse(data["budget"]["passed"])
            self.assertGreaterEqual(len(data["budget"]["violations"]), 2)

    def test_reset_memory_peak_catches_subinterval_spike_and_passes_clean_window(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fix = Path(tmp)
            (fix / "meta.json").write_text(
                json.dumps({"praxi_uid": 999, "memory_peak_reset": True}),
                encoding="utf-8",
            )
            write_meminfo_vmstat(fix / "proc", 6 * 1024 * 1024, 5 * 1024 * 1024)
            write_proc_entry(fix / "proc", 10, "bot", 1, 999, 10, 0, 10, 50_000, 40_000)
            # Clean window with reset memory.peak within budget
            write_cgroup(fix / "cgroup", "/praxi.slice", 350 * MIB, 380 * MIB, 10_000, [10])
            res_ok = self.run_cli("--fixture-dir", str(fix), "--check-budget", "--format", "json")
            self.assertEqual(res_ok.returncode, 0, res_ok.stdout + res_ok.stderr)
            data_ok = json.loads(res_ok.stdout)
            self.assertTrue(data_ok["budget"]["passed"])
            self.assertEqual(data_ok["praxi_tree"]["effective_peak"]["bytes"], 380 * MIB)
            self.assertEqual(
                data_ok["praxi_tree"]["effective_peak_source"],
                "praxi.slice/memory.peak (window reset)",
            )

            # Sub-interval spike above 2.5 GB between 0.5s samples must FAIL
            write_cgroup(fix / "cgroup", "/praxi.slice", 350 * MIB, 2_600_000_000, 10_000, [10])
            res_spike = self.run_cli("--fixture-dir", str(fix), "--check-budget", "--format", "json")
            self.assertEqual(res_spike.returncode, 2, res_spike.stdout)
            data_spike = json.loads(res_spike.stdout)
            self.assertFalse(data_spike["budget"]["passed"])
            self.assertFalse(data_spike["budget"]["inconclusive"])
            self.assertEqual(data_spike["praxi_tree"]["effective_peak"]["bytes"], 2_600_000_000)

    def test_unreset_lifetime_peak_over_budget_fails_inconclusive(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fix = Path(tmp)
            (fix / "meta.json").write_text(
                json.dumps({"praxi_uid": 999, "memory_peak_reset": False}),
                encoding="utf-8",
            )
            write_meminfo_vmstat(fix / "proc", 6 * 1024 * 1024, 5 * 1024 * 1024)
            write_proc_entry(fix / "proc", 10, "bot", 1, 999, 10, 0, 10, 50_000, 40_000)
            # Lifetime memory.peak is 2.6 GB while sampled memory.current is 350 MiB and reset was unavailable
            write_cgroup(fix / "cgroup", "/praxi.slice", 350 * MIB, 2_600_000_000, 10_000, [10])

            res = self.run_cli(
                "--fixture-dir", str(fix),
                "--check-budget",
                "--format", "json",
            )
            self.assertEqual(res.returncode, 2, res.stdout + res.stderr)
            data = json.loads(res.stdout)
            self.assertFalse(data["budget"]["passed"])
            self.assertTrue(data["budget"]["inconclusive"])
            self.assertTrue(any("inconclusive" in v for v in data["budget"]["violations"]))

            res_txt = self.run_cli("--fixture-dir", str(fix), "--check-budget", "--format", "text")
            self.assertEqual(res_txt.returncode, 2)
            self.assertIn("FAIL (INCONCLUSIVE)", res_txt.stdout)

    def test_absent_slice_and_zero_processes_fails_check_budget(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fix = Path(tmp)
            (fix / "meta.json").write_text(json.dumps({"praxi_uid": 999}), encoding="utf-8")
            write_meminfo_vmstat(fix / "proc", 6 * 1024 * 1024, 5 * 1024 * 1024)
            (fix / "cgroup").mkdir(parents=True, exist_ok=True)

            res = self.run_cli("--fixture-dir", str(fix), "--check-budget", "--format", "json")
            self.assertEqual(res.returncode, 2, res.stdout)
            data = json.loads(res.stdout)
            self.assertFalse(data["budget"]["passed"])
            violations = " ".join(data["budget"]["violations"])
            self.assertIn("praxi.slice cgroup was not observed", violations)
            self.assertIn("no Praxi processes observed", violations)

    def test_escaped_praxi_and_other_uid_helpers_fail_and_count_memory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fix = Path(tmp)
            (fix / "meta.json").write_text(
                json.dumps({"praxi_uid": 999, "memory_peak_reset": True}),
                encoding="utf-8",
            )
            write_meminfo_vmstat(fix / "proc", 6 * 1024 * 1024, 3 * 1024 * 1024)
            # Pid 10 inside praxi.slice (slice current 2.45 GB, under 2.5 GB by itself)
            write_proc_entry(fix / "proc", 10, "bot", 1, 999, 10, 0, 10, 50_000, 40_000, cgroup_v2="/praxi.slice/praxi.service")
            # Escaped praxi-uid process (pid 20, 100 MiB) and escaped root-uid helper (pid 30, 50 MiB)
            write_proc_entry(fix / "proc", 20, "chrome", 1, 999, 10, 0, 11, 120_000, 100_000, cgroup_v2="/user.slice/user-1000.slice")
            write_proc_entry(fix / "proc", 30, "bot", 1, 0, 5, 0, 12, 60_000, 50_000, cgroup_v2="/system.slice/praxi-doctor-1.service")
            write_cgroup(fix / "cgroup", "/praxi.slice", 2_450_000_000, 2_450_000_000, 10_000, [10])

            # Note: --require-slice-containment is NOT passed; --check-budget alone must fail!
            res = self.run_cli("--fixture-dir", str(fix), "--check-budget", "--format", "json")
            self.assertEqual(res.returncode, 2, res.stdout)
            data = json.loads(res.stdout)
            self.assertFalse(data["budget"]["passed"])
            escaped_pids = {item["pid"] for item in data["praxi_tree"]["escaped_slice_pids"]}
            self.assertEqual(escaped_pids, {20, 30})
            self.assertEqual(data["praxi_tree"]["escaped_memory_peak"]["bytes"], (100_000 + 50_000) * 1024)
            self.assertGreater(data["praxi_tree"]["effective_peak"]["bytes"], 2_500_000_000)
            self.assertTrue(any("escaped praxi.slice" in v for v in data["budget"]["violations"]))

    def test_race_proc_cgroup_in_slice_not_false_escape(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fix = Path(tmp)
            (fix / "meta.json").write_text(
                json.dumps({"praxi_uid": 999, "memory_peak_reset": True}),
                encoding="utf-8",
            )
            write_meminfo_vmstat(fix / "proc", 6 * 1024 * 1024, 5 * 1024 * 1024)
            write_proc_entry(fix / "proc", 10, "bot", 1, 999, 10, 0, 10, 50_000, 40_000, cgroup_v2="/praxi.slice/praxi.service")
            # Pid 40 appeared in /proc after cgroup.procs read, but /proc/40/cgroup shows /praxi.slice
            write_proc_entry(fix / "proc", 40, "chrome", 10, 999, 5, 0, 11, 20_000, 15_000, cgroup_v2="/praxi.slice/praxi.service")
            write_cgroup(fix / "cgroup", "/praxi.slice", 300 * MIB, 300 * MIB, 10_000, [10])

            res = self.run_cli("--fixture-dir", str(fix), "--check-budget", "--format", "json")
            self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
            data = json.loads(res.stdout)
            self.assertTrue(data["budget"]["passed"])
            self.assertEqual(data["praxi_tree"]["escaped_slice_pids"], [])

    def test_memory_events_high_max_deltas_reported(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fix = Path(tmp)
            (fix / "meta.json").write_text(json.dumps({"praxi_uid": 999}), encoding="utf-8")
            for idx, (h_ev, m_ev) in enumerate(((1, 0), (4, 2))):
                t = fix / "samples" / f"tick_{idx:03d}"
                (t / "tick.json").parent.mkdir(parents=True, exist_ok=True)
                (t / "tick.json").write_text(json.dumps({"elapsed_s": idx * 0.5}), encoding="utf-8")
                write_meminfo_vmstat(t / "proc", 6 * 1024 * 1024, 5 * 1024 * 1024)
                write_proc_entry(t / "proc", 10, "bot", 1, 999, 10 + idx, 0, 10, 50_000, 40_000)
                write_cgroup(t / "cgroup", "/praxi.slice", 300 * MIB, 300 * MIB, 10_000 * (idx + 1), [10], high_events=h_ev, max_events=m_ev)

            res = self.run_cli("--fixture-dir", str(fix), "--check-budget", "--format", "both")
            self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
            self.assertIn("events_delta(high=3,max=2,oom_kill=0)", res.stdout)

    def test_partial_pss_leaves_sum_null_and_falls_back_to_rss(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            fix = Path(tmp)
            (fix / "meta.json").write_text(json.dumps({"praxi_uid": 999, "unit_cgroups": {}}), encoding="utf-8")
            write_meminfo_vmstat(fix / "proc", 6 * 1024 * 1024, 3 * 1024 * 1024)
            # bot has readable PSS, chrome is missing smaps_rollup and exceeds 2.5 GB RSS
            write_proc_entry(fix / "proc", 10, "bot", 1, 999, 10, 0, 10, 50_000, 40_000)
            write_proc_entry(fix / "proc", 20, "chrome", 10, 999, 20, 0, 11, 2_600_000, None)
            (fix / "cgroup").mkdir(parents=True, exist_ok=True)

            res = self.run_cli(
                "--fixture-dir", str(fix),
                "--check-budget",
                "--format", "json",
            )
            self.assertEqual(res.returncode, 2, res.stdout)
            data = json.loads(res.stdout)
            self.assertFalse(data["sampling"]["pss_available"])
            self.assertFalse(data["sampling"]["pss_complete"])
            self.assertEqual(data["sampling"]["pss_incomplete_events"], 1)
            self.assertIsNone(data["praxi_tree"]["sum_process_pss_max"])
            self.assertIn("sum_proc_rss_max", data["praxi_tree"]["effective_peak_source"])
            self.assertFalse(data["budget"]["passed"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
