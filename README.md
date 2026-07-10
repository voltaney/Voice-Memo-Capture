# voice-memo-capture

都度起動型の Windows 音声メモキャプチャツール。グローバルホットキー（`.lnk` のショートカットキー）で起動 → 即録音 → ボタン一つで録音を **Ogg Opus** に変換し n8n Webhook へ POST する。個人用の音声ジャーナリングの PC 側キャプチャ入口。

- 常駐しない（1 回の録音 = 1 プロセス）。
- Rust + Slint（ソフトウェアレンダラ）+ cpal + reqwest。
- 送信は Ogg Opus（`audio/ogg`）。無圧縮 WAV より通信量を大幅削減。

## 必要環境

- Windows / Rust（MSVC ツールチェーン）
- **cmake**（`audiopus` が libopus を同梱ビルドするため必須）

## ビルド / 実行

```sh
cargo run              # 開発時の起動確認
cargo build --release  # リリースビルド（target/release/voice-memo-capture.exe）
```

実行には exe と同じフォルダ（`cargo run` 時はプロジェクトルート）に `.env` が必要。テンプレートは [.env.example](.env.example) を参照。

## 設定（`.env`）

```
N8N_WEBHOOK_URL=https://your-n8n-domain/webhook/xxxxx
N8N_BASIC_AUTH_USER=your_username
N8N_BASIC_AUTH_PASS=your_password
# スペースを含む値は必ず "..." で囲む
TARGET_DEVICE_NAME="Microphone (USB MICROPHONE)"
# 送信 Ogg Opus のビットレート(kbps)。任意。未設定なら 64。範囲 6〜510。
AUDIO_BITRATE_KBPS=64
```

`.env` はコミットしない。値を書き換えるだけで別 Webhook / 別デバイス / 別ビットレートへ切り替えられる。

## 使い方

1. ビルドした exe への Windows ショートカット（`.lnk`）を作り、プロパティの「ショートカットキー」に起動ホットキー（例: Ctrl+Alt+R）を設定してタスクバーにピン留めする。
2. ホットキーで起動すると即録音が始まる。
3. **Enter / Space**（またはボタン）で送信、**Esc** で破棄。送信成功でウィンドウは自動で閉じる。失敗時はエラー表示＋再送信。

指定デバイスが未接続のときは録音せず「デバイスが見つかりません」と表示する（内蔵マイク等での誤録音防止）。

## ドキュメント

詳細な仕様は [docs/spec.md](docs/spec.md)、開発ルールは [CLAUDE.md](CLAUDE.md) を参照。
