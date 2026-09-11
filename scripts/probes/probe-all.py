#!/usr/bin/env python3
"""
Mac M0 探测脚本：仅普通权限、只读、不安装组件。
输出符合 docs/probes/contract.md 的 JSON 报告。
"""
import json
import os
import platform
import re
import subprocess
import sys
import time
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

# ---------- 工具 ----------

def run(cmd: List[str], timeout: int = 30) -> Dict[str, Any]:
    """执行命令，返回 {exit_code, stdout, stderr}；不抛异常。"""
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return {"exit_code": p.returncode, "stdout": p.stdout.strip(), "stderr": p.stderr.strip()}
    except Exception as exc:
        return {"exit_code": -1, "stdout": "", "stderr": str(exc)}

def now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

def bytes_int(s: str) -> Optional[int]:
    try:
        return int(s)
    except Exception:
        return None

# ---------- 主机信息 ----------

def host_info() -> Dict[str, Any]:
    hw_model = run(["sysctl", "-n", "hw.model"])["stdout"]
    chip = run(["sysctl", "-n", "machdep.cpu.brand_string"])["stdout"]
    mem_bytes = bytes_int(run(["sysctl", "-n", "hw.memsize"])["stdout"])
    phys = bytes_int(run(["sysctl", "-n", "hw.physicalcpu"])["stdout"])
    logical = bytes_int(run(["sysctl", "-n", "hw.logicalcpu"])["stdout"])
    nperf = bytes_int(run(["sysctl", "-n", "hw.nperflevels"])["stdout"]) or 0
    perf_levels = []
    for i in range(nperf):
        name = run(["sysctl", "-n", f"hw.perflevel{i}.name"])["stdout"]
        p = bytes_int(run(["sysctl", "-n", f"hw.perflevel{i}.physicalcpu"])["stdout"])
        l = bytes_int(run(["sysctl", "-n", f"hw.perflevel{i}.logicalcpu"])["stdout"])
        perf_levels.append({"name": name, "physical": p, "logical": l})
    sw = run(["sw_vers"])
    os_version = ""
    build = ""
    for line in sw["stdout"].splitlines():
        if line.startswith("ProductVersion:"):
            os_version = line.split(":", 1)[1].strip()
        elif line.startswith("BuildVersion:"):
            build = line.split(":", 1)[1].strip()
    return {
        "model": hw_model,
        "chip": chip,
        "os_version": os_version,
        "build": build,
        "arch": platform.machine(),
        "memory_bytes": mem_bytes,
        "cpu_physical_cores": phys,
        "cpu_logical_cores": logical,
        "cpu_perf_levels": perf_levels,
    }

# ---------- CPU ----------

