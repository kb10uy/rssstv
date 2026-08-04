# SSTV Modes

This document describes the SSTV modes, their geometry, timing, and
identification.

There is no single formal standard covering every mode here; most are de facto
protocols created by scan-converter or software authors, and public sources
conflict with each other. Values below are therefore largely derived from the
original MMSSTV source in `original/mmsstv`, which remains the most complete
single reference. Where that source departs from published descriptions, the
departure is recorded in [mmsstv/modes.md](../mmsstv/modes.md) rather than
here.

## Conventions

### Image Frequencies

Ordinary analog SSTV image intensity is represented by frequency:

| Signal | Frequency |
| --- | ---: |
| Horizontal sync | 1200 Hz |
| Black | 1500 Hz |
| Mid-level | 1900 Hz |
| White | 2300 Hz |

Narrow modes use a smaller image deviation:

| Signal | Frequency |
| --- | ---: |
| Horizontal sync | 1900 Hz |
| Black/porch | 2044 Hz |
| Center | 2172 Hz |
| White | 2300 Hz |

The horizontal axis is analog. A stated width such as 320 or 640 is the sample
and bitmap convention used by software, not a fixed count of on-air symbols.
Different receivers can sample the same scan at different horizontal
resolutions.

### Dimensions

The inventory below reports the bitmap a receiver allocates. Families often
reserve top rows for grayscale or identification in conventional operation;
the transport bitmap can therefore be taller than the active source image.

### Conventional VIS

The conventional VIS sequence is:

```text
1900 Hz 300 ms
1200 Hz  10 ms
1900 Hz 300 ms
1200 Hz  30 ms start
7 data bits, LSB first, 30 ms each
even parity bit, 30 ms
1200 Hz  30 ms stop
```

A data one is 1100 Hz and zero is 1300 Hz. Public tables normally list only the
seven-bit mode number, while an implementation often carries the full eight
transmitted bits including parity as one byte. Robot 36 is VIS `0x08`
publicly and `0x88` as a parity-inclusive byte.

### Extended VIS

MR, MP, and ML modes use a two-byte extension: `0x23` first, then a second
parity-bearing identifier byte using the same 30 ms LSB-first encoding. This is
an MMSSTV extension rather than a conventional seven-bit VIS assignment.

### Narrow N-VIS

Narrow modes do not use conventional VIS. They are identified by an FSK-framed
sequence sharing the FSKID physical layer, described in
[sstv/fskid.md](fskid.md).

## Mode Inventory

`VIS` gives the public seven-bit value followed by the parity-inclusive raw
byte. `Ext` and `N-VIS` give the exact source identifiers. `Period` is the
horizontal synchronization interval; paired-row modes encode two image rows in
one period.

