# MMSSTV FSKID

This document describes the FSK station-identification protocol implemented by
MMSSTV. The short protocol description in `original/mmsstv/fskid.txt` is useful,
but the executable behavior is split between the receiver in `sstv.cpp` and the
transmit orchestration in `Main.cpp`.

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

## Physical Encoding

FSKID uses two audio tones:

| Meaning | Frequency |
| --- | ---: |
| Mark / one | 1900 Hz |
| Space / zero | 2100 Hz |

Each bit lasts 22 ms, giving a nominal rate of 45.45 bits/s. A symbol contains
six bits and has no per-symbol start or stop bit. `CSSTVMOD::WriteFSK()` shifts
the value right after every emitted bit, so bits are transmitted least
significant first.

This differs from the `B5` through `B0` order printed in `fskid.txt`. The source
code and receive shift register agree on least-significant-bit-first operation,
so a compatible implementation must follow the code.

A normal callsign record is preceded by:

```text
2100 Hz  100 ms  space guard
1900 Hz   22 ms  start interval
six-bit symbols
```

MMSSTV normally puts a 300 ms 1500 Hz footer before this sequence. Narrow modes
use 1900 Hz for that footer. The footer is part of application transmit
scheduling, not the FSKID record itself.

## Receive Detector

`CSSTVDEM` runs parallel 100 Hz-bandwidth resonators at 1900 and 2100 Hz. Each
output is full-wave rectified and smoothed by a second-order 50 Hz low-pass
filter. The detector bank receives AGC-normalized audio and is retuned along
with the receiver AFC.

At a decision point, MMSSTV requires the absolute envelope difference to be at
least 2048 on its 16384-scale detector input. The larger envelope then selects
mark or space. An insufficient difference is ambiguous and resets an active
record. In RSSSTV the corresponding normalized contrast is 0.125.

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

## Callsign Record

The station-identification frame is:

```text
0x2a C1 C2 ... CN 0x01 CHECKSUM
CHECKSUM = C1 XOR C2 XOR ... XOR CN
```

Each character in the ASCII range `0x20..0x5f` is encoded by subtracting
`0x20`, producing a six-bit value in `0x00..0x3f`. The receiver adds `0x20`
again, checks the XOR modulo six bits, and trims leading and trailing spaces and
tabs.

At least one encoded character is required. Although the original storage is
larger, the state machine resets when a seventeenth character is received, so
the effective maximum is 16 characters. The value `0x01` is reserved as EOT,
which means an exclamation mark cannot occur as ordinary callsign data.

For `JL1HIS`, the complete symbol vector is:

```text
2a 2a 2c 11 28 29 33 01 25
```

The first `2a` is the frame header. The remaining values decode as:

| Character | Six-bit value |
| --- | ---: |
| `J` | `2a` |
| `L` | `2c` |
| `1` | `11` |
| `H` | `28` |
| `I` | `29` |
| `S` | `33` |

Their XOR is `0x25`.

## Optional Contest Record

MMSSTV can append a contest value directly after a valid callsign checksum,
without another guard or `0x2a` header.

A numeric value from 0 through 4095 is encoded as:

```text
0x02 HIGH LOW CHECKSUM
HIGH = (value >> 6) AND 0x3f
LOW = value AND 0x3f
CHECKSUM = 0x02 XOR HIGH XOR LOW
```

The receiver formats the result as decimal with a minimum width of three.

A text value is encoded as:

```text
S1 S2 ... SN 0x01 CHECKSUM
CHECKSUM = S1 XOR S2 XOR ... XOR SN
```

Text symbols must be at least `0x10`, corresponding to ASCII `0x30..0x5f`.
Transmission uppercases the text. The receive state has an effective maximum of
eight characters.

The original enable checks are inconsistent: callsign and text-contest events
honor `m_fskdecode`, while numeric contest and narrow N-VIS processing do not.

## Narrow N-VIS

Narrow SSTV modes reuse the same physical layer with this framing:

```text
1900 Hz 300 ms
2100 Hz 100 ms
1900 Hz  22 ms
0x2d 0x15 MODE (MODE XOR 0x15)
```

The mode values are:

| Value | Mode |
| ---: | --- |
| `0x02` | MP73-N |
| `0x04` | MP110-N |
| `0x05` | MP140-N |
| `0x14` | MC110-N |
| `0x15` | MC140-N |
| `0x16` | MC180-N |

Unlike a callsign FSKID, N-VIS selects the receive mode and starts image
reception when the synchronization settings permit it.

## RSSSTV Port

The implementation is divided by responsibility:

- `rssstv-fskid` owns the sample-driven acquisition timing, six-bit assembly,
  callsign framing, checksum validation, and bounded identifier value.
- `rssstv-demodulator` reuses its existing AFC-adjusted 1900 and 2100 Hz
  resonators and converts their normalized envelopes to mark, space, or
  ambiguous samples.
- `decode-wav` carries validated identifiers in `DecodeReport` and writes each
  one to stdout as `fskid: CALLSIGN`.

The core accepts classified detector samples rather than audio amplitudes. It is
therefore independent of audio backends and detector scaling, while preserving
MMSSTV's protocol timing. The current port recognizes callsign records. Contest
records, N-VIS events, and FSKID transmission remain future work.
