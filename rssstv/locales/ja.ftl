app-title = rssstv

menu-file = ファイル
menu-edit = 編集
menu-view = 表示
menu-settings = 設定
menu-rig = リグコントロール
menu-zoom-in = 拡大
menu-zoom-out = 縮小
menu-zoom-reset = 等倍に戻す ({ $percent }%)
menu-open-config = 設定フォルダーを開く
menu-quit = 終了
menu-language = 言語
menu-help = ヘルプ

tab-receive = 受信
tab-transmit = 送信
tab-history = 履歴

input-device = 入力デバイス
output-device = 出力デバイス

section-rx-status = 受信ステータス

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
qso-record = 記録
qso-clear = クリア

action-lock = ロック
action-resync = 再同期
action-auto-history = 自動履歴
action-save = 保存
action-copy = コピー
action-zoom = 拡大
action-paste = 貼付
action-edit = 編集
action-set-transmit = 送信にセット
action-transmit = TX
action-stop-transmit = TX停止
action-tone = 1750
action-cw = CW
action-fskid = FSKID
action-open-folder = フォルダーを開く
action-refresh = 再読み込み

section-templates = テンプレート
section-stocks = ストック画像
section-composite = 合成プレビュー
library-empty = ファイルなし

badge-waiting = 信号待ち
badge-receiving = RECEIVING · { $mode } · { $percent }%
badge-complete = COMPLETE · { $mode }
badge-transmit-ready = TX READY · { $mode }
badge-transmit-not-ready = TX NOT READY · { $mode }
badge-transmit-preparing = TX PREPARING · { $mode }
badge-transmitting = TRANSMITTING · { $mode } · { $percent }%
badge-transmit-complete = TX COMPLETE · { $mode }
badge-history = HISTORY · { $mode }

status-receiving = 受信中 ({ $percent }%)
status-idle = 待機中
status-transmitting = 送信中 ({ $percent }%)
status-no-audio = 入力デバイスなし
status-no-output = 出力デバイスなし
status-output-ready = 出力準備完了
status-output-audio = 出力 { $rate } Hz
status-dropped = { $samples } サンプル欠落
status-audio = { $rate } Hz / モノラル
status-afc = AFC { $offset } Hz

error-no-transmit-frame = 先に合成画像を送信にセットしてください
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