| Mode | Identification | Bitmap | Period | Color organization |
| --- | --- | ---: | ---: | --- |
| Robot 36 | VIS `08` / `88` | 320x256, 240 picture rows | 150.000 ms | Y plus alternating R-Y/B-Y |
| Robot 72 | VIS `0c` / `0c` | 320x256, 240 picture rows | 300.000 ms | Y, R-Y, B-Y |
| AVT 90 | VIS `44` / `44`, then AVT sync | 320x256, 240 picture rows | 375.000 ms | R, G, B; no line sync in image body |
| Scottie 1 | VIS `3c` / `3c` | 320x256 | 428.220 ms | G, B, sync, R |
| Scottie 2 | VIS `38` / `b8` | 320x256 | 277.692 ms | G, B, sync, R |
| Scottie DX | VIS `4c` / `cc` | 320x256 | 1050.300 ms | G, B, sync, R |
| Martin 1 | VIS `2c` / `ac` | 320x256 | 446.446 ms | G, B, R |
| Martin 2 | VIS `28` / `28` | 320x256 | 226.798 ms | G, B, R |
| SC2 180 | VIS `37` / `b7` | 320x256 | 711.0437 ms | R, G, B |
| SC2 120 | VIS `3f` / `3f` | 320x256 | 475.52248 ms | R, G, B |
| SC2 60 | VIS `3b` / `bb` | 320x256 | 240.3846 ms | R, G, B |
| PD50 | VIS `5d` / `dd` | 320x256 | 388.160 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| PD90 | VIS `63` / `63` | 320x256 | 703.040 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| PD120 | VIS `5f` / `5f` | 640x496 | 508.480 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| PD160 | VIS `62` / `e2` | 512x400 | 804.416 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| PD180 | VIS `60` / `60` | 640x496 | 754.240 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| PD240 | VIS `61` / `e1` | 640x496 | 1000.000 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| PD290 | VIS `5e` / `de` | 800x616 | 937.280 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| P3 | VIS `71` / `71` | 640x496 | 409.375 ms | R, G, B |
| P5 | VIS `72` / `72` | 640x496 | 614.0625 ms | R, G, B |
| P7 | VIS `73` / `f3` | 640x496 | 818.750 ms | R, G, B |
| MR73 | Ext `23 45` | 320x256 | 286.3 ms | Y, half-length R-Y, half-length B-Y |
| MR90 | Ext `23 46` | 320x256 | 352.3 ms | Y, half-length R-Y, half-length B-Y |
| MR115 | Ext `23 49` | 320x256 | 450.3 ms | Y, half-length R-Y, half-length B-Y |
| MR140 | Ext `23 4a` | 320x256 | 548.3 ms | Y, half-length R-Y, half-length B-Y |
| MR175 | Ext `23 4c` | 320x256 | 684.3 ms | Y, half-length R-Y, half-length B-Y |
| MP73 | Ext `23 25` | 320x256 | 570.0 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| MP115 | Ext `23 29` | 320x256 | 902.0 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| MP140 | Ext `23 2a` | 320x256 | 1090.0 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| MP175 | Ext `23 2c` | 320x256 | 1370.0 ms | Two rows: Y0, R-Y, B-Y, Y1 |
| ML180 | Ext `23 85` | 640x496 | 363.3 ms | Y, half-length R-Y, half-length B-Y |
| ML240 | Ext `23 86` | 640x496 | 483.3 ms | Y, half-length R-Y, half-length B-Y |
| ML280 | Ext `23 89` | 640x496 | 565.3 ms | Y, half-length R-Y, half-length B-Y |
| ML320 | Ext `23 8a` | 640x496 | 645.3 ms | Y, half-length R-Y, half-length B-Y |
| Robot 24 | VIS `04` / `84` | 320x256, 240 picture rows | 200.000 ms | Y, R-Y, B-Y; one scan represents two rows |
| B/W 8 | VIS `02` / `82` | 320x256, 240 picture rows | 66.89709 ms | Luminance only; averages two source rows |
| B/W 12 | VIS `06` / `86` | 320x256, 240 picture rows | 100.000 ms | Luminance only; averages two source rows |
| MP73-N | N-VIS `02` | 320x256 | 570.0 ms | Narrow two-row Y0, R-Y, B-Y, Y1 |
| MP110-N | N-VIS `04` | 320x256 | 858.0 ms | Narrow two-row Y0, R-Y, B-Y, Y1 |
| MP140-N | N-VIS `05` | 320x256 | 1090.0 ms | Narrow two-row Y0, R-Y, B-Y, Y1 |
| MC110-N | N-VIS `14` | 320x256 | 428.5 ms | Narrow R, G, B |
| MC140-N | N-VIS `15` | 320x256 | 548.5 ms | Narrow R, G, B |
| MC180-N | N-VIS `16` | 320x256 | 704.5 ms | Narrow R, G, B |

## Mode Families

### Robot

Robot modes originated in Robot Research Corporation scan converters and
introduced VIS-based automatic mode selection.

MMSSTV's Robot 36 line is:

```text
1200 sync 9 ms
1500 porch 3 ms
Y 88 ms
TCS 4.5 ms
1900 porch 1.5 ms
alternating R-Y or B-Y 44 ms
```

The TCS frequency identifies the chroma component. One chroma scan represents
two adjacent rows.

Robot 72 transmits Y for 138 ms and both 69 ms color-difference components in
each 300 ms line. Robot 24 uses a 200 ms unit with 92 ms Y and two 46 ms chroma
components. MMSSTV duplicates its decoded Robot 24 scan into two displayed rows.

B/W 8 and B/W 12 use the Robot green-component VIS assignments and transmit one
luminance scan for two source rows. MMSSTV averages the two source rows during
TX.

### Martin

Martin M1 and M2 were created by Martin Emmerson, G3OQD. Their direct-color line
structure is:

```text
1200 sync 4.862 ms
1500 porch 0.572 ms
G
1500 separator 0.572 ms
B
1500 separator 0.572 ms
R
1500 separator 0.572 ms
```

M1 uses 146.432 ms per component. M2 uses 73.216 ms. MMSSTV stores both in a
320-wide bitmap even though M2 is conventionally described as having lower
horizontal resolution.

### Scottie

Scottie was created by Eddie Murphy, GM3SBC. It also transmits direct G, B, and
R components, but line sync lies between B and R:

```text
1500 separator 1.5 ms
G
1500 separator 1.5 ms
B
1200 sync 9 ms
1500 porch 1.5 ms
R
```

Relative to sync, a raster line begins with R and continues with the next
encoder call's G and B. MMSSTV compensates for this placement in receive raster
alignment. Scottie DX shares seven-bit VIS `0x4c` with AVT 188 in historical
mode tables; MMSSTV supports Scottie DX but only AVT 90.

