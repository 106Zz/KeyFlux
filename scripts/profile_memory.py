#!/usr/bin/env python3
"""Guided memory snapshots of an existing KeyFlux process; never control its UI."""

import argparse
import csv
from datetime import datetime
import hashlib
import json
import math
from pathlib import Path
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
import monitor_keyflux as monitor


STAGES = [
    ("01_gui_idle", "保持 KeyFlux 运行，从菜单栏 ⇧ 选择“停止监听”；不要退出程序。", True),
    ("02_monitor_idle", "从菜单栏 ⇧ 选择“开始监听”，保持空闲。", False),
    ("03_switch_batch1", "在常用输入框完成第一批切换：建议 50 次单按 Shift、20 次 Caps；确认输入正常。", False),
    ("04_switch_batch2", "在同一输入框再做相同的一批切换，次数尽量相同。", False),
    ("05_monitor_stopped", "从菜单栏 ⇧ 选择“停止监听”，保留进程，不要退出 KeyFlux。", False),
]


def check_listener(pid, label):
    status = monitor.runtime_status(pid)
    if status.state == "unknown":
        raise RuntimeError("无法从 target/keyflux-runtime.log 确认本进程的监听状态。请按 README 重定向启动日志后重测，未确认的阶段不采集。")
    expected = label not in ("01_gui_idle", "05_monitor_stopped")
    if status.state == "recovering" or (status.state == "listening") != expected:
        raise RuntimeError(f"{label} 的监听状态不符合预期（实际：{status.state}）。本次测试中止，请查看运行日志中的错误，不要继续下一批。")
    return status.snapshot()


def check_faults(before, after):
    keys = ("monitor_failures", "recovery_attempts", "recovery_failures", "recovery_successes")
    if any(after[key] != before[key] for key in keys):
        raise RuntimeError("本阶段发生监听故障 / 自动重建，或日志窗口变化。即使已经自动恢复，也不能记为无异常压力测试；本次诊断中止，保留运行日志后排查。")


def save_json(path, value):
    with path.open("x") as output:
        json.dump(value, output, ensure_ascii=False, indent=2)


def metrics(native, pid, start, executable):
    usage, task, cpu = native.read(pid)
    if usage.start != start or native.path(pid) != executable:
        raise RuntimeError("目标进程身份已变化，请结束本次诊断，重新启动脚本。")
    return {"timestamp": monitor.stamp(), "rss_mb": usage.resident / monitor.MB,
            "footprint_mb": usage.footprint / monitor.MB, "threads": task.threads,
            "cpu_time_s": cpu}


def capture(native, pid, start, executable, folder, label, check_leaks=False):
    before = metrics(native, pid, start, executable)
    tools = [("vmmap", ["/usr/bin/vmmap", "-summary", str(pid)]),
             ("footprint", ["/usr/bin/footprint", "-p", str(pid), "-f", "bytes"])]
    if check_leaks:
        # No object content dumps or memory graph files.
        tools.append(("leaks", ["/usr/bin/leaks", "--noContent", str(pid)]))
    statuses = []
    for name, command in tools:
        metrics(native, pid, start, executable)
        began = time.monotonic()
        with (folder / f"{label}-{name}.txt").open("x") as output:
            try:
                result = subprocess.run(command, stdout=output, stderr=subprocess.STDOUT,
                                        timeout=25, start_new_session=True)
                status = {"tool": name, "command": command, "returncode": result.returncode}
            except subprocess.TimeoutExpired:
                output.write("\nDiagnostic tool timed out after 25 seconds.\n")
                status = {"tool": name, "command": command, "error": "timeout"}
            except OSError as error:
                output.write(f"\n{error}\n")
                status = {"tool": name, "command": command, "error": str(error)}
        status["elapsed_seconds"] = time.monotonic() - began
        statuses.append(status)
        if status.get("returncode") != 0:
            print(f"  {name} 未正常完成，已保存原始输出；不会自动提权或重试。", flush=True)
    after = metrics(native, pid, start, executable)
    value = {"stage": label, "before_tools": before, "after_tools": after, "tools": statuses}
    save_json(folder / f"{label}-metrics.json", value)
    return value


def report(folder, stages, reason):
    fields = ["stage", "timestamp", "rss_mb", "footprint_mb", "rss_change_mb",
              "footprint_change_mb", "threads", "failed_tools"]
    lines = ["# KeyFlux 分阶段内存快照", "", f"结束状态：{reason}", "",
             "以下数值取每次运行诊断工具之前；MB = 1,000,000 字节。", "",
             "| 阶段 | RSS MB | Footprint MB | RSS 较上阶段变化 | Footprint 较上阶段变化 | 线程 |",
             "| --- | ---: | ---: | ---: | ---: | ---: |"]
    previous = None
    failures = []
    with (folder / "comparison.csv").open("x", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=fields)
        writer.writeheader()
        for stage in stages:
            current = stage["before_tools"]
            rss_delta = current["rss_mb"] - previous["rss_mb"] if previous else 0
            footprint_delta = current["footprint_mb"] - previous["footprint_mb"] if previous else 0
            failed = [tool["tool"] for tool in stage["tools"] if tool.get("returncode") != 0]
            writer.writerow({"stage": stage["stage"], "timestamp": current["timestamp"],
                             "rss_mb": current["rss_mb"], "footprint_mb": current["footprint_mb"],
                             "rss_change_mb": rss_delta, "footprint_change_mb": footprint_delta,
                             "threads": current["threads"], "failed_tools": ",".join(failed)})
            lines.append(f"| {stage['stage']} | {current['rss_mb']:.3f} | {current['footprint_mb']:.3f} | "
                         f"{rss_delta:+.3f} | {footprint_delta:+.3f} | {current['threads']} |")
            if failed:
                failures.append(f"阶段 {stage['stage']} 的工具未正常完成：{', '.join(failed)}。")
            previous = current
    lines.extend(["", *failures])
    lines.extend(["", "阶段间差值包含初始化、缓存、系统回收和操作差异，不能直接当作模块独占内存或泄漏量。",
                  "vmmap / footprint 是区域分类快照，不能直接定位到 Rust 分配调用栈。",
                  "如需调用栈，下一轮再单独启用分配跟踪；本脚本不改变环境变量或重新启动应用。",
                  "诊断工具可能短暂影响目标进程响应；这轮数据用于定位，不代替正常运行的性能验收。",
                  "工具失败需查看对应 txt 和 metrics.json；leaks 非零退出需结合输出判断，不能直接当成权限失败。"])
    with (folder / "comparison.md").open("x") as output:
        output.write("\n".join(lines) + "\n")
    save_json(folder / "result.json", {"reason": reason, "stages_completed": len(stages), "stages": stages})