def probe_cpu(host: Dict[str, Any]) -> List[Dict[str, Any]]:
    probes = []
    # 拓扑
    probes.append({
        "id": "cpu.topology",
        "category": "cpu",
        "description": "CPU 拓扑：物理/逻辑核心数、性能等级",
        "source": "sysctl hw.physicalcpu hw.logicalcpu hw.nperflevels",
        "status": "verified",
        "value": {
            "physical_cores": host["cpu_physical_cores"],
            "logical_cores": host["cpu_logical_cores"],
            "perf_levels": host["cpu_perf_levels"],
        },
        "unit": "count",
        "sampled_at": now(),
        "sample_window_ms": 0,
        "confidence": "high",
        "notes": "静态拓扑；P/E 映射到具体逻辑处理器索引未验证",
        "evidence": {"raw": f"phys={host['cpu_physical_cores']} logical={host['cpu_logical_cores']} perf_levels={len(host['cpu_perf_levels'])}"},
    })

    # 总占用率：使用 iostat -c 2 差分
    r = run(["iostat", "-c", "2", "-w", "1"], timeout=5)
    if r["exit_code"] == 0:
        lines = r["stdout"].splitlines()
        # 第二次采样在最后一行（索引 3）；格式: KB/t tps MB/s (per disk) ... us sy id 1m 5m 15m
        if len(lines) >= 4:
            parts = lines[3].split()
            # 最后 6 个字段: us sy id 1m 5m 15m
            if len(parts) >= 6:
                try:
                    us = float(parts[-6])
                    sy = float(parts[-5])
                    id_ = float(parts[-4])
                    total_usage = 100.0 - id_
                    probes.append({
                        "id": "cpu.total_usage",
                        "category": "cpu",
                        "description": "CPU 总占用率（%）",
                        "source": "iostat -c 2 -w 1",
                        "status": "verified",
                        "value": {"user": us, "system": sy, "idle": id_, "total_usage": total_usage},
                        "unit": "%",
                        "sampled_at": now(),
                        "sample_window_ms": 1000,
                        "confidence": "medium",
                        "notes": "iostat 差分，第二次采样值",
                        "evidence": {"raw": lines[3]},
                    })
                except (ValueError, IndexError):
                    probes.append({
                        "id": "cpu.total_usage",
                        "category": "cpu",
                        "description": "CPU 总占用率（%）",
                        "source": "iostat -c 2 -w 1",
                        "status": "error",
                        "value": None,
                        "unit": "%",
                        "sampled_at": now(),
                        "sample_window_ms": 0,
                        "confidence": "low",
                        "notes": f"解析失败: {lines[3]}",
                        "evidence": {},
                    })
            else:
                probes.append({
                    "id": "cpu.total_usage",
                    "category": "cpu",
                    "description": "CPU 总占用率（%）",
                    "source": "iostat -c 2 -w 1",
                    "status": "error",
                    "value": None,
                    "unit": "%",
                    "sampled_at": now(),
                    "sample_window_ms": 0,
                    "confidence": "low",
                    "notes": f"行字段不足: {lines[3]}",
                    "evidence": {},
                })
        else:
            probes.append({
                "id": "cpu.total_usage",
                "category": "cpu",
                "description": "CPU 总占用率（%）",
                "source": "iostat -c 2 -w 1",
                "status": "error",
                "value": None,
                "unit": "%",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "low",
                "notes": f"行数不足: {len(lines)}",
                "evidence": {},
            })
    else:
        probes.append({
            "id": "cpu.total_usage",
            "category": "cpu",
            "description": "CPU 总占用率（%）",
            "source": "iostat -c 2 -w 1",
            "status": "error",
            "value": None,
            "unit": "%",
            "sampled_at": now(),
            "sample_window_ms": 0,
            "confidence": "low",
            "notes": f"iostat 失败: {r['stderr'][:200]}",
            "evidence": {},
        })

    # 每核占用率：iostat 不提供每核，需 host_processor_info 或 sysinfo
    probes.append({
        "id": "cpu.per_core_usage",
        "category": "cpu",
        "description": "每逻辑处理器占用率（%）",
        "source": "host_processor_info 或 sysinfo",
        "status": "not_implemented",
        "value": None,
        "unit": "%",
        "sampled_at": now(),
        "sample_window_ms": 0,
        "confidence": "low",
        "notes": "host_processor_info 需验证权限；sysinfo 为 Rust 依赖；当前未实现",
        "evidence": {},
    })
    return probes

# ---------- 内存 ----------

