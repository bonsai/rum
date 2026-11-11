# rum — vim 風 TUI リーダ/エディタ

`chira-ura` のメモを vim キーバインドで素早く閲覧・編集・保存する TUI ツール。
読み込み元は設定（YAML）で選ぶ — SQLite を直接（既定 `~/.chira/ura.db`）か、`chira md` が出す md ディレクトリ。
どのパスもアプリにハードコードしない。

## 起動

```bash
# 設定（YAML）の source に従う（既定は db: ~/.chira/ura.db）
rum

# md/txt ディレクトリを開く
rum path/to/notes

# chira-ura の DB を直接開く
rum --db ~/.chira/ura.db

# 設定ファイルを指定
rum --config ./rum.yaml
```

## 設定（rum.yaml）

探索順: `--config <file>` / 環境変数 `RUM_CONFIG` → `./rum.yaml` → `~/.config/rum/config.yaml` → `~/.chira/rum.yaml`

```yaml
source: db              # db（SQLite）| dir（md/txt）
db: ~/.chira/ura.db     # source: db で開く SQLite
dir: ~/.chira/md        # source: dir で開くディレクトリ / :md :sync の既定
```

`~` は展開される。`rum.example.yaml` を参照。

## 読み込み元の切替

- `:db <path>` — SQLite に切替（chira-ura のメモ。`notes` テーブルを読み書き）
- `:dir <path>` — md/txt ディレクトリに切替
- `:md [dir]` / `:sync [dir]` — 外部 `chira md` / `chira md import`（外部 CLI `chira` が必要です: `cargo install chira-ura`）

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
- `:md [dir]`: 外部コマンド `chira md [dir]` を実行（要 `cargo install chira-ura`）
- `:sync [dir]`: 外部コマンド `chira md import [dir]` を実行（要 `cargo install chira-ura`）

## データの扱い

### DB モード（`source: db`）
- `notes(id, title, content, updatedAt)` を直接読み書きします（chira-ura と同じスキーマ）。
- 保存は `updatedAt = 現在時刻（ミリ秒）` で upsert。`id` が無ければ `note-<timestamp>-<rand4>` を採番。
- サイドバーはノート（新しい順）。`Ctrl-p` / `Ctrl-n` で前後のノート。

### ディレクトリモード（`source: dir`）
- front matter（`id` / `title` / `updatedAt`）を持つ md はフォーマットを維持して保存。
- 保存時、`updatedAt` は現在時刻（ミリ秒）に自動更新。`id` が無ければ採番。
- front matter の無いテキストファイルは素テキストのまま保存。