def main():
    parser = argparse.ArgumentParser(description="分阶段记录当前 KeyFlux 的内存分类，不启动或停止应用。")
    parser.add_argument("--pid", type=int, help="指定当前项目 KeyFlux PID，默认自动查找")
    parser.add_argument("--settle", type=float, default=30, help="每阶段确认后静置秒数，默认 30")
    parser.add_argument("--baseline-wait", type=float, default=120, help="第一次基线静置秒数，默认 120")
    parser.add_argument("--leaks", action="store_true", help="最后阶段额外运行 leaks --noContent，默认不运行")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("此脚本只能在 macOS 运行")
    if not all(math.isfinite(v) and v >= 0 for v in (args.settle, args.baseline_wait)) \
            or (args.pid is not None and args.pid <= 0):
        parser.error("等待时间必须为有限非负数，PID 必须为正整数")
    if not sys.stdin.isatty():
        parser.error("请在交互式终端运行，脚本需要逐步确认你的手动操作")
    folder = None
    stages = []
    reason = "completed"
    try:
        native = monitor.Native()
        executable = (monitor.ROOT / "target/release/KeyFlux").resolve()
        pid = args.pid or monitor.discover(native, executable)
        initial, _, _ = native.read(pid)
        metrics(native, pid, initial.start, executable)
        parent = monitor.ROOT / "target/memory-profile"
        parent.mkdir(parents=True, exist_ok=True)
        folder = Path(tempfile.mkdtemp(prefix=datetime.now().strftime("%Y%m%d-%H%M%S-") + f"pid{pid}-", dir=parent))
        save_json(folder / "metadata.json", {"started_at": monitor.stamp(), "pid": pid,
            "process_start_ticks": initial.start, "executable": str(executable),
            "disk_binary_sha256_at_attach": hashlib.sha256(executable.read_bytes()).hexdigest(),
            "settle_seconds": args.settle, "baseline_wait_seconds": args.baseline_wait,
            "leaks_requested": args.leaks, "bytes_per_mb": monitor.MB})
        print(f"附加 PID={pid}；本次目录：{folder}", flush=True)
        print("不会自动按键、点击 GUI、停止或重启 KeyFlux。保持同一个进程；请先关闭详细诊断。", flush=True)
        with (folder / "notes.jsonl").open("x", buffering=1) as notes:
            for label, instruction, baseline in STAGES:
                before_runtime = monitor.runtime_status(pid).snapshot()
                print(f"\n{label}：{instruction}", flush=True)
                note = input("完成后回到此终端，输入操作次数/应用/异常等备注（可留空），按回车继续：")
                notes.write(json.dumps({"timestamp": monitor.stamp(), "stage": label, "note": note}, ensure_ascii=False) + "\n")
                delay = args.baseline_wait if baseline else args.settle
                check_faults(before_runtime, check_listener(pid, label))
                print(f"静置 {delay:g} 秒；快照完成前请暂停输入操作。", flush=True)
                time.sleep(delay)
                check_faults(before_runtime, check_listener(pid, label))
                value = capture(native, pid, initial.start, executable, folder, label,
                                check_leaks=args.leaks and label == STAGES[-1][0])
                after_runtime = check_listener(pid, label)
                check_faults(before_runtime, after_runtime)
                value["runtime_counter_delta"] = {key: after_runtime[key] - before_runtime[key] for key in monitor.RUNTIME_COUNTERS}
                stages.append(value)
                before = value["before_tools"]
                print(f"已保存：RSS={before['rss_mb']:.3f} MB，Footprint={before['footprint_mb']:.3f} MB。", flush=True)
    except (KeyboardInterrupt, EOFError):
        reason = "user_interrupted"
        print("\n诊断已中断，保留已采集数据；脚本不改变 KeyFlux 的监听状态。")
    except (OSError, RuntimeError) as error:
        reason = f"failed: {error}"
        print(reason, file=sys.stderr)
    finally:
        if folder:
            report(folder, stages, reason)
            print(f"快照与对比报告：{folder}", flush=True)
    if reason != "completed":
        return 1
    return int(any(tool.get("returncode") != 0 for stage in stages for tool in stage["tools"]))


if __name__ == "__main__":
    raise SystemExit(main())
