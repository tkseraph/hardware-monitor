#!/usr/bin/env bash
# Mac M0 探测入口：仅只读、普通权限，不安装组件。
# 输出：docs/probes/<timestamp>-mac-m4.json

set -euo pipefail
cd "$(dirname "$0")/../.."

TS="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="docs/probes/${TS}-mac-m4.json"
mkdir -p docs/probes

echo "[probe] 生成报告: ${OUT}"
python3 scripts/probes/probe-all.py --output "${OUT}"
echo "[probe] 完成: ${OUT}"
