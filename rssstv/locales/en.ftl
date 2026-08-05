app-title = rssstv

menu-file = File
menu-view = View
menu-settings = Settings
menu-rig = Rig Control
menu-zoom-in = Zoom In
menu-zoom-out = Zoom Out
menu-zoom-reset = Reset Zoom ({ $percent }%)
menu-open-received = Open Received Folder
menu-open-sent = Open Sent Folder
menu-open-stocks = Open Stocks Folder
menu-open-templates = Open Templates Folder
menu-open-assets = Open Assets Folder
menu-open-config = Open Config Folder
menu-quit = Quit
menu-language = Language
menu-station = Station…
menu-custom-variables = Template Variables…
menu-transmit = Transmit
menu-receive = Reception
menu-history = Received Images
menu-help = Help
menu-manual = Manual

tab-receive = Receive
tab-transmit = Transmit

input-device = Input device
output-device = Output device

tx-volume = Transmit level { $percent }% ({ $decibels } dB)

section-rx-level = Receive level
section-tx-level = Transmit level
section-mode = Mode
label-auto-vis = Automatic detection (VIS)

section-dsp = DSP
dsp-afc = AFC
dsp-lms = LMS
dsp-slant = Slant

section-qso = QSO
station-title = Station
station-callsign = Callsign
station-qth = QTH
station-grid = Grid
station-callsign-required = A callsign is required to transmit.
station-close = Close
custom-title = Template Variables
custom-note = A template reads these as ${ "{" }custom.name{ "}" }.
custom-invalid = A template cannot reference this name.
custom-add = Add
qso-call = DX CALL
qso-rsv-received = RSV/NR (R)
qso-rsv-nr = RSV/NR (S)
qso-nr-increment = NR Incr.
qso-nr-reset = NR Reset

action-auto-history = Auto history
history-format-webp = WebP (lossless)
history-format-png = PNG
history-format-jpeg = JPEG
action-send-fskid = Send FSKID
action-contest-mode = Contest mode (send number)
action-vis-restart = Restart on a new VIS header
action-transmit = TX
action-stop-transmit = Stop TX
action-tone = Send a { $frequency } Hz tone
action-open-folder = Open folder
action-refresh = Refresh
action-rig-connect = Connect
action-rig-disconnect = Disconnect
action-rig-retry = Reconnect
action-rig-write-script = Write the default script
action-rig-write-bands = Write the default band plan

rig-state-disconnected = Not connected
rig-state-connecting = Connecting…
rig-state-receiving = Connected
rig-state-transmitting = Keyed
rig-state-failed = Failed
rig-frequency-unknown = Frequency not read
rig-mode-unknown = Mode not read

section-radio = Radio
radio-band-unknown = Off band
radio-frequency = { $frequency } MHz
rig-script-written = Written to { $path }

section-templates = Templates
section-stocks = Stock images
library-empty = No files

state-waiting = WAITING FOR SIGNAL
state-receiving = RECEIVING · { $percent }%
state-complete = COMPLETE
state-stopped = RX STOPPED
state-transmit-ready = TX READY
state-transmit-not-ready = TX NOT READY
state-transmit-preparing = TX PREPARING
state-transmit-leader = TRANSMITTING · LEADER
state-transmitting = TRANSMITTING · { $row }/{ $total }
state-transmit-identifying = TRANSMITTING · ID
state-transmit-complete = TX COMPLETE
state-transmit-tone = TRANSMITTING · TONE

status-no-audio = No input device
status-no-output = No output device
status-output-ready = Output ready
status-output-audio = Output { $rate } Hz
status-dropped = Dropped { $samples } samples
status-audio = { $rate } Hz / mono

error-no-transmit-frame = No transmit image has been composed yet
error-tone-active = A tone is being sent
error-transmit-active = A picture is being sent
error-no-output-device = Select an output device first
error-invalid-station-call = Invalid station callsign: { $error }
error-rig-unavailable = Rig control is not ready: { $error }
error-manual-missing = The manual was not found. It is the help folder the release archive puts beside the executable.

geometry = { $mode } · { $width }×{ $height }

device-lost-title = Audio device stopped
device-lost-disconnected = { $device } is no longer available. It may have been unplugged or switched off.
device-lost-invalidated = The stream on { $device } has to be started again.
device-lost-backend = { $device } stopped: { $detail }
device-lost-reception-stopped = Reception has stopped. Reconnect the device and retry, or choose another from the Settings menu.
device-lost-retry = Retry
device-lost-dismiss = Close
