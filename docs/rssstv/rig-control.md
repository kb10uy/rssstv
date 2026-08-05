# Rig Control

The application keys the station's rig, reads what it is tuned to, and moves it
between bands. How any of that is actually done differs by station, so the
application supplies the moments and the operator supplies the means.

This document describes the target design. What is currently implemented is
narrower; see [Status](#status).

## Why a Script

A station keys its rig in more ways than one protocol covers. MMSSTV lets the
operator write a custom PTT command as raw CI-V bytes on a serial port; EXTFSK
hands keying to an external plugin driving DTR or RTS; Hamlib does it through
`rigctld` over a socket. A station may use two at once — CAT through `rigctld`
for frequency, a separate serial line for PTT — and the three have no operation
in common that could be modelled once.

So the boundary is not *which commands* but *which transport*, and the choice
between them is the operator's. The application opens the transports named in
the configuration and calls a Lua script at each moment it reaches; what the
script sends over which transport is its own business.

An earlier design put command lines directly in `config.toml`. It could express
a Hamlib command and nothing else, could not branch, and could not read an
answer back. This replaces it.

## Why `rigctld` Rather Than Linked Hamlib

The Hamlib transport is a socket to a `rigctld` the operator already has
running, rather than `libhamlib` linked into the build.

Linking it would put an autotools C build into every platform's toolchain, ship
a shared library beside the executable, and complicate cross compilation, all
so that this application could own the serial port. Owning it is itself the
problem: a CAT port takes one process, and an SSTV session normally runs beside
a logger that wants the same rig. `rigctld` is Hamlib's own answer to both — it
holds the port and serves any number of clients — so talking to it costs a
socket and no build integration at all.

The consequence is that the operator starts `rigctld` themselves. That is the
same arrangement WSJT-X offers as its `Hamlib NET rigctl` rig type, and it is
what a station running more than one program is doing anyway.

## Layering

| Layer | Holds | Where |
| --- | --- | --- |
| Transports | Sockets and serial ports, framed and typed | `rssstv-rig` |
| Policy | What to send, and when, for this station | `rigcontrol.lua` |
| Band data | Where each band is and what to do on it | `bands.toml` |
| Timing and wiring | Which transports exist, keying delays, poll rate | `config.toml` |

The Lua host and the worker thread live in the application crate rather than in
`rssstv-rig`: a scripting host is application policy, and the reusable crate
stays what it is now, a description of how to talk to a rig.

## Status

Implemented:

- The `rigctld` transport, its extended-response framing, and `\chk_vfo`.
- Named transports under `[rig.ports]`.
- The Lua host: `rigcontrol.lua`, the compiled-in default, the context, ports
  as script objects, the call deadline, and every entry point.
- Keying around a transmission with a lead-in and a tail, and refusing a
  transmission the rig would not take.
- `bands.toml`, the compiled-in default plan, and the radio panel.
- Frequency polling into `${radio.frequency}` and `${radio.band}`.

Not yet implemented, and described here as the target:

- The serial transport, and with it the CI-V and DTR/RTS keying that motivates
  the design.

## The Script

The script lives beside `config.toml` as `rigcontrol.lua`.

A default is compiled into the application and used whenever the file is
absent. It is **not** written to disk on first run: a file written once is a
file that never gains a later fix, and most operators never need to edit it.
The Rig Control menu offers to write it out for those who do, and from then on
their copy is what runs.

### Module Shape

The script is a module: it returns a table of functions.

```lua
local function transmit(ctx)
  ctx.ports.rig:send("T 1")
end

local function receive(ctx)
  ctx.ports.rig:send("T 0")
end

local function poll_frequency(ctx)
  return ctx.ports.rig:frequency()
end

local function set_frequency(ctx, hz)
  ctx.ports.rig:send(("F %d"):format(hz))
end

local function change_band(ctx, band)
  set_frequency(ctx, band.target)
  if band.receive_mode then
    ctx.ports.rig:send(("M %s %d"):format(band.receive_mode, band.bandwidth or 0))
  end
end

return {
  transmit = transmit,
  receive = receive,
  poll_frequency = poll_frequency,
  set_frequency = set_frequency,
  change_band = change_band,
}
```

Every entry point is optional. A table without `transmit` keys nothing, which
is what a station running VOX wants, and needs no separate way of saying so.

### Entry Points

| Function | Called | Contract |
| --- | --- | --- |
| `open(ctx)` | After the transports are opened | Failure means rig control failed; nothing else is called |
| `close(ctx)` | Before the transports are given up | Failure is reported only |
| `transmit(ctx)` | Starting a transmission, before any audio | Failure abandons the transmission |
| `receive(ctx)` | After the last sample, plus the tail | Failure is reported; the rig may still be keyed |
| `poll_frequency(ctx)` | Every poll interval, never while keyed | Returns hertz, or nothing when unknown |
| `set_frequency(ctx, hz)` | The operator asked to tune | Failure is reported |
| `change_band(ctx, band)` | The operator chose a band | Receives the band's whole table |

These are the only functions the application calls. A script is free to share
code between them however it likes; `change_band` calling `set_frequency` above
is the script's own arrangement, not a call the application makes twice.

### The Context

`ctx` is a table, rebuilt for each call:

| Field | Holds |
| --- | --- |
| `ctx.ports` | The transports, by the name they were configured under |
| `ctx.band` | The band the rig is on, or `nil` between bands |
| `ctx.frequency` | The last frequency read, in hertz, or `nil` |
| `ctx.log(message)` | Writes to the application log |

Absent fields are `nil` rather than empty: a rig that has not been read has no
frequency, and one between the bands has no band.

`ctx.band` is the same table `change_band` receives, so a script reads a band's
settings the same way whether it was handed one or is acting on the one the rig
is already on.

### Ports

A port of kind `rigctld`:

| Method | Does |
| --- | --- |
| `port:send(line)` | Sends one command, returns its answer as a list of lines |
| `port:frequency()` | Reads the frequency in hertz, addressed for the VFO mode in use |

`send` raises when the rig refuses the command, carrying Hamlib's status
number. A script that wants to go on regardless wraps it in `pcall`.

A port of kind `serial`, which the CI-V and EXTFSK cases need:

| Method | Does |
| --- | --- |
| `port:write(bytes)` | Writes bytes, given as a Lua string |
| `port:read(count, timeout_ms)` | Reads up to `count` bytes |
| `port:set_rts(on)`, `port:set_dtr(on)` | Drives the modem control lines |

### Bounding and Failure

The script runs on the rig worker's thread, and `transmit` runs in front of a
transmission, so it cannot be allowed to run forever. Each call is given a
deadline, checked from an instruction-count hook, and a call that outstays it
is abandoned as a failure.

That bounds computation rather than waiting: time spent inside `port:send`
does not advance the instruction count, so a script that only talks to the rig
is bounded instead by the transport's own timeout, once per command.

`transmit` failing — by raising, by being aborted, or by the rig refusing a
command — abandons the transmission before any audio is sent, and `receive` is
called afterwards regardless. Commands already sent cannot be taken back, so
the rig is put back the only way there is rather than being left keyed.

The script is not a security boundary. It is the operator's own file, at the
same level of trust as `config.toml`, and gets the ordinary Lua standard
library.

## Band Definitions

Bands live in `bands.toml`, beside `config.toml`. A default ships with the
application and is used when the file is absent; it is the band plan the
operator can replace, which is why it is a file of its own rather than a
section of `config.toml`. Band plans are regional and worth swapping whole.

```toml
[[bands]]
name = "40m"
low = 7_000_000
high = 7_300_000
target = 7_171_000
transmit-mode = "LSB"
receive-mode = "LSB"
bandwidth = 3_000
step = 1_000

[[bands]]
name = "20m"
low = 14_000_000
high = 14_350_000
target = 14_230_000
transmit-mode = "USB"
receive-mode = "USB"
bandwidth = 3_000
step = 1_000
monitor-gain = 0.15
```

A list rather than a table of named sections, because the order is the order
the band selector offers them in, and because a name like `1.25m` would have to
be quoted as a key.

`low` and `high` rather than `start` and `end`, because `end` is a reserved
word in Lua and `band["end"]` is a worse thing to have to write than a pair of
names chosen to avoid it.

The application reads `name`, `low`, and `high` — they are what names a
frequency for `${radio.band}` and what decides which band the rig is on — and
`step`, which is what the interface's step buttons move by. Every other key is
the operator's, passed through to the script untouched. `target`,
`transmit-mode`, `receive-mode`, and `bandwidth` are conventions the default
script follows; `monitor-gain` above is one this station invented, and its own
script is what would act on it.

Keys reach Lua with hyphens turned into underscores, because a hyphen cannot
appear in a Lua identifier: `receive-mode` in the file is `band.receive_mode`
in the script.

A band with an extra setting needs no mechanism of its own — it is a key the
script reads.

The plan in use is read at startup. Writing the default out from the menu does
not reload it: a file being edited is not one to act on halfway through, so the
next start is when a changed plan takes effect.

## Configuration

What stays in `config.toml`:

```toml
[rig]
enabled = true
lead-in = 0.2
tail = 0.05
poll-interval = 1.0

[rig.ports.rig]
kind = "rigctld"
address = "127.0.0.1:4532"

[rig.ports.ptt]
kind = "serial"
device = "COM3"
baud = 19200
```

| Key | Meaning |
| --- | --- |
| `enabled` | Whether to connect at all. The Rig Control menu is the same switch. |
| `lead-in` | Seconds between `transmit` returning and the first audio sample. |
| `tail` | Seconds between the last audio sample and `receive` being called. |
| `poll-interval` | Seconds between calls to `poll_frequency`. `0` never calls it. |

`lead-in` covers the time a rig takes to switch to transmit, which its audio
path does not wait for: anything sent inside it is lost. `tail` covers the
opposite end, where the ring buffer is empty but the device has not finished
playing what it was handed.

Each entry under `[rig.ports]` becomes one member of `ctx.ports` under the same
name. The names above are the ones the default script expects, and a station
with only a `rigctld` needs only the first.

A missing `[rig.ports]` section is one port named `rig` on the default address,
so that switching rig control on works for a station running nothing but
`rigctld`. A section that is present is taken as written: a port of a kind this
build cannot open is dropped rather than replaced, because handing back the
default would put the script on a rig the operator did not ask for.

## The rigctld Transport

`rssstv-rig` speaks the `rigctld` text protocol on a plain TCP socket.

Every command is sent in the protocol's extended form, prefixed with `+`.
Without it, a `rigctld` that succeeds at a `get` command answers with the value
alone and no terminator, so how many lines an answer runs to is a fact about
the particular command. The commands a script sends are exactly the ones this
crate has no such fact about. The extended form answers with a terminating
`RPRT <status>` line whatever the command was and whether or not it succeeded,
which makes the end of an answer readable without knowing what was asked. That
it also labels its values, so nothing has to be read by position, is a
convenience on top.

Commands are sent one at a time and each answer is read to its terminator
before the next command is written. Nothing is pipelined, so a sequence of
commands is a sequence of round trips.

On connecting, the session asks `\chk_vfo` to find out whether `rigctld` was
started with `--vfo` and therefore wants every command addressed to a VFO. The
answer is applied to `port:frequency()` and to anything else the transport
sends for itself. What a script passes to `port:send` goes out as written:
which commands take a VFO is a property of the command, and the operator
writing them knows how their own `rigctld` was started better than a guess here
would. A `rigctld` too old to answer the question is one that does not want the
argument either, so a refusal settles as no VFO rather than as a failure.

### One Command per Line

A command must be one command. `port:send` writes exactly one line, so a
newline inside its argument is refused rather than framed as two commands.
What cannot be refused is two commands' worth of words on a single line, such
as `T 1 T 0`.

That reaches `rigctld` as one line and one answer is read back, but the far end
may find a second command in the words left over and answer twice. The extra
`RPRT` then stays in the stream and every later answer is read one behind. The
damage is bounded — a read that never terminates hits the command timeout and
ends the session, and the interface reports a failed rig rather than
transmitting — but an answer read one behind can also be a stale `RPRT 0`,
which is a keying failure mistaken for success. Send one command per call and
the question does not arise.

This is the shape of the risk rather than an observed failure: exactly how
`rigctld` treats the words left over on a line has not been measured here, and
the tests in this crate answer from a stand-in rather than from Hamlib. Nothing
detects the desynchronization if it happens; draining the socket before each
command would, and is not implemented.

## Behavior Around a Transmission

The transports and the Lua state are owned by a worker thread. Keying means a
socket round trip and then the lead-in, and neither belongs in a frame.

1. The interface asks for keying as the transmission is set up, so the lead-in
   runs alongside filling the audio queue rather than after it.
2. The worker calls `transmit`, waits out `lead-in`, and reports that it is
   transmitting.
3. The interface starts the audio device only once the rig has said so. A
   `transmit` that failed stops the transmission instead: a transmission nobody
   hears is worse than one that did not happen.
4. When the queue has drained, the worker waits out `tail` and calls `receive`.

A transmission that is cancelled or that fails unkeys by the same path, as does
switching rig control off while one is running.

`poll_frequency` is not called while the rig is keyed. Reading the frequency
back mid-transmission says nothing the operator cannot see, and it puts CAT
traffic on the wire during the one part of a session that has to be left alone.
`set_frequency` and `change_band` are refused while keyed for the same reason.

While rig control is switched on and not connected, a transmission is refused
with what the rig said. Switching it off transmits anyway.

## Template Variables

`${radio.frequency}` and `${radio.band}` are filled from what
`poll_frequency` returned and the band that frequency falls in. Without a
connection they hold a fixed placeholder, because the transmit tab has to
compose to something before there is a radio to ask and a missing variable
would refuse to render at all. A rig tuned between the bands leaves
`${radio.band}` empty: the frequency beside it is real, and a band that
contradicted it would be worse than none.

A frame that prints either of them stops being true the moment the operator
tunes, so it is composed again when the rig moves — and only then, for the same
reason a frame that prints the clock is composed again on the minute. A
template that says nothing about the frequency is left alone.

## Interface

The Rig Control menu holds the switch, each transport and where it reaches, the
connection's state or the failure that ended it, what the rig is tuned to, and
reconnecting after a failure. It also writes the default script and the default
band plan out for editing, and opens the folder holding them.

Tuning is worth reaching without a menu, so it goes in the window as a radio
panel shared by both tabs:

| Control | Does |
| --- | --- |
| Band selector | Calls `change_band` with the chosen band from `bands.toml` |
| Frequency | Shows what `poll_frequency` last returned |
| Step down, step up | Calls `set_frequency` with the current frequency moved by the band's `step` |

The panel is shown while rig control is switched on rather than only once it is
connected, so that it does not appear and vanish as a connection is made or
lost. Its controls are disabled unless the rig is connected and not keyed — the
same rule that stops the worker polling during a transmission, for the same
reason: moving a rig that is on the air moves the transmission with it.

A step that would leave the band is disabled rather than clamped, as is one on
a band with no `step` and one off the bands entirely. A button that moves
nothing is better than one that says it moved something.

## Staging

1. **Done.** The Lua host, the `rigctld` port, and `open`, `close`,
   `transmit`, `receive`, and `poll_frequency`. Replaced `[rig.commands]` at
   parity with what worked before it.
2. **Done.** `bands.toml`, `change_band`, `set_frequency`, and the radio panel.
   Replaced `[rig.bands]` and the built-in band table, which left `rssstv-rig`
   holding transports and nothing else.
3. The serial port, and with it CI-V keying and DTR/RTS keying.

Each stage stands on its own. The third is what makes the MMSSTV and EXTFSK
cases work, and it needs nothing from the first two but the seam they define.

The keys of the arrangement before the first stage — `[rig] address`,
`[rig.commands]`, and `[rig.bands]` — are removed from the file when it is next
saved, rather than left behind looking like settings that still do something.

## Verification Strategy

The transport tests answer from a stand-in `rigctld` on a loopback socket, so
they run without Hamlib installed. They cover the extended-response framing,
the VFO question, a refused command, and a hangup.

The script host is tested against the same stand-in, with the script under test
written into a temporary configuration directory: that the shipped default keys
and unkeys, that a module exporting nothing still connects and sends nothing,
that the band reaches the script, that a script which raises is reported
without taking the connection down, that one which never returns is abandoned,
and that a chunk returning something other than a table is refused.

What none of it covers is Hamlib itself. The framing assumption — that `+`
makes every answer end in `RPRT` — is the thing to confirm against a real
`rigctld` first.
