# MMSSTV FSKID

This document describes how MMSSTV detects and acquires FSKID. The protocol
itself is described in [sstv/fskid.md](../sstv/fskid.md); only behavior that
belongs to this implementation is recorded here.

The executable behavior is split between the receiver in `sstv.cpp` and the
transmit orchestration in `Main.cpp`, so neither file describes the feature on
its own.

## Source Map

The relevant original sources are:

- `fskid.txt:1-47`: published wire-format description.
- `sstv.h:704-722`: FSK constants and receive state.
- `sstv.cpp:1446-1455`: 1900 and 2100 Hz detector construction.
- `sstv.cpp:1819-1858`: per-sample detector processing.
- `sstv.cpp:2378-2606`: receive acquisition and record parsing.
- `sstv.cpp:2942-2949`: six-bit transmit order.
- `Main.cpp:6457-6518`: callsign and contest-record transmission.
- `Main.cpp:6939-6969`: narrow-mode N-VIS transmission.

The `CEXTFSK` code in `Comm.cpp` is an unrelated external radio/DLL interface.

`CSSTVMOD::WriteFSK()` shifts the value right after every emitted bit, which is
where the least-significant-bit-first transmit order is decided.

## Receive Detector

`CSSTVDEM` runs parallel 100 Hz-bandwidth resonators at 1900 and 2100 Hz. Each
output is full-wave rectified and smoothed by a second-order 50 Hz low-pass
filter. The detector bank receives AGC-normalized audio and is retuned along
with the receiver AFC.

At a decision point, MMSSTV requires the absolute envelope difference to be at
least 2048 on its 16384-scale detector input. The larger envelope then selects
mark or space. An insufficient difference is ambiguous and resets an active
record. The corresponding normalized contrast in RSSSTV is 0.125.

## Receive Acquisition

`CSSTVDEM::DecodeFSK()` operates once for every input sample:

1. Wait for a sufficiently strong space decision.
2. Require space continuously for 50 ms, half of `FSKGARD`.
3. Search for mark for at most another 100 ms.
4. After detecting mark, wait 11 ms and confirm mark at its midpoint.
5. Sample one data bit every 22 ms.
6. Assemble six chronological bits into one symbol.

Symbol deadlines are accumulated as floating-point sample positions and then
converted to integer positions. This avoids accumulating error at rates such as
11025 Hz, where 22 ms is 242.55 samples. There is no timing recovery after data
sampling begins; an ambiguous bit decision abandons the record.

The first data-bit sample occurs 22 ms after the start interval's midpoint. It
therefore falls approximately at the midpoint of the first bit following the
complete 22 ms start interval.

## Record Limits and Quirks

The callsign state machine resets when a seventeenth character is received. The
original storage is larger, so the 16-character maximum is a consequence of that
reset rather than a declared limit. The contest-record receive state reaches its
eight-character maximum the same way.

The enable checks are inconsistent: callsign and text-contest events honor
`m_fskdecode`, while numeric contest and narrow N-VIS processing do not.

Narrow N-VIS is not merely identification here: it selects the receive mode and
starts image reception when the synchronization settings permit it. Header
creation is in `Main.cpp:6939` and decoding is in `sstv.cpp:2552`.
