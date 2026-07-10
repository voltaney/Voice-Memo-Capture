# 音声メモキャプチャツール 仕様書

## 1. 目的

個人用の音声ジャーナリング（振り返りシステム）の、PC からのキャプチャ入口となる Windows 常駐しない小型ツール。
グローバルホットキーで起動 → 即録音 → ボタン一つで n8n Webhook へ音声を送信する。

前身の AutoHotkey + SoX プロトタイプでは、HTTP 送信を cmd.exe 経由の curl で行っていたためクォートのエスケープ崩れで送信が失敗し続けた。本ツールは Rust の `reqwest` で HTTP リクエストをシェルを介さず直接組み立てることで、この種の問題を構造的に排除する。

## 2. プロセスモデル

**都度起動型**。常駐・トレイ・トグル状態機械は持たない。

1. タスクバーのショートカット（`.lnk`）またはそのショートカットキーでクリック起動。
2. exe 起動と同時に即録音開始し、小さなウィンドウを最前面に表示。
3. ウィンドウの「終了して送信」ボタンで n8n Webhook へ生バイナリ POST。
4. 2xx が返ればウィンドウを閉じてプロセス終了。失敗時はウィンドウ内にエラーを表示し再送信可能。

状態の寿命は「1 回の録音セッション」= プロセス寿命に一致する。

## 3. 技術スタック

| 用途 | クレート |
| --- | --- |
| ウィンドウUI（retained-mode / ソフトウェアレンダラ） | `slint`（`backend-winit` + `renderer-software`）、ビルド時 `slint-build` |
| 録音・デバイス列挙 | `cpal` |
| WAV 書出し（録音の中間ファイル） | `hound` |
| Opus エンコード | `audiopus`（libopus 同梱ビルド） |
| Ogg コンテナ多重化 | `ogg` |
| HTTP 送信（blocking POST） | `reqwest`（`blocking`, `json`） |
| `.env` 読込 | `dotenvy` |

GUI アプリのため `#![windows_subsystem = "windows"]` を付与し、起動時のコンソール窓の点滅を防ぐ。

UI は Slint の**ソフトウェアレンダラ**を使う。GPU 初期化が不要なため起動が軽い（旧 eframe/wgpu 版の「もっさり」対策）。フォントはシステムフォントを自動使用するため、日本語表示に追加のフォント読み込みは不要。UI マークアップは `ui/app.slint`、`build.rs` の `slint_build::compile` で生成コード化し `slint::include_modules!()` で取り込む。

## 4. スレッド構成

- **メインスレッド（UIスレッド）**: Slint イベントループ（`AppWindow::run()`）。状態は `Screen` プロパティで切替。録音セッション/設定はコールバック間で `Rc<RefCell<...>>` 共有。経過秒は `slint::Timer`（200ms 周期）で更新。
- **録音スレッド**: cpal の `Stream` は `!Send` なので起動直後に spawn し、その中でデバイスを開き・ストリーム構築・保持する。停止シグナル（`Arc<AtomicBool>`）を受けて `hound::WavWriter` を finalize する。
- **送信スレッド**: 「送信」/「再送信」押下時に spawn。`reqwest` blocking で POST し、結果を `slint::Weak::upgrade_in_event_loop` で UI スレッドへ戻して画面遷移する。

## 5. 起動シーケンス

