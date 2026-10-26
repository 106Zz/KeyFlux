#!/usr/bin/env python3
"""Read-only macOS process sampling. Python 3.9+, no third-party dependencies."""

import argparse
from collections import deque
import csv
import ctypes as C
from datetime import datetime
import errno
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import resource
import signal
import subprocess
import sys
import tempfile
import threading
import time


ROOT = Path(__file__).resolve().parents[1]
MB = 1_000_000
RUNTIME_COUNTERS = {
    "monitor_failures": "监听故障：",
    "caps_retry_rounds": "Caps 状态尚未同步",
    "caps_retry_successes": "Caps 自动重试恢复成功",
    "recovery_attempts": "开始自动重建监听。",
    "recovery_failures": "自动重建未完成：",
    "recovery_successes": "已自动重建监听，",
}
FIELDS = ["timestamp", "elapsed_s", "interval_s", "phase", "pid", "cpu_percent",
          "cpu_time_s", "rss_mb", "footprint_mb", "threads", "idle_wakeups",
          "interrupt_wakeups", "pageins", "disk_read_bytes", "disk_written_bytes",
          "listener_state", *RUNTIME_COUNTERS, "segment", "interval_state", "interval_phase", "log_coverage_complete"]


def runtime_sources(explicit=None):
    paths = explicit or [ROOT / "target/keyflux-runtime.log", Path.home() / "Library/Logs/KeyFlux/KeyFlux.log"]
    return list(dict.fromkeys(Path(path).expanduser().resolve() for path in paths))


class RuntimeStatus:
    """Only parse this PID's control logs; counters never infer actual typing success."""

    def __init__(self, pid, not_before=None, counter_since=None):
        self.pattern = re.compile(r"^\[[\d.]+ pid=" + str(pid) + r"\] (.*)$")
        self.state = "unknown"
        self.structured = False
        self.counts = dict.fromkeys(RUNTIME_COUNTERS, 0)
        self.pending = b""
        self.pending_by_source = {}
        self.not_before = not_before
        self.counter_since = counter_since
        self.last_state_at = float("-inf")
        self.last_source = None
        self.last_state_line = None
        self.seen = set()
        self.seen_order = deque()
        self.long_lines = 0
        self.state_revision = 0

    def feed(self, content, count=True, source=None):
        pending = self.pending if source is None else self.pending_by_source.get(source, b"")
        lines = (pending + content).split(b"\n")
        pending = lines.pop()
        if len(pending) > 8192:
            self.long_lines += 1
            pending = pending[-8192:]
        if source is None:
            self.pending = pending
        else:
            self.pending_by_source[source] = pending
        accepted = []
        for raw in lines:
            if len(raw) > 8192:
                self.long_lines += 1
                continue
            match = self.pattern.match(raw.decode("utf-8", errors="replace"))
            if not match:
                continue
            body = match[1]
            # Copy only this PID's control records; never copy verbose key/device diagnostics.
            if not body.startswith(("KeyFlux state=", "KeyFlux：", "KeyFlux:")) \
                    and not any(marker in body for marker in RUNTIME_COUNTERS.values()):
                continue
            try:
                recorded_at = float(match[0][1:].split(" ", 1)[0])
            except ValueError:
                continue
            if not math.isfinite(recorded_at) or (self.not_before is not None and recorded_at < self.not_before - 0.001):
                continue
            # Fixed-size fingerprints keep the de-duplication window bounded even for long errors.
            key = hashlib.blake2b(raw, digest_size=16).digest()
            if key in self.seen:
                continue
            self.seen.add(key)
            self.seen_order.append(key)
            if len(self.seen_order) > 65536:
                self.seen.remove(self.seen_order.popleft())
            accepted.append((recorded_at, match[0]))
            next_state = None
            if body.startswith("KeyFlux state="):
                state = body.removeprefix("KeyFlux state=")
                next_state = state if state in ("listening", "stopped", "recovering", "permission-required") else "unknown"
                self.structured = True
            elif not self.structured:
                if "Shift 切换与中/英键短按大写已接入" in body:
                    next_state = "listening"
                elif "监听已停止" in body:
                    next_state = "stopped"
                elif "后自动重建监听" in body:
                    next_state = "recovering"
            if next_state is not None and recorded_at >= self.last_state_at:
                if next_state != self.state:
                    self.state_revision += 1
                self.state, self.last_state_at = next_state, recorded_at
                self.last_source, self.last_state_line = source, match[0]
            if count and (self.counter_since is None or recorded_at >= self.counter_since):
                for key, marker in RUNTIME_COUNTERS.items():
                    if marker in body:
                        self.counts[key] += 1
        return accepted

    def snapshot(self):
        return {"listener_state": self.state, **self.counts}


def runtime_status(pid, sources=None, not_before=None):
    """Bounded control-log snapshot, also used by the guided memory profiler."""
    status = RuntimeStatus(pid, not_before=not_before)
    follower = LogFollower(status, runtime_sources(sources))
    follower.seed(count=True)
    return status


