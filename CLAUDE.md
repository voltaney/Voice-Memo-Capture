# CLAUDE.md

このリポジトリで作業する際の開発ルール。

## プロジェクト概要

都度起動型の Windows 音声メモキャプチャツール（Rust + Slint + cpal + reqwest）。UI は `ui/app.slint`（Slint ソフトウェアレンダラ）。
起動すると即録音が始まり、ボタン一つで任意の Webhook へ音声を送信する（送信先サービスは限定しない）。
詳細な仕様は [docs/spec.md](docs/spec.md) を参照。

## Git 運用

- **機能単位でブランチを切る**: `feature/<機能名>`（例: `feature/audio-capture`）。`main` への直接コミットは避ける。
- **段階的にこまめにコミット**: モジュール（config / audio / sender / app）単位で、1 コミット = 1 つの意味ある変更にまとめる。
- **コミットメッセージは Conventional Commits 風**: `feat:` `fix:` `chore:` `docs:` などの接頭辞を付ける。本文は日本語で可。

## コメント方針

- ソースコード内のコメント・ドキュメンテーションコメント・ログ文言はすべて **日本語** で書く。

## コーディング規約

- コミット前に `cargo fmt` と `cargo clippy` を通す。
- `unwrap()` / `expect()` は UI 初期化など回復不能な箇所に限定する。I/O・録音・送信は `Result` で扱い、エラーを UI に反映する（黙って失敗させない）。
- GUI アプリなので `src/main.rs` の `#![windows_subsystem = "windows"]` は維持する。

## リリース

- **バージョンの正本は `Cargo.toml` の `version`**。exe のプロパティ（詳細タブ）の FileVersion / ProductVersion にも自動で入る。
- 手順: `Cargo.toml` の version を上げてコミット → `git tag vX.Y.Z` → `git push origin main --tags`。
- タグ push で [.github/workflows/release.yml](.github/workflows/release.yml) が走る（fmt / clippy / test → `cargo build --release` → zip を Releases に添付）。タグと `Cargo.toml` の version が食い違うと失敗する。
- exe のプロパティに出る製品名などは `build.rs` の `winresource` で設定している。

## ビルド / 実行

- 開発時の起動確認: `cargo run`
- リリースビルド: `cargo build --release`
- 実行には `.env` が必要（テンプレートは `.env.example`）。`.env` はコミットしない。
