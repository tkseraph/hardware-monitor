#!/usr/bin/env bash
# Mac M0 探测入口：仅只读、普通权限，不安装组件。
# 输出：scripts/probes/reports/<timestamp>-mac-m4.json（该目录被 .gitignore 忽略，
# 本机诊断报告默认不进入版本控制，更不随提交外发 —— R12/A14）。

set -euo pipefail
cd "$(dirname "$0")/../.."

TS="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="scripts/probes/reports/${TS}-mac-m4.json"
mkdir -p scripts/probes/reports

echo "[probe] 生成报告: ${OUT}"
python3 scripts/probes/probe-all.py --output "${OUT}"
echo "[probe] 完成: ${OUT}"
