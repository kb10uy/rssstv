# FSKID

FSKID is the FSK station-identification protocol that accompanies an SSTV
transmission. It carries a callsign, optionally a contest value, and in narrow
modes the mode identifier itself.

There is no formal standard. The published description in
`original/mmsstv/fskid.txt` is the closest thing to one, but the executable
behavior of MMSSTV is authoritative where the two disagree, and one such
disagreement is recorded below. How MMSSTV detects and acquires the signal is
described in [mmsstv/fskid.md](../mmsstv/fskid.md).

## Physical Encoding

FSKID uses two audio tones:

| Meaning | Frequency |
| --- | ---: |
| Mark / one | 1900 Hz |
| Space / zero | 2100 Hz |

Each bit lasts 22 ms, giving a nominal rate of 45.45 bits/s. A symbol contains
six bits and has no per-symbol start or stop bit. Bits are transmitted least
significant first.

This differs from the `B5` through `B0` order printed in `fskid.txt`. The
MMSSTV source and its receive shift register agree on least-significant-bit-first
operation, so a compatible implementation must follow the code.

A normal callsign record is preceded by:

```text
2100 Hz  100 ms  space guard
1900 Hz   22 ms  start interval
six-bit symbols
```

MMSSTV normally puts a 300 ms 1500 Hz footer before this sequence. Narrow modes
use 1900 Hz for that footer. The footer is part of application transmit
scheduling, not the FSKID record itself.

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

At least one encoded character is required. The effective maximum is 16
characters. The value `0x01` is reserved as EOT, which means an exclamation
mark cannot occur as ordinary callsign data.

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

A contest value may follow a valid callsign checksum directly, without another
guard or `0x2a` header.

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
Transmission uppercases the text. The effective maximum is eight characters.

## Narrow N-VIS

Narrow SSTV modes do not use conventional VIS. They reuse the FSKID physical
layer with this framing:

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

Unlike a callsign FSKID, N-VIS identifies the mode and starts image reception.
The narrow modes it selects are described in [sstv/modes.md](modes.md).
