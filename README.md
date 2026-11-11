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
boxes:
  - name: メモ
    source: db
    db: ~/.chira/ura.db
  - name: 下書き
    source: dir
    dir: ~/.chira/md
```

単一の `source`, `db`, `dir` も後方互換で動作します。
`~` は展開されます。`rum.example.yaml` を参照。

## 3ペイン + チラ裏メタファ

- **BOX**: 箱（ノートブック）の一覧。箱 = 読み込み元（SQLite / ディレクトリ）。
- **紙（シート）**: 選択中 BOX の紙一覧。1 枚 = 1 ノート。
- **エディタ**: 紙の本文。1 枚は **たて 24 行固定**（25 行目は入力不可）。溢れたら新しい紙に続きを書く。
- **裏写り**: おもて面の文字が裏からうっすら透ける演出（空行に直前の紙の行が淡く重なる）。既定 ON、`:set ura` / `:set noura` でトグル可能。

## 操作早見表

### モード
- `Normal`: 閲覧・カーソル移動・各種コマンドの受付
- `Insert`: テキスト入力・編集
- `Visual`: 範囲選択（`v`）
- `Command`: `:` でコマンド入力
- `Search`: `/` で検索入力

### ペイン・リスト操作
- `Tab` / `Shift-Tab`: ペイン移動（BOX ↔ 紙 ↔ エディタ）
- `↑` / `↓`（十字）または `j` / `k`: リスト選択 / エディタの行移動
- `n`: フォーカス中ペインで新規作成
  - BOX ペイン → 箱を新規（`:box <name> <db|dir> <path>`）
  - 紙ペイン → 紙を新規
- `Ctrl-n` / `Ctrl-p`（または `]s` / `[s`）: 次の紙 / 前の紙

### 移動（Normal / Visual）
- `h`, `j`, `k`, `l`: 左 / 下 / 上 / 右
- `w`: 次の単語先頭
- `b`: 前の単語先頭
- `0`: 行頭
- `$`: 行末
- `gg`: 先頭行
- `G`: 最終行
- `Ctrl-p`: 前の紙
- `Ctrl-n`: 次の紙

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
- `:new`: 新規紙作成
- `:next` / `:prev`: 次/前の紙を開く
- `:set ura` / `:set noura`: 裏写り演出の有効化 / 無効化
- `:box <name> <db|dir> <path>`: 新規 BOX の追加・設定保存
- `:md [dir]`: 外部コマンド `chira md [dir]` を実行
- `:sync [dir]`: 外部コマンド `chira md import [dir]` を実行

## データの扱い

### DB モード（`source: db`）
- `notes(id, title, content, updatedAt)` を直接読み書きします（chira-ura と同じスキーマ）。
- 保存は `updatedAt = 現在時刻（ミリ秒）` で upsert。`id` が無ければ `note-<timestamp>-<rand4>` を採番。
- サイドバーはノート（新しい順）。`Ctrl-p` / `Ctrl-n` で前後のノート。

### ディレクトリモード（`source: dir`）
- front matter（`id` / `title` / `updatedAt`）を持つ md はフォーマットを維持して保存。
- 保存時、`updatedAt` は現在時刻（ミリ秒）に自動更新。`id` が無ければ採番。
- front matter の無いテキストファイルは素テキストのまま保存。