### AVT 90

AVT, the Amiga Video Transceiver format, uses a synchronous image body without
per-line horizontal sync. MMSSTV supports only AVT 90. It transmits repeated VIS
`0x44`, an AVT digital synchronization train, then 125 ms each of R, G, and B.

The demodulator has a separate PLL-based AVT acquisition and 16-bit sync decoder
in `sstv.cpp:2155`. Exact MMSSTV framing and rounding should be taken from the
source rather than generalized AVT descriptions.

### Wraase SC-2

SC-2 modes originated with Wraase equipment. MMSSTV supports SC2-60, SC2-120,
and SC2-180 and transmits:

```text
1200 sync
1500 porch 0.5 ms
R
G
B
```

MMSSTV assigns equal duration to all three color components in every SC-2 mode:

| Mode | Sync | Each component |
| --- | ---: | ---: |
| SC2-60 | 5.5006 ms | 78.128 ms |
| SC2-120 | 5.52248 ms | 156.5 ms |
| SC2-180 | 5.5437 ms | 235 ms |

Some public SC2-60 and SC2-120 descriptions instead specify a 1:2:1 R:G:B time
allocation. The equal-component values above are deliberate descriptions of
MMSSTV source behavior, not a normalization to those external specifications.

### PD

PD was developed by Paul Turner, G4IJE, and Don Rotier, K0HEO. One transmitted
unit represents two picture rows:

```text
1200 sync 20 ms
1500 porch 2.08 ms
Y0
R-Y shared by both rows
B-Y shared by both rows
Y1
```

All four components have equal duration and no internal separator. The long
sync supports reliable AFC. MMSSTV implements PD50, PD90, PD120, PD160, PD180,
PD240, and PD290 with the published geometry and timing.

### Pasokon P3, P5, and P7

John Langner's Pasokon modes use direct R, G, and B and a 640x496 transport
image, conventionally 16 header rows plus 480 picture rows. Every line consists
of 1965 timing units:

```text
25 units sync
5 units porch
640 units R
5 units separator
640 units G
5 units separator
640 units B
5 units porch
```

P3, P5, and P7 use 4800, 3200, and 2400 units per second respectively. Their
names approximate transmission duration in minutes.

### MMSSTV MP, MR, and ML

These Makoto Mori/MMSSTV families use extended VIS beginning with `0x23`.

MP resembles PD at 320x256 but uses 9 ms sync, 1 ms porch, and equal-duration
`Y0, R-Y, B-Y, Y1` components.

MR uses 320x256 Y/R-Y/B-Y with one full-duration Y scan and two half-duration
chroma scans. ML uses the same organization at 640x496. The source inserts
approximately 0.1 ms held-value intervals between the components.

### MMSSTV Narrow Modes

MP73-N, MP110-N, and MP140-N retain MP's paired-row organization while reducing
the image range to 2044-2300 Hz and using 1900 Hz horizontal sync.

MC110-N, MC140-N, and MC180-N are direct narrow-band R/G/B modes. Their line is
8 ms of 1900 Hz sync, 0.5 ms at 2044 Hz, then three equal color components.

## References

The MMSSTV source remains the primary reference for exact behavior; see
[mmsstv/modes.md](../mmsstv/modes.md). The following public sources were used
for history and independent specification checks:

- J. L. Barber, N7CXI, [Proposal for SSTV Mode Specifications](https://www.classicsstv.com/graphics/daytonpaper.pdf), Dayton SSTV Forum, 2000. Firmware-derived Robot, Martin, Scottie, SC2-180, Pasokon, and PD timing.
- Paul Turner, G4IJE, [The development of the PD modes](https://www.classicsstv.com/pdmodes.php). Creator-published history and PD specification.
- Paul Turner, G4IJE, [Martin Modes](https://www.classicsstv.com/martin_mode.php). Contemporary implementation history.
- John Langner, WB2OSZ, [SSTV Transmission Modes](https://docs.preterhuman.net/SSTV_Transmission_Modes), March 1996 compilation. VIS map and published Pasokon specification.
- Martin Bruchanov, OK2MNM, [Image Communication on Short Waves, Chapter 4](https://www.sstv-handbook.com/download/sstv_04.pdf). Mode histories, signal structures, and MMSSTV-specific families.
- Martin Bruchanov, OK2MNM, [Image Communication on Short Waves, Chapter 5](https://www.sstv-handbook.com/download/sstv_05.pdf). Consolidated mode and VIS tables.
- [MMSSTV distribution page](https://hamsoft.ca/pages/mmsstv.php). MMSSTV authorship and release information.

Public sources contain conflicts with each other and with the MMSSTV source.
Where they differ, the difference is recorded in
[mmsstv/modes.md](../mmsstv/modes.md).