class LogFollower:
    """Bounded multi-file reader; PID/time filtering and de-duplication happen in RuntimeStatus."""

    def __init__(self, status, sources):
        self.status = status
        self.cursors = {str(path): {"path": path, "identity": None, "offset": 0, "error": None} for path in sources}
        self.complete = True
        self.backlog = False
        self.initial_records = []

    def seed(self, count=False):
        for name, cursor in self.cursors.items():
            try:
                with cursor["path"].open("rb") as stream:
                    info = os.fstat(stream.fileno())
                    offset = max(0, info.st_size - 262144)
                    stream.seek(offset)
                    content = stream.read(262144)
                    if offset:
                        content = content.partition(b"\n")[2]
                    accepted = self.status.feed(content, count=count, source=name)
                    if self.status.counter_since is not None:
                        self.initial_records.extend(item for item in accepted if item[0] >= self.status.counter_since)
                    cursor.update(identity=(info.st_dev, info.st_ino), offset=stream.tell())
            except FileNotFoundError:
                pass
            except OSError as error:
                cursor["error"] = str(error)
                self.complete = False
        if self.status.state == "unknown":
            self.complete = False

    def invalidate(self, name):
        self.status.pending_by_source.pop(name, None)
        if self.status.last_source == name:
            if self.status.state != "unknown":
                self.status.state_revision += 1
            self.status.state = "unknown"
            self.complete = False

    def poll(self, event, output):
        accepted, self.backlog = [], False
        for name, cursor in self.cursors.items():
            try:
                with cursor["path"].open("rb") as stream:
                    info = os.fstat(stream.fileno())
                    identity = (info.st_dev, info.st_ino)
                    if cursor["identity"] is not None and (identity != cursor["identity"] or info.st_size < cursor["offset"]):
                        self.invalidate(name)
                        cursor["offset"] = 0
                        event("runtime_log_reopened", source=name)
                    cursor["identity"] = identity
                    stream.seek(cursor["offset"])
                    content = stream.read(1_048_576)
                    cursor["offset"] = stream.tell()
                    if cursor["offset"] < info.st_size:
                        self.backlog, self.complete = True, False
                        if cursor["error"] != "backlog":
                            event("runtime_log_backlog", source=name, unread_bytes=info.st_size - cursor["offset"])
                        cursor["error"] = "backlog"
                    else:
                        cursor["error"] = None
                accepted.extend(self.status.feed(content, source=name))
            except FileNotFoundError:
                if cursor["identity"] is not None:
                    self.invalidate(name)
                    cursor.update(identity=None, offset=0)
                    event("runtime_log_missing", source=name)
            except OSError as error:
                self.complete = False
                self.invalidate(name)
                if cursor["error"] != str(error):
                    event("runtime_log_read_error", source=name, error=str(error))
                cursor["error"] = str(error)
        if self.status.long_lines:
            self.complete = False
        for recorded_at, line in sorted(accepted, key=lambda item: item[0]):
            if self.status.counter_since is None or recorded_at >= self.status.counter_since:
                output.write(line + "\n")

    def snapshot(self):
        value = self.status.snapshot()
        if self.backlog:
            value["listener_state"] = "unknown"
        value["log_coverage_complete"] = self.complete and not self.backlog and self.status.state != "unknown"
        return value


class Usage(C.Structure):
    # sys/resource.h: RUSAGE_INFO_V2, 160 bytes on macOS arm64 / x86_64.
    _fields_ = [("uuid", C.c_uint8 * 16)] + [(name, C.c_uint64) for name in (
        "user", "system", "idle_wakeups", "interrupt_wakeups", "pageins", "wired",
        "resident", "footprint", "start", "exit", "child_user", "child_system",
        "child_idle", "child_interrupt", "child_pageins", "child_elapsed",
        "disk_read", "disk_written")]


class Task(C.Structure):
    # sys/proc_info.h: PROC_PIDTASKINFO, 96 bytes; only thread count is used.
    _fields_ = [(name, C.c_uint64) for name in (
        "virtual", "resident", "user", "system", "threads_user", "threads_system")]
    _fields_ += [(name, C.c_int32) for name in (
        "policy", "faults", "pageins", "cow", "sent", "received", "mach_calls",
        "unix_calls", "switches", "threads", "running", "priority")]


class BsdInfo(C.Structure):
    # sys/proc_info.h: PROC_PIDTBSDINFO; wall-clock start excludes stale PID reuse in logs.
    _fields_ = [(name, C.c_uint32) for name in (
        "flags", "status", "xstatus", "pid", "ppid", "uid", "gid", "ruid", "rgid", "svuid", "svgid", "reserved")]
    _fields_ += [("comm", C.c_char * 16), ("name", C.c_char * 32)]
    _fields_ += [(name, C.c_uint32) for name in ("nfiles", "pgid", "jobc", "tdev", "tpgid")]
    _fields_ += [("nice", C.c_int32), ("start_sec", C.c_uint64), ("start_usec", C.c_uint64)]


class Timebase(C.Structure):
    _fields_ = [("numer", C.c_uint32), ("denom", C.c_uint32)]


