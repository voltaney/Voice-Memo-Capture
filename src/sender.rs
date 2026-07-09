//! n8n Webhook への送信モジュール。
//!
//! `reqwest` の blocking クライアントで WAV ファイルを生バイナリ POST する。
//! シェルを介さないため、旧プロトタイプ（curl 経由）のクォート崩れ問題は起きない。
//! 呼び出しは送信スレッドから行い、結果は `SendResult` で返す。

use std::path::Path;
use std::time::Duration;

/// 送信全体のタイムアウト。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// 失敗時にUIへ見せるレスポンス本文の最大文字数。
const BODY_EXCERPT_LIMIT: usize = 200;

/// 送信結果。失敗理由を UI に見せられるよう区別して保持する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendResult {
    /// 2xx が返り送信成功。
    Success,
    /// 2xx 以外のステータス。本文の先頭を抜粋して保持する。
    HttpError { status: u16, body_excerpt: String },
    /// 接続エラーやファイル読み込み失敗など、レスポンスを得られなかった場合。
    RequestError(String),
}

impl SendResult {
    /// 送信に成功したかどうか。
    pub fn is_success(&self) -> bool {
        matches!(self, SendResult::Success)
    }

    /// UI 表示用のエラーメッセージ（成功時は空文字）。
    pub fn error_message(&self) -> String {
        match self {
            SendResult::Success => String::new(),
            SendResult::HttpError {
                status,
                body_excerpt,
            } => {
                if body_excerpt.is_empty() {
                    format!("送信に失敗しました（HTTP {status}）")
                } else {
                    format!("送信に失敗しました（HTTP {status}）: {body_excerpt}")
                }
            }
            SendResult::RequestError(msg) => format!("送信に失敗しました: {msg}"),
        }
    }
}

/// `wav_path` の WAV を n8n Webhook へ POST する。
///
/// - 認証: Basic 認証（`user` / `pass`）
/// - ヘッダ: `Content-Type: audio/wav`
/// - ボディ: WAV ファイルの生バイナリ
///
/// `url` には `?source=pc` を付与済みのものを渡す前提。
pub fn send_wav(url: &str, user: &str, pass: &str, wav_path: &Path) -> SendResult {
    let bytes = match std::fs::read(wav_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return SendResult::RequestError(format!("WAVファイルの読み込みに失敗しました: {err}"));
        }
    };

    let client = match reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(err) => return SendResult::RequestError(err.to_string()),
    };

    let response = client
        .post(url)
        .basic_auth(user, Some(pass))
        .header(reqwest::header::CONTENT_TYPE, "audio/wav")
        .body(bytes)
        .send();

    match response {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                SendResult::Success
            } else {
                let code = status.as_u16();
                let body = response.text().unwrap_or_default();
                SendResult::HttpError {
                    status: code,
                    body_excerpt: excerpt(&body, BODY_EXCERPT_LIMIT),
                }
            }
        }
        Err(err) => SendResult::RequestError(err.to_string()),
    }
}

/// 文字列の先頭から最大 `limit` 文字を抜き出す（文字境界を尊重）。
fn excerpt(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    trimmed.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_は_先頭から指定文字数を返す() {
        assert_eq!(excerpt("abcdef", 3), "abc");
        assert_eq!(excerpt("abc", 10), "abc");
        assert_eq!(excerpt("  hello  ", 10), "hello");
    }

    #[test]
    fn excerpt_はマルチバイト境界を壊さない() {
        // 3文字＝「あいう」。バイト数ではなく文字数で切ること。
        assert_eq!(excerpt("あいうえお", 3), "あいう");
    }

    #[test]
    fn error_message_は状態ごとに整形される() {
        assert_eq!(SendResult::Success.error_message(), "");
        assert_eq!(
            SendResult::HttpError {
                status: 401,
                body_excerpt: "Unauthorized".to_string(),
            }
            .error_message(),
            "送信に失敗しました（HTTP 401）: Unauthorized"
        );
        assert_eq!(
            SendResult::RequestError("接続できません".to_string()).error_message(),
            "送信に失敗しました: 接続できません"
        );
    }
}
