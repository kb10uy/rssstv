# Rig Control

The application controls the station's rig through Hamlib, and reaches Hamlib
through a `rigctld` the operator already has running rather than through a
library linked into the build.

## Why `rigctld`

Linking `libhamlib` would put an autotools C build into every platform's
toolchain, ship a shared library beside the executable, and complicate cross
compilation, all so that this application could own the serial port. Owning it
is itself the problem: a CAT port takes one process, and an SSTV session
normally runs beside a logger that wants the same rig. `rigctld` is Hamlib's own
answer to both — it holds the port and serves any number of clients over TCP —
so talking to it costs a socket and no build integration at all.

The consequence is that the operator starts `rigctld` themselves. That is the
same arrangement WSJT-X offers as its `Hamlib NET rigctl` rig type, and it is
what a station running more than one program is doing anyway.

## The Protocol

`rssstv-rig` speaks the `rigctld` text protocol on a plain TCP socket.

Every command is sent in the protocol's extended form, prefixed with `+`.
Without it, a `rigctld` that succeeds at a `get` command answers with the value
alone and no terminator, so how many lines an answer runs to is a fact about
the particular command. The operator's commands are exactly the ones this crate
has no such fact about. The extended form answers with a terminating
`RPRT <status>` line whatever the command was and whether or not it succeeded,
which makes the end of an answer readable without knowing what was asked. That
it also labels its values, so nothing has to be read by position, is a
convenience on top.

Commands are sent one at a time and each answer is read to its terminator
before the next command is written. Nothing is pipelined, so a sequence of
commands is a sequence of round trips.

On connecting, the session asks `\chk_vfo` to find out whether `rigctld` was
started with `--vfo` and therefore wants every command addressed to a VFO. The
answer is applied to the commands this crate sends for itself — currently only
`\get_freq`. The operator's commands go out exactly as written: which of them
take a VFO is a property of the command, and guessing would break the ones that
do not. A `rigctld` too old to answer the question is one that does not want the
argument either, so a refusal settles as no VFO rather than as a failure.

A command the rig refuses is reported with Hamlib's own status number and
leaves the connection open. A transport that fails ends the session.

### One Command per Line

A command must be one command. Whitespace is normalized when a script is read
and a newline is whitespace, so a command can never carry one and each is
written to the socket as exactly one line — that much holds by construction.
What does not hold by construction is the other direction: two commands' worth
of words on a single line, such as `T 1 T 0`.

That reaches `rigctld` as one line and one answer is read back, but the far end
may find a second command in the words left over and answer twice. The extra
`RPRT` then stays in the stream and every later answer is read one behind. The
damage is bounded — a read that never terminates hits the command timeout and
ends the session, and the interface reports a failed rig rather than
transmitting — but an answer read one behind can also be a stale `RPRT 0`,
which is a keying failure mistaken for success. Write one command per line and
the question does not arise.

This is the shape of the risk rather than an observed failure: exactly how
`rigctld` treats the words left over on a line has not been measured here, and
the tests in this crate answer from a stand-in rather than from Hamlib. Nothing
detects the desynchronization if it happens; draining the socket before each
command would, and is not implemented.

## Configuration

Everything the rig is told lives in `config.toml` under `[rig]`. The
application writes the section out with its defaults on the first save, so the
keys are there to be edited rather than having to be discovered.

```toml
[rig]
enabled = true
address = "127.0.0.1:4532"
poll-interval = 1.0
lead-in = 0.2
tail = 0.05

[rig.commands]
open = ""
close = ""
transmit = """
L MONITOR_GAIN 0.15
T 1"""
receive = "T 0"

[rig.bands]
"40m" = '\set_ant 1 0'
"20m" = '\set_ant 2 0'
```

| Key | Meaning |
| --- | --- |
| `enabled` | Whether to connect at all. The Rig Control menu is the same switch. |
| `address` | Where `rigctld` is listening. |
| `poll-interval` | Seconds between frequency reads. `0` never reads the frequency. |
| `lead-in` | Seconds between keying the rig and the first audio sample. |
| `tail` | Seconds between the last audio sample and unkeying. |

