app-title = rssstv

menu-file = ファイル
menu-view = 表示
menu-settings = 設定
menu-rig = リグコントロール
menu-zoom-in = 拡大
menu-zoom-out = 縮小
menu-zoom-reset = 等倍に戻す ({ $percent }%)
menu-open-received = 受信画像フォルダーを開く
menu-open-sent = 送信画像フォルダーを開く
menu-open-stocks = ストック画像フォルダーを開く
menu-open-templates = テンプレートフォルダーを開く
menu-open-assets = アセットフォルダーを開く
menu-open-config = 設定フォルダーを開く
menu-quit = 終了
menu-language = 言語
menu-station = 自局情報…
menu-custom-variables = テンプレート変数…
menu-transmit = 送信
menu-receive = 受信
menu-history = 受信画像
menu-help = ヘルプ
menu-manual = マニュアル

tab-receive = 受信
tab-transmit = 送信

input-device = 入力デバイス
output-device = 出力デバイス

tx-volume = 送信レベル { $percent }% ({ $decibels } dB)

section-rx-state = 受信状態
section-tx-level = 送信レベル
section-mode = モード
label-auto-vis = 自動判定 (VIS)

section-dsp = DSP
dsp-afc = AFC
dsp-lms = LMS
dsp-slant = 傾き補正

section-qso = QSO
station-title = 自局情報
station-callsign = コールサイン
station-qth = 運用地
station-grid = グリッド
station-callsign-required = 送信にはコールサインが必要です。
station-close = 閉じる
custom-title = テンプレート変数
custom-note = テンプレートからは ${ "{" }custom.名前{ "}" } で参照します。
custom-invalid = この名前はテンプレートから参照できません。
custom-add = 追加
hint-callsign = Callsign

qso-call = DX CALL
qso-rsv-received = RSV/NR (R)
qso-rsv-nr = RSV/NR (S)
qso-nr-increment = NR Incr.
qso-nr-reset = NR Reset

action-auto-history = 自動履歴
history-format-webp = WebP（可逆）
history-format-png = PNG
history-format-jpeg = JPEG
action-send-fskid = FSKID を送出
action-contest-mode = コンテストモード（ナンバーを送出）
action-vis-restart = 受信中も VIS を検出して再スタート
action-vis-strict = VIS 判定を厳格にする（リーダーを要求）
action-transmit = TX
action-stop-transmit = TX停止
action-tone = { $frequency } Hz のトーンを送信
action-open-folder = フォルダーを開く
action-refresh = 再読み込み
action-rig-connect = 接続
action-rig-disconnect = 切断
action-rig-retry = 接続し直す
action-rig-write-script = デフォルトスクリプトを書き出す
action-rig-write-bands = デフォルトバンドプランを書き出す

rig-state-disconnected = 未接続
rig-state-connecting = 接続中…
rig-state-receiving = 接続済み
rig-state-transmitting = 送信中
rig-state-failed = 接続失敗
rig-frequency-unknown = 周波数未取得
rig-mode-unknown = モード未取得

section-radio = リグ
radio-band-unknown = バンド外
radio-frequency = { $frequency } MHz
rig-script-written = { $path } に書き出しました

section-templates = テンプレート
section-stocks = ストック画像
library-empty = ファイルなし

state-waiting = 信号待ち
state-receiving = RECEIVING · { $percent }%
state-complete = COMPLETE
state-stopped = RX STOPPED
state-rx-muted = 送信中のため受信停止
state-transmit-ready = TX READY
state-transmit-not-ready = TX NOT READY
state-transmit-preparing = TX PREPARING
state-transmit-leader = TRANSMITTING · LEADER
state-transmitting = TRANSMITTING · { $row }/{ $total }
state-transmit-identifying = TRANSMITTING · ID
state-transmit-complete = TX COMPLETE
state-transmit-tone = TRANSMITTING · TONE

status-no-audio = 入力デバイスなし
status-no-output = 出力デバイスなし
status-output-ready = 出力準備完了
status-output-audio = 出力 { $rate } Hz
status-dropped = { $samples } サンプル欠落
status-audio = { $rate } Hz / モノラル

error-no-transmit-frame = 送信画像がまだ合成されていません
error-tone-active = トーン送信中です
error-transmit-active = 送信中です
error-no-output-device = 出力デバイスを選択してください
error-invalid-station-call = 自局コールが不正です: { $error }
error-rig-unavailable = リグコントロールが使用できません: { $error }
error-manual-missing = マニュアルが見つかりません。リリースアーカイブの help フォルダーを実行ファイルと同じ場所に置いてください。

geometry = { $mode } · { $width }×{ $height }

device-lost-title = オーディオデバイスが停止しました
device-lost-disconnected = { $device } が利用できなくなりました。取り外されたか、電源が切れた可能性があります。
device-lost-invalidated = { $device } のストリームを開き直す必要があります。
device-lost-backend = { $device } が停止しました: { $detail }
device-lost-reception-stopped = 受信は停止しています。デバイスを接続し直して再試行するか、設定メニューから別のデバイスを選んでください。
device-lost-retry = 再試行
device-lost-dismiss = 閉じる
