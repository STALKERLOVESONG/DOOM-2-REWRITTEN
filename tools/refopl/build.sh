#!/bin/sh
# Build the reference OPL music renderer from a Chocolate Doom checkout.
# Usage: tools/refopl/build.sh <chocolate-doom checkout> [output]
set -e
CHOCO=${1:?usage: $0 <chocolate-doom checkout> [output]}
OUT=${2:-refopl}
HERE=$(cd "$(dirname "$0")" && pwd)
CFLAGS="-O2 -w -I$HERE/sdlstub -I$HERE/../refdoom/sdlstub -I$HERE/../refdoom -I$CHOCO/src -I$CHOCO/opl"
cc $CFLAGS \
    "$CHOCO/src/i_oplmusic.c" "$CHOCO/src/mus2mid.c" "$CHOCO/src/midifile.c" \
    "$CHOCO/src/memio.c" "$CHOCO/src/m_misc.c" \
    "$CHOCO/opl/opl.c" "$CHOCO/opl/opl3.c" "$CHOCO/opl/opl_queue.c" \
    "$HERE/refopl_driver.c" "$HERE/refopl_main.c" -lm -o "$OUT"
echo "built $OUT"
