---
title: テンプレート
---

テンプレートは、送信画像に重ねるレイアウトを [KDL](https://kdl.dev/) という
テキスト形式で書いたファイルです。座標をフレームに対する割合で書くので、
1 つのテンプレートが Robot 36 でも PD120 でも、モードの寸法に合わせて
そのまま使えます。実行されるコードは含まれず、書けるのはレイヤーの並びだけです。

この章はテンプレートの書きかたのリファレンスです。テンプレートの選びかたと
合成の流れは[送信](transmit.md)を参照してください。

## ファイルと置き場所

テンプレートは、テンプレートフォルダー([ファイルの場所](files.md))に直接
置いた `.kdl` ファイルです。テキストエディタで編集し、一覧の **再読み込み** で
反映します。

テンプレートから参照する画像は、テンプレートと同じフォルダーか、アセット
フォルダーに置きます。参照はそこへの相対パスだけで、絶対パスや `..` で外の
ファイルを指すテンプレートは読み込み時にエラーになります。画像は PNG・JPEG・
BMP・WebP が読めます。透過には(色ではなく)アルファチャンネルを使って
ください。

フォントはファイルではなく、OS にインストールされているものをファミリー名で
参照します。

同梱の `templates/` に、MMSSTV 付属の 5 つのテンプレートを移植したものが
入っています。書きかたの実例としても使えます。

## 全体の形

ファイルの各トップレベルノードが 1 つのレイヤーで、後に書いたものが上に
重なります。`//` から行末まではコメントです。

```kdl
// 背景の上に、ロゴ・受信画像・宛先・帯を重ねる
image "logo.png" {
    position x=(fw)95 y=(fh)5 anchor="top-right"
    size width=(fw)14 aspect="preserve"
}

rximage {
    position x=(fw)5 y=(fh)22
    size width=(fw)35 height=(fh)45 fit="cover"
}

text "To ${contact.callsign}" {
    position x=(fw)50 y=(fh)8 anchor="top-center"
    font family="Noto Sans" size=(fh)9 weight=700
    fill color="#ffffff"
    stroke color="#182030" width=(em)0.08
}

rect {
    position x=(fw)5 y=(fh)78
    size width=(fw)90 height=(fh)17
    fill color="#101820cc"
}
```

スキーマは厳密です。知らないノードや知らないプロパティ、同じプロパティの
重複は、黙って無視されるのではなくエラーになります。書き間違いが「なぜか
表示されない」ではなくエラーとして見えるためです。

色は `#RRGGBB` か、アルファ付きの `#RRGGBBAA` で書きます。

## 座標と単位

数値には KDL の型注釈で単位を付けます。

| 単位 | 意味 | 例 |
| --- | --- | --- |
| `(fw)` | フレーム幅に対するパーセント | `x=(fw)95` |
| `(fh)` | フレーム高さに対するパーセント | `y=(fh)5` |
| `(em)` | そのテキストのフォントサイズの倍数 | `width=(em)0.08` |

`(fw)100` がフレームの右端、`(fh)100` が下端です。`(em)` はフォントサイズが
決まっている場所(テキストのストローク幅など)でだけ使えます。

位置は負の値も書けるので、レイヤーをフレームの外にはみ出させることが
できます。幅・高さ・フォントサイズ・ストローク幅は負にできず、幅か高さが
0 になるレイヤーはエラーです。

`position` の `anchor` は、そのレイヤーのどの点を `x` `y` に置くかです。
`top-left`(既定)、`top-center`、`top-right`、`center`、`bottom-left`、
`bottom-center`、`bottom-right` が書けます。右端に寄せるものは
`top-right` などで右端から測っておくと、縦横比の違うモードでも配置が
崩れません。

`position` には `rotate` で角度(度、時計回り)も書けます。レイヤーは
アンカーに指定した点を中心に回るので、角に固定したレイヤーは回しても
その角から離れません。

## レイヤー

| レイヤー | 引数 | 必須の子ノード | 任意の子ノード |
| --- | --- | --- | --- |
| `image` | 画像ファイルへの相対パス | `position`、`size` | `clip` |
| `rximage` | なし | `position`、`size` | `clip` |
| `text` | 表示するテキスト | `position`、`font`、`fill` | `stroke` |
| `rect` | なし | `position`、`size`(塗りか線が必要) | `fill`、`stroke` |
| `ellipse` | なし | `position`、`size`(塗りか線が必要) | `fill`、`stroke` |
| `line` | なし | `start`、`end`、`stroke` | なし |
| `group` | なし | レイヤーを 1 つ以上 | `position` |

子ノードの書きかたは次のとおりです。

```kdl
position x=(fw)0 y=(fh)0 anchor="top-left" rotate=-12
size width=(fw)10 height=(fh)10 fit="contain" radius=(fh)2
start x=(fw)0 y=(fh)0
end x=(fw)100 y=(fh)100
font family="Noto Sans" size=(fh)9 weight=700 style="italic" leading=1.2
fill color="#ffffff"
stroke color="#182030" width=(em)0.08
clip shape="circle"
```

`line` は `start` と `end` の 2 点を `stroke` で結びます。2 点には `anchor`
も `rotate` も書けません。

`group` はレイヤーをまとめ、`position` を書くとその分だけ中身全体が
ずれます。中身の座標はグループ内の割合になるのではなく、フレームに対する
割合のままです。グループの `position` に `rotate` を書くと、中身全体が
その点を中心に回ります。

## 画像の合わせかた

`image` と `rximage` の `size` には `fit` で、指定した枠に画像をどう
収めるかを書きます。

| `fit` | 動作 |
| --- | --- |
| `contain`(既定) | 縦横比を保って枠に収める |
| `cover` | 縦横比を保って枠を覆い、はみ出しは切る |
| `stretch` | 縦横比を無視して枠いっぱいに伸ばす |
| `preserve` | 縦横比を保つ(`aspect="preserve"` と同じ) |

`image` は幅か高さの一方を省け、そのときは画像自身の縦横比から求められます。
`rect` と `ellipse` は幅と高さの両方が必要です。

## 角丸と切り抜き

`rect`・`image`・`rximage` の `size` には `radius` で角丸の半径を書けます。
`ellipse` には角がないので書けません。

`image` と `rximage` には `clip` で、箱ではない形に切り抜けます。
`shape="circle"` は枠の中央に収まる最大の円、`shape="ellipse"` は枠に
内接する楕円です。`radius` と `clip` は同時には書けません。

```kdl
rximage {
    position x=(fw)5 y=(fh)22
    size width=(fw)35 height=(fh)35 fit="cover"
    clip shape="circle"
}
```

## テキスト

`font` の `family` と `size` と `weight` は必須です。`weight` は 1〜1000 の
数値で、400 が標準、700 が太字です。`style` は `normal`(既定)か `italic`
です。指定したファミリーが OS に見つからないときはエラーになります。

テキスト中の `\n` は改行です。行は 1 つの `position` と `anchor` を共有し、
`anchor` はブロック全体に効きます。`leading` は行送り(フォントサイズの
倍数、既定 1.2)です。

`stroke` は文字の縁取りです。幅を `(em)` で書いておくと、モードの解像度が
変わっても縁の太さが文字に比例します。縁は文字の後ろに塗られるので、
太くしても文字が痩せません。塗りを透明(`#00000000` など)にして縁だけを
残す書きかたもできます。

```kdl
text "CQ CQ CQ\nde ${station.callsign}\npse K" {
    position x=(fw)5 y=(fh)10 anchor="top-left"
    font family="Noto Sans" size=(fh)8 weight=700 leading=1.4
    fill color="#ffffff"
    stroke color="#182030" width=(em)0.08
}
```

## グラデーション

`rect`・`ellipse`・`text` の `fill` には、単色の代わりにグラデーションを
書けます。

```kdl
rect {
    position x=(fw)0 y=(fh)0
    size width=(fw)100 height=(fh)6
    fill gradient="linear" angle=0 {
        stop offset=0 color="#00ffff"
        stop offset=1 color="#00ff00"
    }
}
```

`gradient` は `linear` か `radial` です。`stop` は 2 つ以上書き、`offset` は
0〜1 で減らないように並べます。色にはアルファも書けるので、フェードアウト
にも使えます。

`linear` の `angle` は度(時計回り)で、0 が左から右、90 が上から下です。
`radial` はレイヤーの中央から広がるので `angle` は書けません。グラデーション
はレイヤー自身の枠に対して塗られます。テキストでは文字列全体にかかり、
行ごとにはやり直しません。`stroke` は単色だけです。

## 変数

テキストには `${...}` で値を差し込めます。

| 変数 | 内容 |
| --- | --- |
| `${station.callsign}` | 自局コールサイン(自局情報ダイアログ) |
| `${station.qth}`、`${station.grid}` | 運用地とグリッド(同上) |
| `${contact.callsign}` | 相手局コールサイン(QSO パネル) |
| `${report.sent}`、`${report.number}` | 送る RSV とシリアルナンバー(同上) |
| `${report.received}` | 受け取った RSV/NR(同上) |
| `${radio.frequency}` | リグの周波数(MHz、数値) |
| `${radio.band}` | 周波数の乗っているバンドの名前 |
| `${tx.timestamp.utc}`、`${tx.timestamp.local}` | 合成した時点の時刻 |
| `${rx.timestamp.utc}`、`${rx.timestamp.local}` | `rximage` の画像を受信した時刻 |
| `${custom.名前}` | **設定 › テンプレート変数…** で決めた値 |
| `${application.version}` | アプリケーションのバージョン |

存在しない変数を参照するとエラーになります。`$${name}` と書くと、差し込み
ではなく `${name}` という文字がそのまま出ます。

コールサイン 2 つは、欄が空のあいだ `Callsign` という語で展開されます。
リグコントロールが未接続のあいだ、`${radio.frequency}` と `${radio.band}` は
固定の値(7.178 と 40m)になります。リグがどのバンドにも乗っていないときは
`${radio.band}` は空です。詳しくは[送信](transmit.md)と
[リグコントロール](rig.md)を参照してください。

### 時刻の書式

時刻の変数は、コロンに続けて `strftime` 系の書式を書けます。

```kdl
text "${tx.timestamp.utc:%d %b %Y %H:%MZ}"
```

書式を書かないときは `%Y-%m-%d %H:%M` です。書式はコロンから閉じ括弧までな
ので、`%H:%M` のようにコロンを含んでもエスケープは要りません。時刻でない
変数に書式を付けるとエラーになります。

各時刻は `.utc` と `.local` の両方で用意されます。テンプレートの側で時差を
計算する必要はありません。

時刻を印字するテンプレートは、分が変わるたびに自動で合成し直されます。

## 受信画像レイヤー

`rximage` は、直前に受信した画像をはめ込むレイヤーです。対象になるのは
最後まで受信できた画像か、途中で切れても 65 % 以上復調できた画像で、
受信画像をディスクに保存しない設定でも使えます。まだ対象がないうちは、
そのモードのテストパターンが出るので、起動直後でも合成は失敗しません。
`${rx.timestamp.*}` はこの画像を受信した時刻です。

## うまくいかないとき

テンプレートが読めない、フォントや画像が見つからない、変数が存在しない
といった場合、送信タブの画像は更新されず、ウィンドウ最下段にエラーが
出ます。送信画像がない状態では TX は押せません。

同梱の `encode-wav` でも、アプリケーションを起動せずにテンプレートを
確認できます(使いかたは同梱の `README.md`)。ただし `encode-wav` が
定義する変数は `${station.callsign}` と `${tx.timestamp.*}` だけなので、
それ以外の変数を使うテンプレートはエラーになります。
