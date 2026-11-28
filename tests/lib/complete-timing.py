#!/usr/bin/env python3
"""How long nami takes to say what fits, for each kind of tab completion
(`nami __complete`): each asked RUNS times, the median and the slowest.
usage: complete-timing.py [nami] (default: nami on PATH). Prints a line each.
"""
import statistics
import subprocess
import sys
import time

nami = sys.argv[1] if len(sys.argv) > 1 else "nami"
RUNS = 30
CASES = [
    ("a command", ["inst"]),
    ("an option", ["install", "--"]),
    ("packages: fire", ["install", "fire"]),
    ("packages: f", ["install", "f"]),
    ("packages: all", ["install", ""]),
    ("versions: firefox@", ["install", "firefox@"]),
    ("installed: remove", ["remove", ""]),
    ("tasks: run", ["run", ""]),
    ("services: up", ["up", ""]),
    ("changes: undo", ["undo", ""]),
    ("keys: config set", ["config", "set", ""]),
    ("desktop look", ["desktop", "look", ""]),
    ("starters: new --from", ["new", "x", "--from", ""]),
]

for name, words in CASES:
    times = []
    lines = 0
    for _ in range(RUNS):
        t = time.perf_counter()
        out = subprocess.run([nami, "__complete", "bash", "--", *words], capture_output=True, text=True).stdout
        times.append((time.perf_counter() - t) * 1000)
        lines = len(out.splitlines())
    print(f"{name:24} median {statistics.median(times):5.1f} ms   slowest {max(times):5.1f} ms   ({lines} answers)")
