#!/usr/bin/env python3
"""Read-only macOS process sampling. Python 3.9+, no third-party dependencies."""

import argparse
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
import resource
import signal
import subprocess
import sys
import tempfile
import threading


ROOT = Path(__file__).resolve().parents[1]
MB = 1_000_000
FIELDS = ["timestamp", "elapsed_s", "interval_s", "phase", "pid", "cpu_percent",
          "cpu_time_s", "rss_mb", "footprint_mb", "threads", "idle_wakeups",
          "interrupt_wakeups", "pageins", "disk_read_bytes", "disk_written_bytes"]


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
        base = Timebase()
        if self.system.mach_timebase_info(C.byref(base)) != 0 or not base.denom:
            raise RuntimeError("无法读取 Mach 时钟比例")
        self.seconds_per_tick = base.numer / base.denom / 1_000_000_000
        if C.sizeof(Usage) != 160 or C.sizeof(Task) != 96:
            raise RuntimeError("本机进程信息结构大小不匹配")

    def now(self):
        # Includes system sleep, unlike mach_absolute_time on macOS.
        return self.system.mach_continuous_time() * self.seconds_per_tick

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

    @staticmethod
    def fail(operation):
        code = C.get_errno() or errno.EIO
        raise OSError(code, f"{operation}: {os.strerror(code)}")


def stamp():
    return datetime.now().astimezone().isoformat(timespec="seconds")


def discover(native, expected):
    result = subprocess.run(["/usr/bin/pgrep", "-x", "KeyFlux"], capture_output=True, text=True)
    if result.returncode not in (0, 1):
        raise RuntimeError(f"无法读取进程列表：{result.stderr.strip()}")
    matches = []
    for candidate in result.stdout.split():
        try:
            if native.path(int(candidate)) == expected:
                matches.append(int(candidate))
        except ProcessLookupError:
            continue
    if not matches:
        raise RuntimeError(f"没有找到运行中的 {expected}。请先打开 KeyFlux，再启动采样。")
    if len(matches) != 1:
        raise RuntimeError(f"发现多个匹配进程 {matches}，请用 --pid 指定一个。")
    return matches[0]


def summarize(folder):
    metadata = json.loads((folder / "metadata.json").read_text())
    with (folder / "samples.csv").open(newline="") as source:
        rows = list(csv.DictReader(source))
    result = {}
    for label, selected in (("全部样本", rows), ("预热后样本", [r for r in rows if r["phase"] == "steady"])):
        group = {"samples": len(selected)}
        for metric in ("rss_mb", "footprint_mb"):
            values = [float(row[metric]) for row in selected]
            if values:
                group[metric] = {"first": values[0], "last": values[-1],
                                 "growth": values[-1] - values[0], "mean": sum(values) / len(values),
                                 "peak": max(values), "at_or_above_limit_samples": sum(
                                     value >= metadata["memory_limit_mb"] for value in values)}
        cpu = [(float(row["cpu_percent"]), float(row["interval_s"])) for row in selected if row["cpu_percent"]]
        if cpu:
            seconds = sum(duration for _, duration in cpu)
            group["cpu_percent"] = {"weighted_mean": sum(value * duration for value, duration in cpu) / seconds,
                                    "peak_interval_mean": max(value for value, _ in cpu),
                                    "valid_interval_seconds": seconds,
                                    "at_or_above_1_percent_intervals": sum(value >= 1 for value, _ in cpu)}
        if selected:
            group["peak_threads"] = max(int(row["threads"]) for row in selected)
        result[label] = group
    result["observed_elapsed_seconds"] = float(rows[-1]["elapsed_s"]) if rows else 0
    return result