def probe_memory(host: Dict[str, Any]) -> List[Dict[str, Any]]:
    probes = []
    probes.append({
        "id": "memory.total",
        "category": "memory",
        "description": "物理内存总量",
        "source": "sysctl hw.memsize",
        "status": "verified",
        "value": host["memory_bytes"],
        "unit": "bytes",
        "sampled_at": now(),
        "sample_window_ms": 0,
        "confidence": "high",
        "notes": "静态总量",
        "evidence": {"raw": str(host["memory_bytes"])},
    })

    # 动态内存：memory_pressure + sysctl
    r = run(["memory_pressure"], timeout=5)
    if r["exit_code"] == 0:
        try:
            page_size = bytes_int(run(["sysctl", "-n", "hw.pagesize"])["stdout"]) or 16384
            free = int(re.search(r'Pages free: (\d+)', r["stdout"]).group(1)) * page_size
            active = int(re.search(r'Pages active: (\d+)', r["stdout"]).group(1)) * page_size
            inactive = int(re.search(r'Pages inactive: (\d+)', r["stdout"]).group(1)) * page_size
            wired = int(re.search(r'Pages wired down: (\d+)', r["stdout"]).group(1)) * page_size
            compressor = int(re.search(r'Pages used by compressor: (\d+)', r["stdout"]).group(1)) * page_size
            speculative = int(re.search(r'Pages speculative: (\d+)', r["stdout"]).group(1)) * page_size
            purgeable = int(re.search(r'Pages purgeable: (\d+)', r["stdout"]).group(1)) * page_size

            used = active + wired + compressor
            available = free + inactive + speculative + purgeable
            total_calc = used + available
            total_expected = host["memory_bytes"]
            diff = total_expected - total_calc

            probes.append({
                "id": "memory.used_available",
                "category": "memory",
                "description": "内存已用/可用（动态）",
                "source": "memory_pressure + sysctl vm.page_free_count",
                "status": "verified",
                "value": {
                    "used_bytes": used,
                    "available_bytes": available,
                    "free_bytes": free,
                    "active_bytes": active,
                    "inactive_bytes": inactive,
                    "wired_bytes": wired,
                    "compressed_bytes": compressor,
                    "speculative_bytes": speculative,
                    "purgeable_bytes": purgeable,
                    "total_calculated": total_calc,
                    "total_expected": total_expected,
                    "diff_bytes": diff,
                    "used_percent": round(used / total_expected * 100, 2),
                },
                "unit": "bytes",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "medium",
                "notes": f"口径: used=active+wired+compressor; available=free+inactive+speculative+purgeable; 与物理总量差异 {diff} bytes (~{diff/1024**2:.1f}MB)",
                "evidence": {"raw": "memory_pressure 输出已解析"},
            })
        except Exception as exc:
            probes.append({
                "id": "memory.used_available",
                "category": "memory",
                "description": "内存已用/可用（动态）",
                "source": "memory_pressure + sysctl vm.page_free_count",
                "status": "error",
                "value": None,
                "unit": "bytes",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "low",
                "notes": f"解析失败: {exc}",
                "evidence": {},
            })
    else:
        probes.append({
            "id": "memory.used_available",
            "category": "memory",
            "description": "内存已用/可用（动态）",
            "source": "memory_pressure + sysctl vm.page_free_count",
            "status": "error",
            "value": None,
            "unit": "bytes",
            "sampled_at": now(),
            "sample_window_ms": 0,
            "confidence": "low",
            "notes": f"memory_pressure 失败: {r['stderr'][:200]}",
            "evidence": {},
        })

    # 进程内存排行：ps -A -o pid,rss,comm 然后按 RSS 数值降序取前 10。
    # 注意：不能用 `ps -r` —— 它按 CPU% 排序，不是内存 (F16)。
    r = run(["ps", "-A", "-o", "pid,rss,comm"], timeout=10)
    if r["exit_code"] == 0:
        try:
            lines = r["stdout"].strip().split('\n')
            processes = []
            for line in lines[1:]:  # 跳过表头
                parts = line.split(None, 2)
                if len(parts) >= 3:
                    try:
                        processes.append({
                            "pid": int(parts[0]),
                            "rss_kb": int(parts[1]),
                            "name": parts[2].split('/')[-1],  # 只保留进程名，不含路径
                        })
                    except ValueError:
                        continue
            # 按 RSS 降序取前 10
            processes.sort(key=lambda p: p["rss_kb"], reverse=True)
            processes = processes[:10]
            probes.append({
                "id": "memory.process_rank",
                "category": "memory",
                "description": "进程内存排行（RSS）",
                "source": "ps -A -o pid,rss,comm, sorted by rss desc",
                "status": "verified",
                "value": {"top_processes": processes},
                "unit": "KB",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "medium",
                "notes": "RSS 为驻留内存；不含 footprint；权限受限进程可能无法读取",
                "evidence": {"count": len(processes)},
            })
        except Exception as exc:
            probes.append({
                "id": "memory.process_rank",
                "category": "memory",
                "description": "进程内存排行（RSS）",
                "source": "ps -A -o pid,rss,comm, sorted by rss desc",
                "status": "error",
                "value": None,
                "unit": "KB",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "low",
                "notes": f"解析失败: {exc}",
                "evidence": {},
            })
    else:
        probes.append({
            "id": "memory.process_rank",
            "category": "memory",
            "description": "进程内存排行（RSS）",
            "source": "ps -A -o pid,rss,comm, sorted by rss desc",
            "status": "error",
            "value": None,
            "unit": "KB",
            "sampled_at": now(),
            "sample_window_ms": 0,
            "confidence": "low",
            "notes": f"ps 失败: {r['stderr'][:200]}",
            "evidence": {},
        })
    return probes

