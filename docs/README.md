# RSSSTV Documentation

The documentation is divided by what each document is about, because the three
subjects answer to different authorities. A protocol description answers to the
on-air signal, a description of MMSSTV answers to its source, and a description
of RSSSTV answers to this repository's code.

## `sstv/` — the protocols

SSTV as it exists on the air, independent of any one implementation. Values
here are largely derived from the MMSSTV source, which remains the most
complete reference for modes that have no formal standard, but the subject is
the signal rather than the program.

- [sstv/modes.md](sstv/modes.md): modes, geometry, timing, and VIS
  identification.
- [sstv/fskid.md](sstv/fskid.md): the FSK station-identification protocol,
  including the callsign, contest, and narrow N-VIS records.

## `mmsstv/` — the original implementation

The behavior of the original MMSSTV source in `original/mmsstv`, which this
project treats as the reference implementation. These documents describe what
that program does, including where it departs from published descriptions.

- [mmsstv/architecture.md](mmsstv/architecture.md): the application's
  structure, state, and data flow.
- [mmsstv/dsp.md](mmsstv/dsp.md): filters, discriminators, synchronization, and
  clock correction.
- [mmsstv/modes.md](mmsstv/modes.md): how its mode table is written, and where
  it differs from public tables.
- [mmsstv/fskid.md](mmsstv/fskid.md): its FSKID detector and acquisition.
- [mmsstv/porting.md](mmsstv/porting.md): reading the original source for the
  Rust port.

## `rssstv/` — this project

The Rust implementation: what it is meant to be, and what it currently is.

- [rssstv/architecture.md](rssstv/architecture.md): target architecture and the
  current crate structure.
- [rssstv/gui-design.md](rssstv/gui-design.md): the desktop application, its
  audio boundary, and its platform integration.
- [rssstv/rig-control.md](rssstv/rig-control.md): the transports the rig is
  reached over, the script that decides what is sent, and the band plan both
  read. Describes a target design ahead of what is implemented.
- [rssstv/template-design.md](rssstv/template-design.md): the portable transmit
  overlay format.
- [rssstv/release.md](rssstv/release.md): what CI checks, and how a tag becomes
  a release.