class Native:
    def __init__(self):
        self.proc = C.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        self.system = C.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
        self.proc.proc_pid_rusage.argtypes = [C.c_int, C.c_int, C.c_void_p]
        self.proc.proc_pid_rusage.restype = C.c_int
        self.proc.proc_pidinfo.argtypes = [C.c_int, C.c_int, C.c_uint64, C.c_void_p, C.c_int]
        self.proc.proc_pidinfo.restype = C.c_int
        self.proc.proc_pidpath.argtypes = [C.c_int, C.c_void_p, C.c_uint32]
        self.proc.proc_pidpath.restype = C.c_int
        self.system.mach_timebase_info.argtypes = [C.POINTER(Timebase)]
        self.system.mach_timebase_info.restype = C.c_int
        self.system.mach_continuous_time.argtypes = []
        self.system.mach_continuous_time.restype = C.c_uint64
        self.system.mach_absolute_time.argtypes = []
        self.system.mach_absolute_time.restype = C.c_uint64
        base = Timebase()
        if self.system.mach_timebase_info(C.byref(base)) != 0 or not base.denom:
            raise RuntimeError("无法读取 Mach 时钟比例")
        self.seconds_per_tick = base.numer / base.denom / 1_000_000_000
        if C.sizeof(Usage) != 160 or C.sizeof(Task) != 96 or C.sizeof(BsdInfo) != 136:
            raise RuntimeError("本机进程信息结构大小不匹配")

    def now(self):
        # Includes system sleep, unlike mach_absolute_time on macOS.
        return self.system.mach_continuous_time() * self.seconds_per_tick

    def awake(self):
        return self.system.mach_absolute_time() * self.seconds_per_tick

    def path(self, pid):
        buf = C.create_string_buffer(4096)
        if self.proc.proc_pidpath(pid, buf, len(buf)) <= 0:
            self.fail("proc_pidpath")
        return Path(os.fsdecode(buf.value)).resolve()

    def read(self, pid):
        usage, task = Usage(), Task()
        if self.proc.proc_pid_rusage(pid, 2, C.byref(usage)) != 0:
            self.fail("proc_pid_rusage")
        if usage.exit:
            raise ProcessLookupError(errno.ESRCH, "目标进程已退出")
        if self.proc.proc_pidinfo(pid, 4, 0, C.byref(task), C.sizeof(task)) != C.sizeof(task):
            self.fail("proc_pidinfo")
        # Modern XNU fill_task_rusage -> task_power_info_locked returns Mach ticks,
        # not nanoseconds on Apple Silicon. Always apply mach_timebase_info.
        # https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/task.c
        return usage, task, (usage.user + usage.system) * self.seconds_per_tick

    def started_at(self, pid):
        info = BsdInfo()
        if self.proc.proc_pidinfo(pid, 3, 0, C.byref(info), C.sizeof(info)) != C.sizeof(info):
            self.fail("proc_pidinfo(start)")
        if info.pid != pid:
            raise ProcessLookupError(errno.ESRCH, "PID 身份已变化")
        return info.start_sec + info.start_usec / 1_000_000

    @staticmethod
    def fail(operation):
        code = C.get_errno() or errno.EIO
        raise OSError(code, f"{operation}: {os.strerror(code)}")


def stamp():
    return datetime.now().astimezone().isoformat(timespec="seconds")


def discover(native, expected, allow_missing=False):
    result = subprocess.run(["/usr/bin/pgrep", "-x", "KeyFlux"], capture_output=True, text=True)
    if result.returncode not in (0, 1):
        raise RuntimeError(f"无法读取进程列表：{result.stderr.strip()}")
    matches = []
    for candidate in result.stdout.split():
        try:
            if native.path(int(candidate)) == expected:
                matches.append(int(candidate))
        except OSError as error:
            if error.errno in (errno.ESRCH, errno.ENOENT):
                continue
            raise
    if not matches:
        if allow_missing:
            return None
        raise RuntimeError(f"没有找到运行中的 {expected}。请先打开 KeyFlux，再启动采样。")
    if len(matches) != 1:
        raise RuntimeError(f"发现多个匹配进程 {matches}，请用 --pid 指定一个。")
    return matches[0]


def segment_key(row):
    return row.get("segment") or row["pid"]


def statistics_for(rows, limit, steady=False, listening=False):
    group = {"samples": len(rows), "segments": len({segment_key(row) for row in rows})}
    for metric in ("rss_mb", "footprint_mb"):
        values = [float(row[metric]) for row in rows]
        if values:
            group[metric] = {
                "first": values[0], "last": values[-1],
                "growth": values[-1] - values[0] if group["segments"] == 1 else None,
                "mean": sum(values) / len(values), "peak": max(values),
                "p95": sorted(values)[max(0, math.ceil(len(values) * 0.95) - 1)],
                "at_or_above_limit_samples": sum(value >= limit for value in values)}
    cpu = [(float(row["cpu_percent"]), float(row["interval_s"])) for row in rows
           if row["cpu_percent"] and float(row["interval_s"]) > 0
           and (not steady or row.get("interval_phase", row["phase"]) == "steady")
           and (not listening or row.get("interval_state", row.get("listener_state")) == "listening")]
    if cpu:
        seconds = sum(duration for _, duration in cpu)
        group["cpu_percent"] = {"weighted_mean": sum(value * duration for value, duration in cpu) / seconds,
                                "peak_interval_mean": max(value for value, _ in cpu),
                                "valid_interval_seconds": seconds,
                                "at_or_above_1_percent_intervals": sum(value >= 1 for value, _ in cpu)}
    if rows:
        group["peak_threads"] = max(int(row["threads"]) for row in rows)
        if "listener_state" in rows[0]:
            states = [row["listener_state"] for row in rows]
            group["listener_state_samples"] = {state: states.count(state) for state in sorted(set(states))}
    return group


