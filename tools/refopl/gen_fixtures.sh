#!/bin/sh
# Regenerate crates/doom_audio/tests/fixtures/*.opl from the reference
# renderer: the full register-write log (from chip reset) plus an FNV-1a 64
# hash of the reference PCM after "# start".
# Usage: tools/refopl/gen_fixtures.sh <refopl binary> <iwad>
set -e
REF=${1:?usage: $0 <refopl binary> <iwad>}
IWAD=${2:?usage: $0 <refopl binary> <iwad>}
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=$HERE/../../crates/doom_audio/tests/fixtures
TMP=$(mktemp -d)
for SONG in D_E1M1 D_INTER D_E2M8; do
    "$REF" "$IWAD" $SONG 10 "$TMP/$SONG.wav" "$TMP/$SONG.log" 44100 64 >/dev/null
    python3 - "$TMP/$SONG.wav" "$TMP/$SONG.log" "$OUT/$SONG.opl" <<'PY'
import sys
pcm = open(sys.argv[1], 'rb').read()[44:]
h = 0xcbf29ce484222325
for b in pcm:
    h = ((h ^ b) * 0x100000001b3) & 0xffffffffffffffff
with open(sys.argv[3], 'w') as out:
    out.write(f"# reference: Chocolate Doom i_oplmusic.c + Nuked OPL3 (tools/refopl), 44100 Hz, volume 64\n")
    out.write(f"# pcm_fnv1a64 {h:016x} bytes {len(pcm)}\n")
    out.write(open(sys.argv[2]).read())
PY
    echo "wrote $OUT/$SONG.opl"
done
rm -rf "$TMP"