`lead-in` covers the time a rig takes to switch to transmit, which its audio
path does not wait for: anything sent inside it is lost. `tail` covers the
opposite end, where the ring buffer is empty but the device has not finished
playing what it was handed.

### Commands

An event holds one string: the commands to send, one per line, written exactly
as they would be typed at `rigctl`. Both the short forms and the long
`\set_level` forms work, because neither is interpreted here — the line is
passed through, and `rigctld` splits it on whitespace itself. A blank line is
spacing rather than a command, and a line is one command rather than several,
for the reason given under [One Command per Line](#one-command-per-line).

A single command needs no ceremony:

```toml
transmit = "T 1"
```

Several want TOML's multi-line form. Use the literal `'''` quoting for anything
containing a backslash, which is every Hamlib command written in full:

```toml
transmit = '''
\set_mode PKTUSB 3000
\set_ptt 1'''
```

Each line is its own round trip, in the order it was written:

```text
-> +\set_mode PKTUSB 3000
<- set_mode:
<- RPRT 0
-> +\set_ptt 1
<- set_ptt:
<- RPRT 0
```

The commands attached to an event run in the order they are written and stop at
the first one the rig refuses. A sequence that selects a data mode before keying
only means anything if the keying does not happen when the mode change failed.

| Event | When it runs |
| --- | --- |
| `open` | Immediately after connecting. |
| `close` | Before the connection is given up. |
| `transmit` | At the start of a transmission, before any audio. |
| `receive` | After the last sample has been played, plus `tail`. |

`transmit` and `receive` default to `T 1` and `T 0`, so keying works before
anything is configured. A key that is present replaces the default outright,
including with nothing: a station keyed by VOX writes `transmit = ""` and means
it. A key that is absent, or that holds something that is not text, leaves the
default in place.

### Bands

`[rig.bands]` attaches commands to an amateur band, sent when the polled
frequency arrives on it. Connecting while already on a band counts as arriving,
so a station that selects an antenna per band selects one without waiting for
the operator to tune somewhere else first. Staying on a band does not resend
them, and leaving the bands entirely and coming back is an arrival again.

Band names are the ones an operator writes: `160m` through `10m`, `6m`, `4m`,
`2m`, `1.25m`, `70cm`, `33cm`, `23cm`, plus `2200m` and `630m`. A name outside
that list is dropped rather than carried around, and is not written back on the
next save.

The band edges are the widest any region allocates. They name a frequency and
pick the commands attached to it; deciding what may be transmitted where is the
operator's licence rather than this table's job.

## Behavior Around a Transmission

The connection is owned by a worker thread. Keying means a socket round trip and
then the lead-in, and neither belongs in a frame.

1. The interface asks for keying as the transmission is set up, so the lead-in
   runs alongside filling the audio queue rather than after it.
2. The worker runs the `transmit` commands, waits out `lead-in`, and reports
   that it is transmitting.
3. The interface starts the audio device only once the rig has said so. A rig
   that was asked to key and refused stops the transmission instead: a
   transmission nobody hears is worse than one that did not happen.
4. When the queue has drained, the worker waits out `tail` and runs the
   `receive` commands.

A transmission that is cancelled or that fails unkeys by the same path, as does
switching rig control off while one is running.

Polling stops while the rig is keyed. Reading the frequency back
mid-transmission says nothing the operator cannot see, and it puts CAT traffic
on the wire during the one part of a session that has to be left alone.

While rig control is switched on and not connected, a transmission is refused
with what the rig said. Switching it off transmits anyway, which is the whole of
what the menu offers besides the connection's state.

## Template Variables

`${radio.frequency}` and `${radio.band}` are filled from the polled frequency.
Without a connection they hold a fixed placeholder, because the transmit tab has
to compose to something before there is a radio to ask and a missing variable
would refuse to render at all. A rig tuned between the bands leaves
`${radio.band}` empty: the frequency beside it is real, and a band that
contradicted it would be worse than none.

A frame that prints either of them stops being true the moment the operator
tunes, so it is composed again when the rig moves — and only then, for the same
reason a frame that prints the clock is composed again on the minute. A template
that says nothing about the frequency is left alone.
