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
menu-transmit = Transmit
menu-receive = Reception
menu-history = Received Images
menu-help = Help

tab-receive = Receive
tab-transmit = Transmit

input-device = Input device
output-device = Output device

tx-volume = Transmit level { $percent }% ({ $decibels } dB)

section-rx-level = Receive level
section-tx-level = Transmit level
section-mode = Mode
label-auto-vis = Automatic detection (VIS)
mode-detecting = { $mode } (detecting)

section-dsp = DSP
dsp-afc = AFC
dsp-lms = LMS
dsp-slant = Slant

section-qso = QSO
qso-call = Call
qso-station-call = My call
qso-rsv-nr = RSV/NR
qso-clear = Clear

action-auto-history = Auto history
history-format-webp = WebP (lossless)
history-format-png = PNG
history-format-jpeg = JPEG
action-send-fskid = Send FSKID
action-vis-restart = Restart on a new VIS header
action-transmit = TX
action-stop-transmit = Stop TX
action-open-folder = Open folder
action-refresh = Refresh

section-templates = Templates
section-stocks = Stock images
library-empty = No files

badge-waiting = WAITING FOR SIGNAL
badge-receiving = RECEIVING · { $mode } · { $percent }%
badge-complete = COMPLETE · { $mode }
badge-stopped = RX STOPPED · { $mode }
badge-transmit-ready = TX READY · { $mode }
badge-transmit-not-ready = TX NOT READY · { $mode }
badge-transmit-preparing = TX PREPARING · { $mode }
badge-transmit-leader = TRANSMITTING · { $mode } · LEADER
badge-transmitting = TRANSMITTING · { $mode } · { $row }/{ $total }
badge-transmit-identifying = TRANSMITTING · { $mode } · ID
badge-transmit-complete = TX COMPLETE · { $mode }

status-receiving = Receiving ({ $percent }%)
status-idle = Idle
status-transmitting = Transmitting (line { $row }/{ $total })
status-transmit-leader = Transmitting (leader)
status-transmit-identifying = Transmitting (station ID)
status-no-audio = No input device
status-no-output = No output device
status-output-ready = Output ready
status-output-audio = Output { $rate } Hz
status-dropped = Dropped { $samples } samples
status-audio = { $rate } Hz / mono
status-afc = AFC { $offset } Hz

error-no-transmit-frame = No transmit image has been composed yet
error-no-output-device = Select an output device first
error-invalid-station-call = Invalid station callsign: { $error }

geometry = { $mode } · { $width }×{ $height }

device-lost-title = Audio device stopped
device-lost-disconnected = { $device } is no longer available. It may have been unplugged or switched off.
device-lost-invalidated = The stream on { $device } has to be started again.
device-lost-backend = { $device } stopped: { $detail }
device-lost-reception-stopped = Reception has stopped. Reconnect the device and retry, or choose another from the Settings menu.
device-lost-retry = Retry
device-lost-dismiss = Close
