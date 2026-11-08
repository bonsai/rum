# rum — vim 風 TUI リーダ/エディタ

`chira-ura` の markdown ディレクトリ（既定 `~/.chira/md`）を開き、vim キーバインドで素早く閲覧・編集・保存する TUI ツール。
保存は md / txt のみ直接更新し、SQLite DB には触りません（DB への取り込みは `chira md import` / `:sync`）。

## 起動

```bash
# 既定ディレクトリ (~/.chira/md) で起動
rum

# 指定ディレクトリで起動
rum path/to/notes
```

## 操作早見表

### モード
- `Normal`: 閲覧・カーソル移動・各種コマンドの受付
- `Insert`: テキスト入力・編集
- `Visual`: 範囲選択（`v`）
- `Command`: `:` でコマンド入力
- `Search`: `/` で検索入力

### 移動（Normal / Visual）
- `h`, `j`, `k`, `l`: 左 / 下 / 上 / 右
- `w`: 次の単語先頭
- `b`: 前の単語先頭
- `0`: 行頭
- `$`: 行末
- `gg`: 先頭行
- `G`: 最終行
- `Ctrl-p`: 前のファイル
- `Ctrl-n`: 次のファイル

### 編集（Normal）
- `i`: カーソル位置で Insert モード
- `a`: カーソル右で Insert モード
- `I`: 行頭で Insert モード
- `A`: 行末で Insert モード
- `o`: 下に新しい行を挿入して Insert モード
- `O`: 上に新しい行を挿入して Insert モード
- `x`: 1文字削除
- `dd`: 1行削除（切り取り）
- `yy`: 1行ヤンク（コピー）
- `p`: 貼り付け
- `u`: Undo（元に戻す）
- `Ctrl-r`: Redo（やり直し）
- `Ctrl-s`: 上書き保存（クイック保存）

### 検索（Normal）
- `/pattern`: 検索
- `n`: 次の一致へ
- `N`: 前の一致へ

### コマンド（`:` で起動）
- `:w [path]`: 保存（パス省略時は現在のファイル）
- `:q`: 終了（未保存時は警告）
- `:q!`: 変更を破棄して強制終了
- `:wq` / `:x`: 保存して終了
- `:e <path>`: 指定ファイルを開く
- `:e! [path]`: 現在のファイルまたは指定ファイルを再読み込み（変更破棄）
- `:new`: 新規メモ作成
- `:next`: 次のファイルを開く
- `:prev`: 前のファイルを開く
- `:md [dir]`: 外部コマンド `chira md [dir]` を実行
- `:sync [dir]`: 外部コマンド `chira md import [dir]` を実行

## Front Matter の仕様
- front matter（`id` / `title` / `updatedAt`）を持つファイルはフォーマットを維持して保存されます。
- 保存時、`updatedAt` は現在時刻（ミリ秒）に自動更新されます。
- `id` が無い場合は自動採番（`note-<timestamp>-<rand4>`）されます。
- front matter の無いテキストファイルは素テキストのまま保存されます。