# ---------- GPU ----------

def probe_gpu() -> List[Dict[str, Any]]:
    probes = []
    # GPU 名称：实际从 system_profiler 读取，不硬编码 (F16)。
    gpu_name = None
    r_sp = run(["system_profiler", "SPDisplaysDataType", "-json"], timeout=15)
    if r_sp["exit_code"] == 0:
        try:
            sp = json.loads(r_sp["stdout"])
            items = sp.get("SPDisplaysDataType", [])
            if items:
                gpu_name = items[0].get("_name")
        except (json.JSONDecodeError, KeyError, IndexError):
            gpu_name = None
    probes.append({
        "id": "gpu.name",
        "category": "gpu",
        "description": "GPU 名称",
        "source": "system_profiler SPDisplaysDataType",
        "status": "verified" if gpu_name else "error",
        "value": gpu_name,
        "unit": "",
        "sampled_at": now(),
        "sample_window_ms": 0,
        "confidence": "high" if gpu_name else "low",
        "notes": "集成 GPU，与系统共享统一内存" if gpu_name else "system_profiler 未返回名称",
        "evidence": {"raw": gpu_name} if gpu_name else {},
    })
    # GPU 利用率和内存统计：ioreg PerformanceStatistics
    r = run(["ioreg", "-l", "-w", "0"], timeout=10)
    if r["exit_code"] == 0:
        import re
        perf_stats = re.findall(r'"PerformanceStatistics" = (\{[^}]+\})', r["stdout"])
        if perf_stats:
            # 解析第一个 PerformanceStatistics（通常是主 GPU）
            stats_str = perf_stats[0]
            # 提取数值字段
            def extract_num(key):
                m = re.search(rf'"{key}"=(\d+)', stats_str)
                return int(m.group(1)) if m else None

            device_util = extract_num("Device Utilization %")
            renderer_util = extract_num("Renderer Utilization %")
            tiler_util = extract_num("Tiler Utilization %")
            in_use_mem = extract_num("In use system memory")
            alloc_mem = extract_num("Alloc system memory")

            probes.append({
                "id": "gpu.utilization",
                "category": "gpu",
                "description": "GPU 总利用率（%）",
                "source": "ioreg PerformanceStatistics",
                "status": "verified",
                "value": {
                    "device_utilization": device_util,
                    "renderer_utilization": renderer_util,
                    "tiler_utilization": tiler_util,
                },
                "unit": "%",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "medium",
                "notes": "ioreg 快照；Device Utilization 为综合指标",
                "evidence": {"raw": stats_str[:200]},
            })

            probes.append({
                "id": "gpu.memory",
                "category": "gpu",
                "description": "GPU 内存用量/占用率（统一内存架构）",
                "source": "ioreg PerformanceStatistics",
                "status": "verified",
                "value": {
                    "in_use_system_memory": in_use_mem,
                    "alloc_system_memory": alloc_mem,
                    "note": "统一内存架构，无独立显存分母；值为驱动分配的系统内存",
                },
                "unit": "bytes",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "medium",
                "notes": "In use = 当前使用，Alloc = 驱动已分配；非全局 GPU 内存统计",
                "evidence": {"raw": stats_str[:200]},
            })
        else:
            probes.append({
                "id": "gpu.utilization",
                "category": "gpu",
                "description": "GPU 总利用率（%）",
                "source": "ioreg PerformanceStatistics",
                "status": "unavailable",
                "value": None,
                "unit": "%",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "low",
                "notes": "ioreg 中未找到 PerformanceStatistics",
                "evidence": {},
            })
            probes.append({
                "id": "gpu.memory",
                "category": "gpu",
                "description": "GPU 内存用量/占用率（统一内存架构）",
                "source": "ioreg PerformanceStatistics",
                "status": "unavailable",
                "value": None,
                "unit": "bytes/%",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "low",
                "notes": "ioreg 中未找到 PerformanceStatistics",
                "evidence": {},
            })
    else:
        probes.append({
            "id": "gpu.utilization",
            "category": "gpu",
            "description": "GPU 总利用率（%）",
            "source": "ioreg PerformanceStatistics",
            "status": "error",
            "value": None,
            "unit": "%",
            "sampled_at": now(),
            "sample_window_ms": 0,
            "confidence": "low",
            "notes": f"ioreg 失败: {r['stderr'][:200]}",
            "evidence": {},
        })
        probes.append({
            "id": "gpu.memory",
            "category": "gpu",
            "description": "GPU 内存用量/占用率（统一内存架构）",
            "source": "ioreg PerformanceStatistics",
            "status": "error",
            "value": None,
            "unit": "bytes/%",
            "sampled_at": now(),
            "sample_window_ms": 0,
            "confidence": "low",
            "notes": f"ioreg 失败: {r['stderr'][:200]}",
            "evidence": {},
        })
    return probes

