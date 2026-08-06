---
title: ファイルの場所
---

設定や画像は、OS ごとの標準的なユーザーディレクトリに置かれます。実行ファイルの
隣には何も作られないので、アーカイブを展開し直しても設定は残ります。逆に、USB
メモリーに入れて持ち歩くような使いかたはできません。

## フォルダー

| 内容 | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 設定 | `%APPDATA%\RSSSTV\config.toml` | `~/Library/Application Support/RSSSTV/config.toml` | `$XDG_CONFIG_HOME/rssstv/config.toml` |
| テンプレート | `%APPDATA%\RSSSTV\templates` | `~/Library/Application Support/RSSSTV/templates` | `$XDG_DATA_HOME/rssstv/templates` |
| アセット | `%APPDATA%\RSSSTV\assets` | `~/Library/Application Support/RSSSTV/assets` | `$XDG_DATA_HOME/rssstv/assets` |
| 画像 | `ピクチャ\RSSSTV` | `~/Pictures/RSSSTV` | `$XDG_PICTURES_DIR/RSSSTV` |

画像フォルダーの下に `Stocks`(送信用の背景)、`Sent`(送信した画像)、
`Received`(受信した画像)があります。年月などの下位フォルダーは作られません。

これらは起動時にすべて作られます。`config.toml` がなければ空のものが作られ、
すでにあるものが置き換えられることはありません。

**ファイル** メニューから、それぞれのフォルダーをファイルマネージャーで開けます。
テンプレートとストック画像の一覧にある **フォルダーを開く** も同じものです。

## テンプレートとアセット

テンプレートは `templates` フォルダーに直接置いた `.kdl` ファイルです。下位
フォルダーの中は見ません。テンプレートから参照する画像やフォントなどは `assets`
に置きます。参照できるのはこれらのフォルダーの中だけです。`..` や絶対パスで
外のファイルを指すテンプレートは、読み込み時にエラーになります。

## 設定ファイル

`config.toml` には、言語、表示倍率、選んだデバイス、テンプレートとストックの選択、
モード、DSP の状態、受信画像の保存設定、自局コールサイン、コンテストモード、
QSO パネルのシリアルナンバーが保存されます。アプリケーションが書き戻すのは
これらのキーだけで、コメントや知らないキーはそのまま残ります。

`[variables]` テーブルは、テンプレートから `${custom.名前}` で読む値です。
**設定 › テンプレート変数…** で編集するものと同じです。`${...}` に書けない名前は
読み込み時に捨てられます。

`[rig]` にはリグコントロールの接続先や送受切り替えの待ち時間が入ります。
`rigcontrol.lua` と `bands.toml` は `config.toml` と同じフォルダーに置きます
([リグコントロール](rig.md))。

## ログ

診断ログはファイルに追記されます。置き場所は設定とは別で、Linux では
`$XDG_STATE_HOME`、Windows と macOS ではローカル(同期されない)データ
ディレクトリです。1 メガバイトを超えると切り替わり、1 世代前まで残ります。

不具合の報告にはこのファイルを添えてください。