def report_text(result):
    lines = ["KeyFlux 性能采样报告", "", f"采样跨度：{result['observed_elapsed_seconds'] / 3600:.2f} 小时"]
    for label in ("全部样本", "预热后样本"):
        group = result[label]
        lines.extend(["", f"{label}：{group['samples']} 条"])
        for metric, title in (("rss_mb", "RSS"), ("footprint_mb", "物理 Footprint")):
            if metric in group:
                value = group[metric]
                lines.append(f"{title}：平均 {value['mean']:.3f} MB，采样峰值 {value['peak']:.3f} MB，"
                             f"首末变化 {value['growth']:+.3f} MB，达到/超过内存目标 {value['at_or_above_limit_samples']} 条")
        if "cpu_percent" in group:
            cpu = group["cpu_percent"]
            lines.append(f"CPU：有效区间加权平均 {cpu['weighted_mean']:.4f}%，最高区间平均 {cpu['peak_interval_mean']:.4f}%")
        if "peak_threads" in group:
            lines.append(f"线程峰值：{group['peak_threads']}")
    lines.extend(["", "MB = 1,000,000 字节；CPU 100% 表示占满一个逻辑核。",
                  "峰值是采样观测值，不能排除采样间隔中的瞬时内存峰值；CPU 是区间平均值。",
                  "采样间隔过长（例如合盖睡眠）不计入 CPU 均值，详见 events.jsonl。",
                  "这里只读取整个进程指标，不能判断 GUI 内的监听是否仍开启，也不能证明不存在内存泄漏。"])
    return "\n".join(lines) + "\n"


def monitor(args):
    native = Native()
    executable = (ROOT / "target/release/KeyFlux").resolve()
    pid = args.pid if args.pid else discover(native, executable)
    if native.path(pid) != executable:
        raise RuntimeError("指定 PID 不是当前项目的 target/release/KeyFlux，拒绝混入其他进程数据。")
    initial, _, _ = native.read(pid)
    parent = ROOT / "target/performance"
    parent.mkdir(parents=True, exist_ok=True)
    folder = Path(tempfile.mkdtemp(prefix=datetime.now().strftime("%Y%m%d-%H%M%S-") + f"pid{pid}-", dir=parent))
    metadata = {"schema": 1, "started_at": stamp(), "pid": pid, "executable": str(executable),
                "disk_binary_sha256_at_attach": hashlib.sha256(executable.read_bytes()).hexdigest(),
                "process_start_ticks": initial.start, "macos": platform.mac_ver()[0], "arch": platform.machine(),
                "interval_seconds": args.interval, "duration_hours": args.hours,
                "warmup_seconds": args.warmup, "memory_limit_mb": args.memory_limit_mb,
                "bytes_per_mb": MB, "mach_seconds_per_tick": native.seconds_per_tick,
                "runtime_log_source": str(ROOT / "target/keyflux-runtime.log"),
                "notes": "只读附加，不启动、停止或重启 KeyFlux；采样器是独立进程。"}
    with (folder / "metadata.json").open("x") as output:
        json.dump(metadata, output, ensure_ascii=False, indent=2)
    stop = threading.Event()
    reason = "duration_complete"

    def stopped(signum, _frame):
        nonlocal reason
        reason = f"signal_{signal.Signals(signum).name}"
        stop.set()

    for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(signum, stopped)
    started = native.now()
    previous = None
    above = {"rss": False, "footprint": False}
    runtime_source = ROOT / "target/keyflux-runtime.log"
    runtime_offset = runtime_source.stat().st_size if runtime_source.exists() else 0
    runtime_inode = runtime_source.stat().st_ino if runtime_source.exists() else None
    print(f"正在采样 KeyFlux PID={pid}，每 {args.interval:g} 秒一次，最长 {args.hours:g} 小时。", flush=True)
    print(f"日志目录：{folder}\nCtrl+C 只停止采样并生成报告，不退出 KeyFlux。", flush=True)

    with (folder / "samples.csv").open("x", newline="", buffering=1) as samples, \
            (folder / "events.jsonl").open("x", buffering=1) as events, \
            (folder / "keyflux-runtime.log").open("x", buffering=1) as runtime:
        writer = csv.DictWriter(samples, fieldnames=FIELDS)
        writer.writeheader()

        def event(kind, **details):
            events.write(json.dumps({"timestamp": stamp(), "event": kind, **details}, ensure_ascii=False) + "\n")

        def copy_runtime():
            nonlocal runtime_offset, runtime_inode
            try:
                if not runtime_source.exists():
                    return
                status = runtime_source.stat()
                if runtime_inode != status.st_ino or status.st_size < runtime_offset:
                    runtime_offset = 0
                    event("runtime_log_reopened")
                runtime_inode = status.st_ino
                # Bound each read; disable detailed keyboard logging for this run.
                with runtime_source.open("rb") as source:
                    source.seek(runtime_offset)
                    content = source.read(1_048_576)
                    runtime_offset = source.tell()
                if content:
                    runtime.write(content.decode("utf-8", errors="replace"))
            except OSError as error:
                event("runtime_log_read_error", error=str(error))

        event("attached", pid=pid, runtime_log_from_byte=runtime_offset)
        try:
            while not stop.is_set():
                now = native.now()
                elapsed = now - started
                if elapsed >= args.hours * 3600:
                    break
                usage, task, cpu_total = native.read(pid)
                if usage.start != initial.start or native.path(pid) != executable:
                    reason = "process_identity_changed"
                    event(reason, pid=pid)
                    break
                interval, cpu = 0.0, None
                if previous:
                    interval = now - previous[0]
                    delta = cpu_total - previous[1]
                    if 0 < interval <= max(args.interval * 3, args.interval + 5) and delta >= 0:
                        cpu = delta / interval * 100
                    else:
                        event("sampling_gap_or_counter_reset", interval_seconds=interval)
                phase = "warmup" if elapsed < args.warmup else "steady"
                rss, footprint = usage.resident / MB, usage.footprint / MB
                writer.writerow(dict(zip(FIELDS, [stamp(), f"{elapsed:.3f}", f"{interval:.3f}", phase, pid,
                    "" if cpu is None else f"{cpu:.6f}", f"{cpu_total:.6f}", f"{rss:.6f}",
                    f"{footprint:.6f}", task.threads, usage.idle_wakeups, usage.interrupt_wakeups,
                    usage.pageins, usage.disk_read, usage.disk_written])))
                for metric, value in (("rss", rss), ("footprint", footprint)):
                    current = value >= args.memory_limit_mb
                    if current != above[metric]:
                        event("memory_limit_crossing", metric=metric, mb=value, at_or_above=current, phase=phase)
                        above[metric] = current
                copy_runtime()
                previous = (now, cpu_total)
                stop.wait(min(args.interval, max(0, args.hours * 3600 - (native.now() - started))))
        except OSError as error:
            reason = "process_unavailable" if error.errno in (errno.ESRCH, errno.ENOENT) else "sampling_error"
            event(reason, error=str(error))
        finally:
            copy_runtime()
            own = resource.getrusage(resource.RUSAGE_SELF)
            event("finished", reason=reason, elapsed_seconds=native.now() - started,
                  sampler_cpu_seconds=own.ru_utime + own.ru_stime, sampler_peak_rss_mb=own.ru_maxrss / MB)
    result = summarize(folder)
    result["finish_reason"] = reason
    with (folder / "summary.json").open("x") as output:
        json.dump(result, output, ensure_ascii=False, indent=2)
    text = report_text(result)
    with (folder / "summary.txt").open("x") as output:
        output.write(f"结束原因：{reason}\n" + text)
    print(text + f"结束原因：{reason}\n日志目录：{folder}")
    return 1 if reason in ("sampling_error", "process_identity_changed") else 0