# ---------- 磁盘 ----------

def probe_disks() -> List[Dict[str, Any]]:
    probes = []
    r = run(["diskutil", "list", "-plist", "physical"])
    if r["exit_code"] == 0:
        import plistlib
        try:
            data = plistlib.loads(r["stdout"].encode())
            disks = []
            for entry in data.get("AllDisksAndPartitions", []):
                dev = entry.get("DeviceIdentifier")
                if not dev:
                    continue
                info_r = run(["diskutil", "info", "-plist", dev])
                if info_r["exit_code"] != 0:
                    continue
                info = plistlib.loads(info_r["stdout"].encode())
                disks.append({
                    "id": dev,
                    "name": info.get("MediaName", ""),
                    "bus": info.get("BusProtocol", ""),
                    "internal": info.get("Internal", False),
                    "size_bytes": info.get("TotalSize", 0),
                    "smart": info.get("SMARTStatus", ""),
                })
            probes.append({
                "id": "disk.inventory",
                "category": "disk",
                "description": "物理磁盘清单",
                "source": "diskutil list/info -plist",
                "status": "verified",
                "value": disks,
                "unit": "",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "high",
                "notes": "静态清单；SMARTStatus 为摘要，非温度/通电/磨损",
                "evidence": {"count": len(disks)},
            })
        except Exception as exc:
            probes.append({
                "id": "disk.inventory",
                "category": "disk",
                "description": "物理磁盘清单",
                "source": "diskutil list/info -plist",
                "status": "error",
                "value": None,
                "unit": "",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "low",
                "notes": f"解析失败: {exc}",
                "evidence": {},
            })
    else:
        probes.append({
            "id": "disk.inventory",
            "category": "disk",
            "description": "物理磁盘清单",
            "source": "diskutil list/info -plist",
            "status": "error",
            "value": None,
            "unit": "",
            "sampled_at": now(),
            "sample_window_ms": 0,
            "confidence": "low",
            "notes": f"diskutil 失败: {r['stderr'][:200]}",
            "evidence": {},
        })

    # 磁盘吞吐：iostat 按盘拆分
    r = run(["iostat", "-d", "-c", "2", "-w", "1"], timeout=5)
    if r["exit_code"] == 0:
        lines = r["stdout"].splitlines()
        if len(lines) >= 4:
            # 第一行是磁盘名称，第二行是表头，第三行是第一次采样，第四行是第二次采样
            disk_names = lines[0].split()
            data_line = lines[3].split()
            # 每个磁盘3个字段: KB/t tps MB/s
            disks = []
            for i, disk_name in enumerate(disk_names):
                base_idx = i * 3
                if base_idx + 2 < len(data_line):
                    try:
                        kb_t = float(data_line[base_idx])
                        tps = float(data_line[base_idx + 1])
                        mb_s = float(data_line[base_idx + 2])
                        disks.append({
                            "device": disk_name,
                            "kb_per_transfer": kb_t,
                            "tps": tps,
                            "mb_per_sec": mb_s,
                        })
                    except (ValueError, IndexError):
                        continue
            probes.append({
                "id": "disk.throughput",
                "category": "disk",
                "description": "物理盘读写速率（MB/s）",
                "source": "iostat -d -c 2 -w 1",
                "status": "verified",
                "value": {"disks": disks, "total_mb_per_sec": sum(d["mb_per_sec"] for d in disks)},
                "unit": "MB/s",
                "sampled_at": now(),
                "sample_window_ms": 1000,
                "confidence": "medium",
                "notes": "iostat 差分，第二次采样值，按盘拆分",
                "evidence": {"raw": lines[3]},
            })
        else:
            probes.append({
                "id": "disk.throughput",
                "category": "disk",
                "description": "物理盘读写速率（MB/s）",
                "source": "iostat -d -c 2 -w 1",
                "status": "error",
                "value": None,
                "unit": "MB/s",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "low",
                "notes": f"行数不足: {len(lines)}",
                "evidence": {},
            })
    else:
        probes.append({
            "id": "disk.throughput",
            "category": "disk",
            "description": "物理盘读写速率（MB/s）",
            "source": "iostat -d -c 2 -w 1",
            "status": "error",
            "value": None,
            "unit": "MB/s",
            "sampled_at": now(),
            "sample_window_ms": 0,
            "confidence": "low",
            "notes": f"iostat 失败: {r['stderr'][:200]}",
            "evidence": {},
        })

    # 卷容量与物理盘/容器关系：diskutil apfs list
    r = run(["diskutil", "apfs", "list", "-plist"], timeout=15)
    if r["exit_code"] == 0:
        import plistlib
        try:
            data = plistlib.loads(r["stdout"].encode())
            containers = []
            for container in data.get("Containers", []):
                container_ref = container.get("ContainerReference")
                physical_store = container.get("PhysicalStores", [{}])[0].get("DeviceIdentifier") if container.get("PhysicalStores") else None
                volumes = []
                for vol in container.get("Volumes", []):
                    volumes.append({
                        "id": vol.get("DeviceIdentifier"),
                        "name": vol.get("Name"),
                        "role": vol.get("Role"),
                        "mount_point": vol.get("MountPoint"),
                        "capacity_consumed": vol.get("CapacityInUse"),
                    })
                containers.append({
                    "container_ref": container_ref,
                    "physical_store": physical_store,
                    "capacity_ceiling": container.get("CapacityCeiling"),
                    "capacity_in_use": container.get("CapacityInUse"),
                    "capacity_free": container.get("CapacityFree"),
                    "volumes": volumes,
                })
            probes.append({
                "id": "disk.volume_capacity",
                "category": "disk",
                "description": "卷容量与物理盘/容器关系",
                "source": "diskutil apfs list -plist",
                "status": "verified",
                "value": containers,
                "unit": "bytes",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "high",
                "notes": "APFS 容器共享空间，容量已用按容器统计，卷容量为消耗值",
                "evidence": {"count": len(containers)},
            })
        except Exception as exc:
            probes.append({
                "id": "disk.volume_capacity",
                "category": "disk",
                "description": "卷容量与物理盘/容器关系",
                "source": "diskutil apfs list -plist",
                "status": "error",
                "value": None,
                "unit": "bytes",
                "sampled_at": now(),
                "sample_window_ms": 0,
                "confidence": "low",
                "notes": f"解析失败: {exc}",
                "evidence": {},
            })
    else:
        probes.append({
            "id": "disk.volume_capacity",
            "category": "disk",
            "description": "卷容量与物理盘/容器关系",
            "source": "diskutil apfs list -plist",
            "status": "error",
            "value": None,
            "unit": "bytes",
            "sampled_at": now(),
            "sample_window_ms": 0,
            "confidence": "low",
            "notes": f"diskutil 失败: {r['stderr'][:200]}",
            "evidence": {},
        })

    # 注意：thermal.cpu / thermal.gpu / fan.rpm 由 probe_thermal_fan() 统一注册，
    # 这里不再重复 (F16 重复 ID)。

    # 磁盘温度/通电时间：本脚本未实现读取（Rust 应用已通过 diskutil
    # SMARTDeviceSpecificKeysMayVaryNotGuaranteed 读到 TEMPERATURE /
    # POWER_ON_HOURS_0，说明普通权限下并非不可用）。探测脚本未验证前
    # 标 not_implemented，不能写 unavailable (F16)。
    probes.append({
        "id": "disk.temperature",
        "category": "disk",
        "description": "磁盘温度",
        "source": "diskutil SMARTDeviceSpecificKeysMayVaryNotGuaranteed.TEMPERATURE",
        "status": "not_implemented",
        "value": None,
        "unit": "celsius",
        "sampled_at": now(),
        "sample_window_ms": 0,
        "confidence": "low",
        "notes": "探测脚本未实现；Rust 应用已从 diskutil 读到该字段，待统一验证",
        "evidence": {},
    })
    probes.append({
        "id": "disk.power_on_hours",
        "category": "disk",
        "description": "磁盘通电时间",
        "source": "diskutil SMARTDeviceSpecificKeysMayVaryNotGuaranteed.POWER_ON_HOURS_0",
        "status": "not_implemented",
        "value": None,
        "unit": "hours",
        "sampled_at": now(),
        "sample_window_ms": 0,
        "confidence": "low",
        "notes": "探测脚本未实现；多段高字节字段（POWER_ON_HOURS_1 等）未验证",
        "evidence": {},
    })
    return probes