def summarize(folder):
    metadata = json.loads((folder / "metadata.json").read_text())
    rows, incomplete = [], 0
    with (folder / "samples.csv").open(newline="") as source:
        for row in csv.DictReader(source):
            try:
                if None in row or any(value is None for value in row.values()):
                    raise ValueError("unfinished CSV row")
                values = [float(row[key]) for key in ("elapsed_s", "interval_s", "rss_mb", "footprint_mb")]
                if row["cpu_percent"]:
                    values.append(float(row["cpu_percent"]))
                if not all(math.isfinite(value) for value in values) or not row.get("pid") or not row.get("phase"):
                    raise ValueError("incomplete row")
                int(row["threads"])
                int(row["pid"])
                if row.get("segment"):
                    int(row["segment"])
                rows.append(row)
            except (ValueError, TypeError, KeyError):
                incomplete += 1
    limit = metadata["memory_limit_mb"]
    steady = [row for row in rows if row["phase"] == "steady"]
    result = {"全部样本": statistics_for(rows, limit), "预热后样本": statistics_for(steady, limit, steady=True),
              "incomplete_rows": incomplete}
    if rows and "listener_state" in rows[0]:
        result["预热后监听样本"] = statistics_for(
            [row for row in steady if row["listener_state"] == "listening"], limit, steady=True, listening=True)
    result["observed_elapsed_seconds"] = float(rows[-1]["elapsed_s"]) if rows else 0
    runtime = folder / "runtime-status.json"
    if runtime.exists():
        result["runtime"] = json.loads(runtime.read_text())
    grouped = {}
    for row in rows:
        grouped.setdefault(segment_key(row), []).append(row)
    result["segments"] = [{"segment": key, "pid": selected[0]["pid"], "first_at": selected[0]["timestamp"],
                           "last_at": selected[-1]["timestamp"], **statistics_for(selected, limit)}
                          for key, selected in grouped.items()]
    # A live --report must not present a five-minute-old checkpoint as the latest sample.
    saved_runtime = result.get("runtime")
    if rows and "listener_state" in rows[-1] and (not saved_runtime or (
            saved_runtime.get("recorded_at") is not None and saved_runtime["recorded_at"] < rows[-1]["timestamp"])):
        runtime_value = result.setdefault("runtime", {})
        runtime_value.update(listener_state=rows[-1]["listener_state"],
                             log_coverage_complete=all(row.get("log_coverage_complete", "True") == "True" for row in rows),
                             process_available=True, waiting_for_process=False, recorded_at=rows[-1]["timestamp"],
                             pid=int(rows[-1]["pid"]), segment=int(rows[-1].get("segment") or 1))
        for key in RUNTIME_COUNTERS:
            runtime_value[key] = sum(max(int(row.get(key) or 0) for row in selected) for selected in grouped.values())
    result["event_counts"], result["sampling_gap_seconds"], result["process_wait_seconds"] = {}, 0.0, 0.0
    events = folder / "events.jsonl"
    if events.exists():
        for line in events.read_text().splitlines():
            try:
                entry = json.loads(line)
                kind = entry["event"]
                result["event_counts"][kind] = result["event_counts"].get(kind, 0) + 1
                if kind == "sampling_gap_or_counter_reset":
                    result["sampling_gap_seconds"] += entry.get("interval_seconds", 0)
                if kind == "process_wait_ended":
                    result["process_wait_seconds"] += entry.get("seconds", 0)
                if kind == "finished":
                    result["finish_reason"] = entry["reason"]
                    result["capture_elapsed_seconds"] = entry.get("elapsed_seconds", result["observed_elapsed_seconds"])
            except (ValueError, KeyError, TypeError):
                continue  # A report may race with the append of the final event line.
    hourly = {}
    for row in rows:
        hourly.setdefault(int(float(row["elapsed_s"]) // 3600), []).append(row)
    result["hourly"] = [{"hour": hour + 1, "first_at": selected[0]["timestamp"], "last_at": selected[-1]["timestamp"],
                        **statistics_for(selected, limit)} for hour, selected in hourly.items()]
    result["note_count"] = 0
    notes = folder / "notes.jsonl"
    if notes.exists():
        result["note_count"] = sum(bool(line.strip()) for line in notes.read_text().splitlines())
    return result


def report_text(result):
    elapsed = result.get("capture_elapsed_seconds", result["observed_elapsed_seconds"])
    lines = ["KeyFlux 性能采样报告", "", f"采样跨度：{elapsed / 3600:.2f} 小时",
             f"记录状态：{result.get('finish_reason', '采集中 / 按当前样本汇总')}"]
    for label in ("全部样本", "预热后样本", "预热后监听样本"):
        if label not in result:
            continue
        group = result[label]
        lines.extend(["", f"{label}：{group['samples']} 条"])
        for metric, title in (("rss_mb", "RSS"), ("footprint_mb", "物理 Footprint")):
            if metric in group:
                value = group[metric]
                growth = f"首末变化 {value['growth']:+.3f} MB" if value['growth'] is not None else "跨进程，不合并计算内存增长"
                lines.append(f"{title}：平均 {value['mean']:.3f} MB，P95 {value['p95']:.3f} MB，采样峰值 {value['peak']:.3f} MB，"
                             f"{growth}，达到/超过内存目标 {value['at_or_above_limit_samples']} 条")
        if "cpu_percent" in group:
            cpu = group["cpu_percent"]
            lines.append(f"CPU：有效区间加权平均 {cpu['weighted_mean']:.4f}%，最高区间平均 {cpu['peak_interval_mean']:.4f}%")
        if "peak_threads" in group:
            lines.append(f"线程峰值：{group['peak_threads']}")
        if "listener_state_samples" in group:
            lines.append(f"日志对应监听状态的样本数：{group['listener_state_samples']}")
    if "runtime" in result:
        runtime = result["runtime"]
        lines.extend(["", f"当前 / 结束时日志状态：{runtime.get('listener_state', 'unknown')}",
            f"采样期间功能故障：{runtime.get('monitor_failures', 0)} 次；Caps 额外确认轮次：{runtime.get('caps_retry_rounds', 0)}；轮内恢复成功：{runtime.get('caps_retry_successes', 0)} 次。",
            f"自动重建尝试：{runtime.get('recovery_attempts', 0)} 次，成功：{runtime.get('recovery_successes', 0)} 次，失败：{runtime.get('recovery_failures', 0)} 次。"])
        if not runtime.get("log_coverage_complete", True):
            lines.append("状态日志覆盖不完整：故障数可能少计，不能用 0 次验收无故障。")
    lines.extend(["", f"进程采样分段：{len(result.get('segments', []))}；采样长空档合计：{result.get('sampling_gap_seconds', 0):.1f} 秒；"
                  f"等待进程：{result.get('process_wait_seconds', 0):.1f} 秒；人工备注：{result.get('note_count', 0)} 条。"])
    if result.get("event_counts", {}).get("process_ended"):
        lines.append("本轮出现过进程退出 / 身份变化；重新附加不代表期间没有故障。")
    if result.get("incomplete_rows"):
        lines.append(f"忽略 {result['incomplete_rows']} 条未写完整或无效的 CSV 行；原文件保留。")
    lines.extend(["", "MB = 1,000,000 字节；CPU 100% 表示占满一个逻辑核。",
                  "峰值是采样观测值，不能排除采样间隔中的瞬时内存峰值；CPU 是区间平均值。",
                  "采样间隔过长（例如合盖睡眠）不计入 CPU 均值，详见 events.jsonl。",
                  "监听状态和故障次数来自当前 PID 的基础运行日志；日志缺失时状态为 unknown，不能据 CPU 低判断监听正常。",
                  "自动恢复成功仍计作发生过故障；实际输入效果需要人工确认，这些采样也不能证明不存在内存泄漏。"])
    return "\n".join(lines) + "\n"


def say(message):
    try:
        print(message, flush=True)
    except (BrokenPipeError, OSError):
        pass  # Losing the console must not discard CSV/events or the final report.


def atomic_text(path, content):
    # Only generated files inside this run's new directory are replaced; raw samples are append-only.
    temporary = path.with_name("." + path.name + ".tmp")
    with temporary.open("w", encoding="utf-8") as output:
        output.write(content)
    os.replace(temporary, path)


def checkpoint(folder, runtime, segments):
    atomic_text(folder / "runtime-status.json", json.dumps(runtime, ensure_ascii=False, indent=2))
    atomic_text(folder / "segments.json", json.dumps(segments, ensure_ascii=False, indent=2))
    result = summarize(folder)
    atomic_text(folder / "summary.json", json.dumps(result, ensure_ascii=False, indent=2))
    atomic_text(folder / "summary.txt", report_text(result))
    columns = ["hour", "first_at", "last_at", "samples", "segments", "cpu_percent", "cpu_valid_seconds",
               "rss_mean_mb", "rss_p95_mb", "rss_peak_mb", "footprint_mean_mb", "footprint_p95_mb", "footprint_peak_mb"]
    temporary = folder / ".hourly.csv.tmp"
    with temporary.open("w", newline="", encoding="utf-8") as output:
        writer = csv.DictWriter(output, fieldnames=columns)
        writer.writeheader()
        for hour in result["hourly"]:
            row = {key: hour[key] for key in columns[:5]}
            row.update(cpu_percent=hour.get("cpu_percent", {}).get("weighted_mean", ""),
                       cpu_valid_seconds=hour.get("cpu_percent", {}).get("valid_interval_seconds", 0))
            for prefix, metric in (("rss", "rss_mb"), ("footprint", "footprint_mb")):
                for suffix, field in (("mean", "mean"), ("p95", "p95"), ("peak", "peak")):
                    row[f"{prefix}_{suffix}_mb"] = hour.get(metric, {}).get(field, "")
            writer.writerow(row)
    os.replace(temporary, folder / "hourly.csv")
    return result


def attach(native, pid, executable, sources, index):
    if native.path(pid) != executable:
        raise RuntimeError("指定 PID 不是当前项目的 target/release/KeyFlux，拒绝混入其他进程数据。")
    usage, _, _ = native.read(pid)
    launch_time = native.started_at(pid)
    checked, _, _ = native.read(pid)
    if checked.start != usage.start:
        raise ProcessLookupError(errno.ESRCH, "附加过程中 PID 身份发生变化")
    state = RuntimeStatus(pid, not_before=launch_time, counter_since=math.floor(time.time() * 1000) / 1000)
    logs = LogFollower(state, sources)
    logs.seed(count=True)
    descriptor = {"segment": index, "pid": pid, "process_start_ticks": usage.start,
                  "process_launched_at": datetime.fromtimestamp(launch_time).astimezone().isoformat(timespec="seconds"),
                  "attached_at": stamp(), "disk_binary_sha256_at_attach": hashlib.sha256(executable.read_bytes()).hexdigest(),
                  "initial_listener_state": state.state, "initial_state_record": state.last_state_line,
                  "initial_runtime": logs.snapshot()}
    return {"descriptor": descriptor, "logs": logs, "attached_clock": native.now(), "previous": None,
            "above": {"rss": None, "footprint": None}}


def monitor(args):
    native = Native()
    executable = (ROOT / "target/release/KeyFlux").resolve()
    sources = runtime_sources(args.runtime_log)
    first_pid = args.pid if args.pid else discover(native, executable)
    current = attach(native, first_pid, executable, sources, 1)
    parent = ROOT / "target/performance"
    parent.mkdir(parents=True, exist_ok=True)
    folder = Path(tempfile.mkdtemp(prefix=datetime.now().strftime("%Y%m%d-%H%M%S-") + f"pid{first_pid}-", dir=parent))
    metadata = {"schema": 3, "started_at": stamp(), "pid": first_pid, "executable": str(executable),
                "sampler_pid": os.getpid(), "python_version": platform.python_version(),
                "sampler_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                "disk_binary_sha256_at_attach": current["descriptor"]["disk_binary_sha256_at_attach"],
                "process_start_ticks": current["descriptor"]["process_start_ticks"],
                "macos": platform.mac_ver()[0], "arch": platform.machine(),
                "interval_seconds": args.interval, "duration_hours": args.hours, "warmup_seconds": args.warmup,
                "memory_limit_mb": args.memory_limit_mb, "bytes_per_mb": MB,
                "mach_seconds_per_tick": native.seconds_per_tick, "runtime_log_sources": [str(path) for path in sources],
                "follow_restarts": args.follow_restarts, "checkpoint_seconds": 300, "tag": args.tag,
                "notes": "只读附加，不启动、停止或重启 KeyFlux；每个进程单独分段，预热从各次附加开始。"}
    with (folder / "metadata.json").open("x", encoding="utf-8") as output:
        json.dump(metadata, output, ensure_ascii=False, indent=2)
    stop, reason = threading.Event(), "duration_complete"
    old_signals = {}

    def stopped(signum, _frame):
        nonlocal reason
        reason = f"signal_{signal.Signals(signum).name}"
        stop.set()

    for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        old_signals[signum] = signal.signal(signum, stopped)
    started, next_checkpoint, next_status = native.now(), 0.0, args.status_every
    segments, totals = [current["descriptor"]], dict.fromkeys(RUNTIME_COUNTERS, 0)
    all_logs_complete, waiting_since, last_snapshot = True, None, current["logs"].snapshot()
    process_available = True
    latest, terminal_report = None, None
    say(f"采样器 PID={os.getpid()}；目标 KeyFlux PID={first_pid}。每 {args.interval:g} 秒一次，最长 {args.hours:g} 小时。")
    say(f"日志目录：{folder}\n自动识别终端 / 自启日志；每 5 分钟保存摘要。Ctrl+C 只停止采样，不退出 KeyFlux。")
    if args.follow_restarts:
        say("已开启跟随模式：KeyFlux 退出后等待同一路径的新进程；不会替你启动程序，重开后单独分段。")
    if last_snapshot["listener_state"] == "unknown":
        say("注意：尚无当前进程的状态日志。资源仍会采集，功能状态标为 unknown，不能据此验收无故障。")

    with (folder / "samples.csv").open("x", newline="", buffering=1, encoding="utf-8") as samples, \
            (folder / "events.jsonl").open("x", buffering=1, encoding="utf-8") as events, \
            (folder / "keyflux-runtime.log").open("x", buffering=1, encoding="utf-8") as runtime:
        writer = csv.DictWriter(samples, fieldnames=FIELDS)
        writer.writeheader()
        for _, line in sorted(current["logs"].initial_records, key=lambda item: item[0]):
            runtime.write(line + "\n")

        def event(kind, **details):
            events.write(json.dumps({"timestamp": stamp(), "elapsed_s": round(native.now() - started, 3),
                                    "event": kind, **details}, ensure_ascii=False) + "\n")

        def poll_logs():
            nonlocal last_snapshot
            before = current["logs"].snapshot()
            current["logs"].poll(event, runtime)
            last_snapshot = current["logs"].snapshot()
            if before != last_snapshot:
                event("listener_status", pid=current["descriptor"]["pid"], **last_snapshot)
                say(f"[{stamp()}] PID={current['descriptor']['pid']} 状态={last_snapshot['listener_state']}；"
                    f"本段故障={last_snapshot['monitor_failures']}，自动重建={last_snapshot['recovery_attempts']}。")

        def close_segment(cause, process_ended=False):
            nonlocal current, all_logs_complete, last_snapshot
            if not process_ended and cause != "sampling_error":
                poll_logs()
            else:
                # Once identity is lost, the same PID may already belong to another process.
                # Keep the last verified snapshot rather than attributing a reused PID's new tail.
                last_snapshot = current["logs"].snapshot()
            descriptor = current["descriptor"]
            descriptor.update(ended_at=stamp(), end_reason=cause, final_runtime=last_snapshot.copy())
            for key in RUNTIME_COUNTERS:
                totals[key] += last_snapshot[key]
            all_logs_complete = all_logs_complete and last_snapshot["log_coverage_complete"]
            event("process_ended" if process_ended else "segment_closed", segment=descriptor["segment"],
                  pid=descriptor["pid"], reason=cause)
            current = None

        def runtime_snapshot():
            active = current["logs"].snapshot() if current else last_snapshot.copy()
            counts = {key: totals[key] + (active[key] if current else 0) for key in RUNTIME_COUNTERS}
            return {**active, **counts, "recorded_at": stamp(), "capture_elapsed_seconds": native.now() - started,
                    "listener_state": active["listener_state"] if process_available else "unknown",
                    "log_coverage_complete": all_logs_complete and active["log_coverage_complete"] and waiting_since is None,
                    "initial_listener_state": segments[0]["initial_listener_state"],
                    "counter_scope": "各进程附加后的控制日志之和；已按 PID / 启动时间过滤并去重，不包括附加前故障。",
                    "waiting_for_process": waiting_since is not None, "process_available": process_available,
                    "pid": segments[-1]["pid"] if process_available else None,
                    "segment": segments[-1]["segment"] if process_available else None}

        event("attached", **current["descriptor"])
        try:
            while not stop.is_set():
                now = native.now()
                elapsed = now - started
                if elapsed >= args.hours * 3600:
                    break
                if current is None:
                    candidate = discover(native, executable, allow_missing=True)
                    if candidate is not None:
                        try:
                            current = attach(native, candidate, executable, sources, len(segments) + 1)
                        except ProcessLookupError:
                            stop.wait(min(args.interval, max(0, args.hours * 3600 - (native.now() - started))))
                            continue
                        if waiting_since is not None:
                            event("process_wait_ended", seconds=now - waiting_since)
                        waiting_since = None
                        process_available = True
                        next_checkpoint = 0
                        segments.append(current["descriptor"])
                        for _, line in sorted(current["logs"].initial_records, key=lambda item: item[0]):
                            runtime.write(line + "\n")
                        event("attached", **current["descriptor"])
                        say(f"[{stamp()}] 已附加新进程 PID={candidate}，开始第 {len(segments)} 段；未合并内存增长。")
                if current is not None:
                    descriptor = current["descriptor"]
                    pid = descriptor["pid"]
                    lost = None
                    try:
                        usage, task, cpu_total = native.read(pid)
                        if usage.start != descriptor["process_start_ticks"] or native.path(pid) != executable:
                            lost = "process_identity_changed"
                    except OSError as error:
                        if error.errno not in (errno.ESRCH, errno.ENOENT):
                            raise
                        lost = "process_unavailable"
                    if lost:
                        close_segment(lost, process_ended=True)
                        process_available = False
                        all_logs_complete = False
                        if not args.follow_restarts:
                            reason = lost
                            break
                        waiting_since = now
                        latest = None
                        next_checkpoint = 0
                        event("waiting_for_process")
                        say(f"[{stamp()}] KeyFlux 已退出或身份变化；继续等待重新运行，期间记为中断，不会自动启动它。")
                    else:
                        previous = current["previous"]
                        awake = native.awake()
                        interval, cpu = 0.0, None
                        if previous:
                            interval, delta = now - previous[0], cpu_total - previous[1]
                            sleep_seconds = max(0.0, interval - (awake - previous[5]))
                            if 0 < interval <= max(args.interval * 3, args.interval + 5) and delta >= 0 and sleep_seconds < 1:
                                cpu = delta / interval * 100
                            else:
                                event("sampling_gap_or_counter_reset", pid=pid, interval_seconds=interval,
                                      estimated_sleep_seconds=sleep_seconds)
                        phase = "warmup" if now - current["attached_clock"] < args.warmup else "steady"
                        poll_logs()
                        state = current["logs"].snapshot()
                        revision = current["logs"].status.state_revision
                        row = {"timestamp": stamp(), "elapsed_s": f"{elapsed:.3f}", "interval_s": f"{interval:.3f}",
                               "phase": phase, "pid": pid, "cpu_percent": "" if cpu is None else f"{cpu:.6f}",
                               "cpu_time_s": f"{cpu_total:.6f}", "rss_mb": f"{usage.resident / MB:.6f}",
                               "footprint_mb": f"{usage.footprint / MB:.6f}", "threads": task.threads,
                               "idle_wakeups": usage.idle_wakeups, "interrupt_wakeups": usage.interrupt_wakeups,
                               "pageins": usage.pageins, "disk_read_bytes": usage.disk_read, "disk_written_bytes": usage.disk_written,
                               "segment": descriptor["segment"], "interval_state": "transition", "interval_phase": "transition", **state}
                        if previous:
                            if previous[2] == state["listener_state"] and previous[4] == revision:
                                row["interval_state"] = state["listener_state"]
                            if previous[3] == phase:
                                row["interval_phase"] = phase
                        writer.writerow(row)
                        latest = row
                        for metric, value in (("rss", usage.resident / MB), ("footprint", usage.footprint / MB)):
                            above = value >= args.memory_limit_mb
                            if above != current["above"][metric]:
                                event("memory_limit_crossing", pid=pid, metric=metric, mb=value, at_or_above=above, phase=phase)
                                current["above"][metric] = above
                        current["previous"] = (now, cpu_total, state["listener_state"], phase, revision, awake)
                if elapsed >= next_checkpoint:
                    samples.flush()
                    terminal_report = checkpoint(folder, runtime_snapshot(), segments)
                    next_checkpoint = elapsed + 300
                if not args.quiet and elapsed >= next_status:
                    if latest:
                        say(f"[{stamp()}] PID={latest['pid']} CPU={latest['cpu_percent'] or '--'}% "
                            f"RSS={latest['rss_mb']}MB Footprint={latest['footprint_mb']}MB 状态={latest['listener_state']}")
                    else:
                        say(f"[{stamp()}] 等待 KeyFlux 重新运行，已保留此前数据。")
                    next_status = elapsed + args.status_every
                stop.wait(min(args.interval, max(0, args.hours * 3600 - (native.now() - started))))
        except (OSError, RuntimeError, ValueError) as error:
            if not stop.is_set():
                reason = "sampling_error"
                process_available = None
                all_logs_complete = False
                event(reason, error=str(error))
                say(f"采样遇到错误，将保留已有数据：{error}")
        finally:
            if current is not None:
                close_segment(reason)
            if waiting_since is not None:
                event("process_wait_ended", seconds=native.now() - waiting_since, resumed=False)
            own = resource.getrusage(resource.RUSAGE_SELF)
            event("finished", reason=reason, elapsed_seconds=native.now() - started,
                  sampler_cpu_seconds=own.ru_utime + own.ru_stime, sampler_peak_rss_mb=own.ru_maxrss / MB)
            samples.flush()
            terminal_report = checkpoint(folder, runtime_snapshot(), segments)
            for signum, handler in old_signals.items():
                signal.signal(signum, handler)
    say(report_text(terminal_report) + f"结束原因：{reason}\n日志目录：{folder}")
    return 1 if reason in ("sampling_error", "process_identity_changed", "process_unavailable") else 0


def main():
    parser = argparse.ArgumentParser(description="KeyFlux 只读性能采样；默认每 30 秒记录，24 小时后结束。")
    parser.add_argument("--pid", type=int, help="手动指定当前项目 KeyFlux 的 PID")
    parser.add_argument("--interval", type=float, default=30, help="采样间隔秒，最少 1 秒，默认 30")
    parser.add_argument("--hours", type=float, default=24, help="最长采样小时数，默认 24")
    parser.add_argument("--warmup", type=float, default=120, help="从附加开始的预热秒数，默认 120；仍保存所有样本")
    parser.add_argument("--memory-limit-mb", type=float, default=15, help="严格低于此值，MB=1000000 字节，默认 15")
    parser.add_argument("--report", type=Path, help="读取已有目录显示报告；搭配 --note 时仅追加人工备注")
    parser.add_argument("--tag", default="", help="记录测试条件标签，例如 optimized-external-normal；不改变目标进程")
    parser.add_argument("--runtime-log", type=Path, action="append", help="指定控制日志，可重复；默认自动读取终端与自启两种路径")
    parser.add_argument("--follow-restarts", action="store_true", help="进程退出后等待同一路径的新进程并分段，不自动启动 KeyFlux")
    parser.add_argument("--status-every", type=float, default=300, help="终端状态显示间隔秒，默认 300；不影响采样间隔")
    parser.add_argument("--quiet", action="store_true", help="隐藏定期终端状态，仍保留开始 / 故障 / 结束提示与全部日志")
    parser.add_argument("--note", help="与 --report 目录搭配，追加一条带时间的人工异常备注（最多 1000 字）")
    args = parser.parse_args()
    if args.note is not None and not args.report:
        parser.error("--note 必须与 --report 实际采样目录一起使用")
    if args.report:
        try:
            if args.note is not None:
                if not args.note.strip() or len(args.note) > 1000:
                    parser.error("备注不能为空，且不能超过 1000 字；不要填写输入文字或敏感内容")
                folder = args.report.resolve()
                if not (folder / "metadata.json").is_file() or not (folder / "samples.csv").is_file():
                    parser.error("不是有效的采样目录")
                with (folder / "notes.jsonl").open("a", encoding="utf-8") as output:
                    output.write(json.dumps({"timestamp": stamp(), "note": args.note.strip()}, ensure_ascii=False) + "\n")
                print("已追加备注，不影响当前采样或 KeyFlux。")
                return 0
            print(report_text(summarize(args.report.resolve())))
            return 0
        except (OSError, ValueError, KeyError) as error:
            parser.error(f"无法读取采样目录：{error}")
    if sys.platform != "darwin":
        parser.error("采样只能在 macOS 运行")
    if not all(math.isfinite(value) for value in (args.interval, args.hours, args.warmup, args.memory_limit_mb, args.status_every)) \
            or args.interval < 1 or args.hours <= 0 or args.warmup < 0 or args.memory_limit_mb <= 0 \
            or args.status_every < 1 \
            or (args.pid is not None and args.pid <= 0):
        parser.error("采样 / 状态显示间隔必须 >=1 秒；时长和内存目标必须 >0；预热 >=0；PID 必须为正整数")
    try:
        return monitor(args)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"采样未完成：{error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
