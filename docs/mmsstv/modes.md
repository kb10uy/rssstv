# MMSSTV Mode Implementation

This document records how MMSSTV expresses the SSTV modes, and where its
behavior differs from public tables. The modes themselves — geometry, timing,
and identification — are described in [sstv/modes.md](../sstv/modes.md), whose
values are largely derived from this source.

## Source Map

MMSSTV defines 43 receive/transmit modes in `sstv.h:450`. Their names and UI
order are in `sstv.cpp:493`, timing and geometry in `sstv.cpp:607`, RX raster
conversion in `Main.cpp:3715`, and TX line generators in `Main.cpp:6088`. The
narrow-mode frequency constants are declared at `sstv.h:440`.

The source enum names the narrow MP-like modes `smMN73`, `smMN110`, and
`smMN140`; the UI displays them as MP73-N, MP110-N, and MP140-N.

MMSSTV usually represents the full eight transmitted VIS bits, including
parity, as one byte, whereas public tables normally list only the seven-bit
mode number. Robot 36 is VIS `0x08` publicly, while MMSSTV transmits and
compares the parity-inclusive byte `0x88`.

## Notes and Differences

- VIS comments in `sstv.cpp` often show the seven-bit value while switch cases
  use the parity-inclusive byte.
- The P5 decoder comment says `$71`, but RX and TX code both use `0x72`.
- AVT framing, extended VIS parity, and N-VIS framing are implementation-specific
  areas where MMSSTV source should control compatibility behavior.
- MMSSTV's SC2-60/120 component allocation differs from common public tables.
- MC110-N uses 140 ms components in source, not the 143 ms found in one
  secondary table.
- One public handbook table gives 143 ms per component for MC110-N; the source
  uses 140 ms and a 428.5 ms period. Source behavior is authoritative here.
- `m_OFP` and other receive offsets are empirically tuned synchronization
  positions and do not necessarily equal literal TX porch boundaries.
- Bitmap dimensions describe MMSSTV storage. They should not be interpreted as
  a mandatory analog horizontal sample count. For Robot and AVT, MMSSTV
  allocates 256 rows but treats 240 as picture rows.
