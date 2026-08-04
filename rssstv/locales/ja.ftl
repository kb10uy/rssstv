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
menu-transmit = 送信
menu-receive = 受信
menu-history = 受信画像
menu-help = ヘルプ

tab-receive = 受信
tab-transmit = 送信

input-device = 入力デバイス
output-device = 出力デバイス

section-rx-status = 受信ステータス
section-tx-control = 送信コントロール
tx-volume = 送信レベル { $percent }%

section-mode = モード
label-auto-vis = 自動判定 (VIS)
mode-detecting = { $mode }（自動検出中）

section-dsp = DSP
dsp-afc = AFC
dsp-lms = LMS
dsp-slant = 傾き補正

section-qso = QSO
qso-call = コール
qso-station-call = 自局コール
qso-rsv-nr = RSV/NR
qso-clear = クリア

action-auto-history = 自動履歴
history-format-webp = WebP（可逆）
history-format-png = PNG
history-format-jpeg = JPEG
action-send-fskid = FSKID を送出
action-vis-restart = 受信中も VIS を検出して再スタート
action-transmit = TX
action-stop-transmit = TX停止
action-open-folder = フォルダーを開く
action-refresh = 再読み込み

section-templates = テンプレート
section-stocks = ストック画像
library-empty = ファイルなし

badge-waiting = 信号待ち
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

status-receiving = 受信中 ({ $percent }%)
status-idle = 待機中
status-transmitting = 送信中 ({ $row }/{ $total } 行)
status-transmit-leader = 送信中 (同期信号)
status-transmit-identifying = 送信中 (識別信号)
status-no-audio = 入力デバイスなし
status-no-output = 出力デバイスなし
status-output-ready = 出力準備完了
status-output-audio = 出力 { $rate } Hz
status-dropped = { $samples } サンプル欠落
status-audio = { $rate } Hz / モノラル
status-afc = AFC { $offset } Hz

error-no-transmit-frame = 送信画像がまだ合成されていません
error-no-output-device = 出力デバイスを選択してください
error-invalid-station-call = 自局コールが不正です: { $error }

geometry = { $mode } · { $width }×{ $height }

device-lost-title = オーディオデバイスが停止しました
device-lost-disconnected = { $device } が利用できなくなりました。取り外されたか、電源が切れた可能性があります。
device-lost-invalidated = { $device } のストリームを開き直す必要があります。
device-lost-backend = { $device } が停止しました: { $detail }
device-lost-reception-stopped = 受信は停止しています。デバイスを接続し直して再試行するか、設定メニューから別のデバイスを選んでください。
device-lost-retry = 再試行
device-lost-dismiss = 閉じる