# ---------- 温度 / 风扇 ----------

def probe_thermal_fan() -> List[Dict[str, Any]]:
    probes = []
    probes.append({
        "id": "thermal.cpu",
        "category": "thermal",
        "description": "CPU 温度",
        "source": "SMC / IOKit 传感器",
        "status": "unavailable",
        "value": None,
        "unit": "celsius",
        "sampled_at": now(),
        "sample_window_ms": 0,
        "confidence": "high",
        "notes": "powermetrics 需 root；SMC 私有 API 需 IOKit；普通权限下不可用",
        "evidence": {},
    })
    probes.append({
        "id": "thermal.gpu",
        "category": "thermal",
        "description": "GPU 温度",
        "source": "SMC / IOKit 传感器",
        "status": "unavailable",
        "value": None,
        "unit": "celsius",
        "sampled_at": now(),
        "sample_window_ms": 0,
        "confidence": "high",
        "notes": "同 CPU 温度，普通权限下不可用",
        "evidence": {},
    })
    probes.append({
        "id": "fan.rpm",
        "category": "fan",
        "description": "风扇转速",
        "source": "SMC / IOKit 传感器",
        "status": "not_implemented",
        "value": None,
        "unit": "rpm",
        "sampled_at": now(),
        "sample_window_ms": 0,
        "confidence": "low",
        "notes": "本脚本未实际检测风扇传感器；找不到 ioreg 字段不能推断无风扇。需独立验证。",
        "evidence": {},
    })
    return probes

