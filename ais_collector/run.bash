#!/bin/bash
# ⭐ Give the collector high I/O priority (best-effort class 0 = highest)
# and normal CPU priority — it MUST keep writing 24/7 without interruption.
nohup ionice -c2 -n0 python3 ./collect_ais.py > /mnt/wdblack/ais_collection.log 2>&1 &
echo "✅ Collector started (PID $!) with high I/O priority."
