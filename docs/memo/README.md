# RSSSTV Development Documentation

The documentation is divided by what each document is about, because the three
subjects answer to different authorities. A protocol description answers to the
on-air signal, a description of MMSSTV answers to its source, and a description
of RSSSTV answers to this repository's code.

None of it is written for the operator. The manual the release archives carry
is [../help/index.md](../help/index.md), which describes the application from
the outside and is the only documentation a release ships.

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
- [mmsstv/jasta.md](mmsstv/jasta.md): MMJASTA, the contest scorer bundled with
  the source, and the MDT log format it reads.
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
- [rssstv/web-demo.md](rssstv/web-demo.md): the browser build of the receive
  path, what the page has to do around it, and what the audio APIs do to the
  samples on the way in.
- [rssstv/release.md](rssstv/release.md): what CI checks, and how a tag becomes
  a release.