1. `.env` を読込み `Config` を構築。固定パス（録音の中間 `capture.wav` と送信用 `capture.ogg`。ともに `%TEMP%\voice-memo-capture\` 配下）を決定。前回セッションの古い `capture.ogg` は録音開始前に削除する（再送信の使い回し判定で誤送信しないため）。
2. デバイス存在チェック: `cpal::default_host().input_devices()` を列挙し `TARGET_DEVICE_NAME` に部分一致するデバイスを探す。
   - **見つかった**: 録音スレッドを起動し即録音開始。初期状態 = `Recording`。
   - **見つからない**: 録音を開始せず初期状態 = `DeviceMissing`（内蔵マイク等での誤録音を防ぐ安全装置）。
3. ウィンドウ（角丸カードUI）を表示（`show()` → メインモニタ中央へ移動 → `run_event_loop()`）。
   - **最前面固定**: `Window.always-on-top` で他ウィンドウに埋もれないようにする。
   - **中央配置**: `GetSystemMetrics` でプライマリモニタのサイズを取り、物理座標の中央へ `set_position`。

## 6. 状態機械（Slint `Screen`）

UI は `Screen` プロパティの4画面で表現する（egui 版の DeviceMissing / Fatal は「閉じるのみ」で同じ見た目のため `blocked` に統合し、`message` で文言を出し分ける）。

| 画面 | 表示 | ボタン |
| --- | --- | --- |
| `recording` | パルスする赤ドット＋「録音中」＋経過タイマー | `送信` / `破棄` |
| `sending` | 3点スピナー＋「送信中…」 | なし |
| `failed` | 主メッセージ（原因）＋技術詳細（`detail`） | `再送信` / `閉じる` |
| `blocked` | メッセージ（「デバイスが見つかりません: …」等） | `閉じる` |

送信**成功**時は `slint::quit_event_loop()` でウィンドウを閉じてプロセス終了する（成功トーストは出さない）。

### ボタン動作

- **送信**: 録音停止 → WAV 確定 → `sending` へ遷移し送信スレッド起動。送信スレッド内で WAV を Ogg Opus へ変換してから POST する（変換コストが UI を固めないよう別スレッドで実施）。
- **再送信**: 既に変換済みの OGG があればそれを再送信する（録音・再エンコードはし直さない）。
- **破棄**: 録音停止（WAV は残るが送らない）→ ウィンドウを閉じて終了。
- **閉じる**（`blocked` / `failed`）: そのまま終了。

### キーボード操作（マウス移動なし）

起動時に `forward-focus` でキーハンドラ（`FocusScope`）へフォーカスを渡すため、マウスを触らず操作できる。既定アクションのボタンにはフォーカスリングを表示する。

| キー | `recording` | `failed` | `blocked` |
| --- | --- | --- | --- |
| Enter / Space | 送信 | 再送信 | 閉じる |
| Esc | 破棄 | 閉じる | 閉じる |

マウスによるボタンクリックも併用できる。

- 固定パス 2 つ（`%TEMP%\voice-memo-capture\` 配下の `capture.wav`＝録音の中間、`capture.ogg`＝送信用）。起動のたびに上書き。
- 送信**成功**時は WAV・OGG を両方削除。**失敗 / 破棄**時は保持（次回起動で上書き）。→ 失敗しても録音を失わずリトライできる。
- サンプルフォーマット（f32 / i16 / i32）はデバイスに応じて判定し、適切な `WavSpec` で書き出す。チャンネル数・サンプルレートはデバイス既定を使う。

### Ogg Opus への変換（送信前）

- 送信の直前に中間 WAV を Ogg Opus（`audiopus` + `ogg`）へ変換し、それを送る。無圧縮 WAV に対し通信量を桁違いに削減する。
- 目標ビットレートは `.env` の `AUDIO_BITRATE_KBPS`（既定 64、範囲 6〜510 にクランプ）。音声メモ用途なら 64 で実用十分。音質を上げたい場合は 96〜256 などへ。
- Opus は 8/12/16/24/48kHz のみ対応のため、デバイス既定レート（44100 等）は 48kHz へ線形補間でリサンプルする（主にアップサンプリング）。3ch 以上はモノラルへダウンミックスする。
- 変換は送信スレッド内で行い、初回送信でのみ実施。再送信では生成済み OGG を使い回す。

## 8. n8n Webhook への送信仕様

- メソッド: `POST`
- URL: `N8N_WEBHOOK_URL` に `?source=pc` を付与（`.env` には素の URL を書く）。
- 認証: `Authorization: Basic <base64>`（`reqwest` の `.basic_auth(user, Some(pass))` に委譲）。
- `Content-Type: audio/ogg`
- ボディ: Ogg Opus ファイルの生バイナリ。
- 成功判定: HTTP ステータス 2xx。
- 失敗理由は「原因が分かる」よう分類して表示する（`SendResult`）:
  - **HTTP エラー**: ステータス別の日本語メッセージ（401/403=認証、404=URL、5xx=サーバ側）＋レスポンス本文の先頭抜粋を技術詳細に。
  - **接続系エラー**: `reqwest` の `is_timeout` / `is_connect` / `is_builder` / `is_request` で「タイムアウト / 接続不可 / URL不正 / 送信失敗」に分類。技術詳細には `source()` チェーン（DNS/TLS等の下層メッセージ）を連結。
  - UI では主メッセージ（`message`）と技術詳細（`detail`、小さめ表示）を分けて出す。

## 9. 設定（`.env`）

```
N8N_WEBHOOK_URL=https://your-n8n-domain/webhook/xxxxx
N8N_BASIC_AUTH_USER=your_username
N8N_BASIC_AUTH_PASS=your_password
# スペースを含む値は "..." で囲む（dotenvy はクォート無しのスペース入り値を解析エラーにする）
TARGET_DEVICE_NAME="Microphone (USB MICROPHONE)"
# 送信する Ogg Opus の目標ビットレート（kbps）。任意。未設定なら 64。範囲 6〜510。
AUDIO_BITRATE_KBPS=64
```

> **注意**: `TARGET_DEVICE_NAME` のようにスペースを含む値は必ずダブルクォートで囲むこと。
> 囲まないと dotenvy が `.env` の解析に失敗し、その値以降が読み込まれない。
> 解析エラー時はウィンドウにその旨（該当行）を表示する。

`.env` はコミットしない（`.gitignore` に登録）。テンプレートとして `.env.example` のみコミットする。
`.env` の値を書き換えるだけで別 Webhook / 別デバイスへ切り替えられること。

### 読み込み場所

`.lnk` やタスクバーから起動するとカレントディレクトリ（CWD）が不定になるため、
**実行ファイル（`voice-memo-capture.exe`）と同じディレクトリの `.env` を最優先で読む**。
その後、補完として CWD 側の `.env`（`cargo run` 時などはプロジェクトルート）も読む。
dotenvy は既に設定済みの環境変数を上書きしないため、exe 隣の `.env` が優先される。
運用時は **exe と同じフォルダに `.env` を置く** こと。

## 10. ショートカット運用

ビルドした `voice-memo-capture.exe` への Windows ショートカット（`.lnk`）を作成し、プロパティの「ショートカットキー」欄に起動ホットキー（例: Ctrl+Alt+R）を設定、タスクバーにピン留めする。
OS 標準のショートカットキー機能を使うため、アプリ側でグローバルホットキーを実装する必要はない。
補助として `create-shortcut.ps1`（WScript.Shell で `.lnk` を生成し `HotKey` を設定するスクリプト）を同梱してもよい（任意）。

### アプリアイコン

アイコンは exe に埋め込み済み（`build.rs` の `winresource` が `assets/icon.ico` を埋め込む）なので、Explorer・タスクバー・`.lnk` では自動でアプリアイコンが表示される。手動でのアイコン割り当ては不要。実行中ウィンドウのタイトルバー/タスクバーには Slint の `Window.icon`（`assets/icon.png`）が使われる。
アイコンのデザインは `assets/make_icon.py`（Pillow）で再生成できる（赤の角丸タイル＋白いマイク）。

## 11. 非対応事項（スコープ外）

- 常駐・システムトレイ・録音トグル。
- 成功時のトースト通知、ウィンドウアイコン。
- グローバルホットキーのアプリ内実装（`.lnk` のショートカットキー機能で代替）。

## 12. 受け入れ基準

1. `cargo run`（またはビルド済み exe）でウィンドウが表示され、起動と同時に録音が始まる（「録音中…」表示）。
2. 指定デバイス接続状態で「終了して送信」→「送信中…」→ n8n が 2xx を返すとウィンドウが自動で閉じる。
3. 指定デバイス未接続で起動 → 録音されず「デバイスが見つかりません」表示、「閉じる」で終了。
4. URL / パスワード誤りの `.env` で送信 → ウィンドウが閉じず、エラー内容＋「再送信」ボタンが出る。固定パス WAV が保持されていることを確認。
5. 「破棄」ボタン → 送信せずウィンドウが閉じる。
6. `.env` の値だけ書き換えて別 Webhook / 別デバイスに切替できること。

補助検証: 送信部は Basic 認証 / 2xx 制御のため、n8n 実エンドポイントか、`?source=pc`・ヘッダ・ボディをログ出力する簡易 HTTP スタブで確認する。
