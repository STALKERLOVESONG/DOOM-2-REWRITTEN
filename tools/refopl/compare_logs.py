#!/usr/bin/env python3
"""Compare OPL register-write logs from tools/refopl (Chocolate Doom) and
crates/doom_audio/examples/music_trace.rs.

usage: compare_logs.py <reference.log> <ours.log>

Reports the first differing write (register/value order) and the timing
difference between matching writes, in samples."""
import sys

def load(path):
    start, rows, started = 0, [], False
    for line in open(path):
        if line.startswith('# start'):
            start, started = int(line.split()[2]), True
            rows = []  # writes before the song (chip init) are not compared
            continue
        if line.startswith('#'):
            continue
        t, r, v = line.split()
        rows.append((int(t), int(r, 16), int(v, 16)))
    return [(t - start, r, v) for t, r, v in rows]

ref, ours = load(sys.argv[1]), load(sys.argv[2])
n = min(len(ref), len(ours))
first = next((i for i in range(n) if ref[i][1:] != ours[i][1:]), None)
print(f"writes: reference {len(ref)}, ours {len(ours)}")
if first is None:
    print(f"register/value sequence identical for {n} writes")
else:
    print(f"first difference at write {first}:")
    for i in range(max(0, first - 5), min(n, first + 10)):
        mark = '>>' if ref[i][1:] != ours[i][1:] else '  '
        print(f"{mark} {i:6}  ref t={ref[i][0]:8} {ref[i][1]:03x}={ref[i][2]:02x}   ours t={ours[i][0]:8} {ours[i][1]:03x}={ours[i][2]:02x}")
upto = first if first is not None else n
d = [ours[i][0] - ref[i][0] for i in range(upto)]
if d:
    print(f"timing (ours - ref, samples) over {upto} matching writes: min {min(d)}, max {max(d)}, last {d[-1]}")