def main():
    parser = argparse.ArgumentParser(description="KeyFlux 只读性能采样；默认每 30 秒记录，24 小时后结束。")
    parser.add_argument("--pid", type=int, help="手动指定当前项目 KeyFlux 的 PID")
    parser.add_argument("--interval", type=float, default=30, help="采样间隔秒，最少 1 秒，默认 30")
    parser.add_argument("--hours", type=float, default=24, help="最长采样小时数，默认 24")
    parser.add_argument("--warmup", type=float, default=120, help="从附加开始的预热秒数，默认 120；仍保存所有样本")
    parser.add_argument("--memory-limit-mb", type=float, default=15, help="严格低于此值，MB=1000000 字节，默认 15")
    parser.add_argument("--report", type=Path, help="只读取已有采样目录，在终端显示报告，不修改日志")
    args = parser.parse_args()
    if args.report:
        try:
            print(report_text(summarize(args.report.resolve())))
            return 0
        except (OSError, ValueError, KeyError) as error:
            parser.error(f"无法读取采样目录：{error}")
    if sys.platform != "darwin":
        parser.error("采样只能在 macOS 运行")
    if not all(math.isfinite(value) for value in (args.interval, args.hours, args.warmup, args.memory_limit_mb)) \
            or args.interval < 1 or args.hours <= 0 or args.warmup < 0 or args.memory_limit_mb <= 0 \
            or (args.pid is not None and args.pid <= 0):
        parser.error("间隔必须 >=1 秒；时长和内存目标必须 >0；预热 >=0；PID 必须为正整数")
    try:
        return monitor(args)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"采样未完成：{error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