# ---------- 汇总 ----------

def build_report() -> Dict[str, Any]:
    host = host_info()
    probes: List[Dict[str, Any]] = []
    probes.extend(probe_cpu(host))
    probes.extend(probe_memory(host))
    probes.extend(probe_gpu())
    probes.extend(probe_disks())
    probes.extend(probe_thermal_fan())

    summary = {"verified": 0, "partial": 0, "blocked_permission": 0, "unavailable": 0, "error": 0, "not_implemented": 0, "blockers": []}
    for p in probes:
        summary[p["status"]] = summary.get(p["status"], 0) + 1
        if p["status"] in ("blocked_permission", "error"):
            summary["blockers"].append({"id": p["id"], "notes": p["notes"]})

    return {
        "report_version": "0.1",
        "generated_at": now(),
        "host": host,
        "permissions": {
            "mode": "normal",
            "enhanced_authorized": False,
            "enhanced_available": False,
            "notes": "普通权限运行，未提权，未安装增强组件",
        },
        "probes": probes,
        "summary": summary,
    }

def main():
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--output", required=True)
    args = ap.parse_args()
    report = build_report()
    with open(args.output, "w", encoding="utf-8") as f:
        json.dump(report, f, ensure_ascii=False, indent=2)
    print(f"报告已生成: {args.output}")
    print(f"状态统计: {report['summary']}")

if __name__ == "__main__":
    main()
