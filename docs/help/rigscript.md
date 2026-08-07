---
title: リグスクリプト
---

リグコントロールが「いつ」動くかはアプリケーションが決めますが、そのとき
無線機に「何を」送るかは Lua スクリプト `rigcontrol.lua` が決めます。この章は
そのスクリプトと、バンドプラン `bands.toml`、`config.toml` の `[rig]` の
リファレンスです。接続のしかたと画面の操作は
[リグコントロール](rig.md)を参照してください。

どちらのファイルもアプリケーションに内蔵の既定があり、ファイルがなければ
それが使われます。書き換えるには **設定 › リグコントロール** から書き出して
([リグコントロール](rig.md))、テキストエディタで編集してください。変更は
次回の起動から有効です。

## スクリプトの形

スクリプトはモジュールです。関数のテーブルを返します。書き出される既定の
スクリプトは、コメントを除くとこれだけです。

```lua
local function transmit(ctx)
  ctx.ports.rig:send("T 1")
end

local function receive(ctx)
  ctx.ports.rig:send("T 0")
end

local function poll_frequency(ctx)
  return ctx.ports.rig:frequency(), ctx.ports.rig:mode()
end

local function set_frequency(ctx, hz)
  ctx.ports.rig:send(("F %d"):format(hz))
end

local function change_band(ctx, band)
  if band.target then
    set_frequency(ctx, band.target)
  end
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

どの関数も省略できます。`transmit` のないテーブルはキーイングを何もしない、
という意味になります。VOX で運用しているならそれで十分です。

## 呼ばれる関数

アプリケーションが呼ぶのは次の 7 つだけです。それ以外の関数を書いても
呼ばれませんが、スクリプト内で共有するのは自由です。

| 関数 | 呼ばれるとき | 失敗したとき |
| --- | --- | --- |
| `open(ctx)` | 接続が開いた直後 | 接続失敗になり、以後何も呼ばれない |
| `close(ctx)` | 接続を手放す直前 | 報告されるだけ |
| `transmit(ctx)` | 送信の開始時、音声を出す前 | 送信そのものが中止される |
| `receive(ctx)` | 音声を出し終えた後 | 報告される(無線機は送信状態のままかもしれない) |
| `poll_frequency(ctx)` | ポーリング間隔ごと | 報告される |
| `set_frequency(ctx, hz)` | 操作パネルで周波数を動かしたとき | 報告される |
| `change_band(ctx, band)` | 操作パネルでバンドを選んだとき | 報告される |

`poll_frequency` は周波数(Hz)とモードの 2 値を返します。分からないときは
何も返さないでください。送信でキーイングしている間は、`poll_frequency` は
呼ばれず、`set_frequency` と `change_band` の操作は受け付けられません。

`transmit` が失敗する — エラーを起こす、打ち切られる、無線機がコマンドを
拒む — と、音声を出す前に送信が中止され、その後で `receive` が呼ばれて
無線機を受信に戻します。受信状態の無線機に音声だけが流れることはありません。

長く走りすぎた呼び出しは失敗として打ち切られます。打ち切られるのは計算で、
`port:send` の応答待ちは対象外です(そちらは通信自体のタイムアウトで
守られます)。

スクリプトでは Lua の標準ライブラリがそのまま使えます。`config.toml` と
同じく、運用者自身のファイルとして信頼されます。

## コンテキスト

各関数は `ctx` テーブルを受け取ります。

| フィールド | 内容 |
| --- | --- |
| `ctx.ports` | `rigctld` への接続。`[rig.ports]` に書いた名前で引く |
| `ctx.band` | いま乗っているバンドのテーブル。バンド外なら `nil` |
| `ctx.frequency` | 最後に読めた周波数(Hz)。まだなければ `nil` |
| `ctx.log(text)` | アプリケーションのログに書く |

`ctx.band` は `change_band` が受け取るのと同じテーブルなので、バンドの設定は
どの関数からも同じ形で読めます。

## ポート

`ctx.ports` の各ポートは 1 つの `rigctld` です。

| メソッド | 動作 |
| --- | --- |
| `port:send(line)` | コマンドを 1 つ送り、応答を行のリストで返す |
| `port:frequency()` | 周波数を Hz で読む |
| `port:mode()` | 運用モードを読む |

コマンドが拒まれると `send` は Lua のエラーを起こします。失敗しても先へ
進みたいときは `pcall` で包んでください。

`send` に渡すのは必ず 1 コマンドです。`T 1 T 0` のように 1 行に 2 つ書くと、
`rigctld` が 2 回答えることがあり、以後の応答が 1 つずつずれます。改行を
含む文字列は送られる前に拒まれます。

`frequency()` と `mode()` は、`rigctld` が `--vfo` 付きで起動されていても
自動で対応します。`send` に書いたコマンドは書いたとおり送られるので、VFO
引数が要るかどうかは自分の `rigctld` の起動方法に合わせてください。

## バンドプラン

バンドの一覧は `bands.toml` に書きます。並び順がそのまま操作パネルの
バンドセレクターの順になります。

```toml
[[bands]]
name = "40m"
low = 7_000_000
high = 7_300_000
target = 7_178_000
transmit-mode = "PKTLSB"
receive-mode = "LSB"
bandwidth = 3_000
step = 3_000
```

アプリケーション自身が読むキーは 4 つだけです。

| キー | 意味 |
| --- | --- |
| `name` | バンドの名前。セレクターと `${radio.band}` に出る |
| `low`、`high` | バンドの下端と上端(Hz)。どのバンドに乗っているかの判定に使う |
| `step` | 操作パネルの左右ボタンで動かす幅(Hz)。なければボタンは押せない |

それ以外のキーはすべて、そのままスクリプトの `band` テーブルに渡されます。
ハイフンは Lua の識別子に使えないため、アンダースコアに変わります。ファイルの
`receive-mode` はスクリプトでは `band.receive_mode` です。

`target`(バンドに移動したとき合わせる周波数)、`receive-mode`、`bandwidth`
は既定のスクリプトが読む慣例のキーです。`transmit-mode` は既定のバンドプランに
書かれていますが、読むのは自分で書いたスクリプトだけです。アンテナ切り替えや
アンプの設定など、バンドごとに変えたいものは好きなキーを足してスクリプトで
読んでください。

既定のバンドプランの周波数は割り当てではなく慣習です。どこで送信してよいかは
免許の範囲で判断してください。

## config.toml の [rig]

接続先と時間の設定は `config.toml` に書きます。

```toml
[rig]
enabled = true
lead-in = 0.2
tail = 0.05
poll-interval = 1.0

[rig.ports.rig]
address = "127.0.0.1:4532"
```

| キー | 意味 |
| --- | --- |
| `enabled` | 接続するかどうか。**リグ** パネルの接続ボタンと同じスイッチ |
| `lead-in` | `transmit` が終わってから音声を出し始めるまでの秒数(既定 0.2) |
| `tail` | 音声を出し終えてから `receive` を呼ぶまでの秒数(既定 0.05) |
| `poll-interval` | `poll_frequency` を呼ぶ間隔の秒数(既定 1.0)。`0` で呼ばない |

`lead-in` は、無線機が送信に切り替わるまでの時間です。切り替わる前に出た
音声は失われるので、キーイングの遅い無線機では増やしてください。`tail` は
その逆で、最後の音声がデバイスから出きるのを待つ時間です。

`[rig.ports]` の各セクションが、同じ名前で `ctx.ports` のポートになります。
何も書かなければ `rig` という名前のポートが `127.0.0.1:4532` に作られ、
既定のスクリプトはそれを使います。無線機とアンプのように複数の `rigctld` に
つなぐときは、セクションを増やします。

```toml
[rig.ports.rig]
address = "127.0.0.1:4532"

[rig.ports.amplifier]
address = "127.0.0.1:4533"
```
