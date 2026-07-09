//! n8n Webhook への送信モジュール。
//!
//! `reqwest` の blocking クライアントで WAV ファイルを生バイナリ POST する。
//! シェルを介さないため、旧プロトタイプ（curl 経由）のクォート崩れ問題は起きない。
//! 呼び出しは送信スレッドから行い、結果は `SendResult` で返す。
//!
//! 失敗時は「原因が分かる」ことを重視し、主メッセージ（`message`）と
//! 技術詳細（`detail`、エラーの source チェーン）を分けて保持する。

use std::path::Path;
use std::time::Duration;

/// 送信全体のタイムアウト。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// 失敗時にUIへ見せるレスポンス本文の最大文字数。
const BODY_EXCERPT_LIMIT: usize = 200;
/// 技術詳細（source チェーン）の最大文字数。
const DETAIL_LIMIT: usize = 300;

/// 送信結果。失敗理由を UI に見せられるよう区別して保持する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendResult {
    /// 2xx が返り送信成功。
    Success,
    /// 2xx 以外のステータス。本文の先頭を抜粋して保持する。
    HttpError { status: u16, body_excerpt: String },
    /// 接続エラーやファイル読み込み失敗など、レスポンスを得られなかった場合。
    /// `summary` は原因の要約、`detail` は技術詳細（無ければ空）。
    RequestError { summary: String, detail: String },
}

impl SendResult {
    /// 送信に成功したかどうか。
    pub fn is_success(&self) -> bool {
        matches!(self, SendResult::Success)
    }

    /// UI 表示用の主メッセージ（成功時は空文字）。
    pub fn message(&self) -> String {
        match self {
            SendResult::Success => String::new(),
            SendResult::HttpError { status, .. } => http_status_message(*status),
            SendResult::RequestError { summary, .. } => summary.clone(),
        }
    }

    /// UI 表示用の技術詳細（無ければ空文字）。
    ///
    /// HTTP エラーではレスポンス本文の抜粋、接続エラーでは source チェーン。
    pub fn detail(&self) -> String {
        match self {
            SendResult::Success => String::new(),
            SendResult::HttpError { body_excerpt, .. } => body_excerpt.clone(),
            SendResult::RequestError { detail, .. } => detail.clone(),
        }
    }
}

/// HTTP ステータスから、原因が分かりやすい日本語メッセージを組み立てる。
fn http_status_message(status: u16) -> String {
    match status {
        401 | 403 => {
            format!("認証に失敗しました（HTTP {status}）。ユーザー名／パスワードを確認してください")
        }
        404 => "受け口が見つかりません（HTTP 404）。Webhook URL を確認してください".to_string(),
        500..=599 => format!("サーバ側でエラーが発生しました（HTTP {status}）"),
        _ => format!("サーバがエラーを返しました（HTTP {status}）"),
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
            return SendResult::RequestError {
                summary: "録音ファイル（WAV）の読み込みに失敗しました".to_string(),
                detail: err.to_string(),
            };
        }
    };

    let client = match reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(err) => return classify_reqwest_error(&err),
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
        Err(err) => classify_reqwest_error(&err),
    }
}

/// reqwest のエラーを種類ごとに分類し、要約＋技術詳細を組み立てる。
fn classify_reqwest_error(err: &reqwest::Error) -> SendResult {
    let summary = if err.is_timeout() {
        "タイムアウトしました（30秒以内に応答がありません）"
    } else if err.is_connect() {
        "サーバに接続できません（URL・ネットワーク・DNS を確認してください）"
    } else if err.is_builder() {
        "リクエストの組み立てに失敗しました（Webhook URL が不正の可能性）"
    } else if err.is_request() {
        "リクエストの送信に失敗しました"
    } else {
        "送信中にエラーが発生しました"
    };

    SendResult::RequestError {
        summary: summary.to_string(),
        detail: excerpt(&error_chain(err), DETAIL_LIMIT),
    }
}

/// エラーの `source()` チェーンを辿り、重複を除いて連結する（下層のDNS/TLS等を残すため）。
fn error_chain(err: &reqwest::Error) -> String {
    let mut parts: Vec<String> = vec![err.to_string()];
    let mut source = std::error::Error::source(err);
    while let Some(err) = source {
        let text = err.to_string();
        if !parts.contains(&text) {
            parts.push(text);
        }
        source = err.source();
    }
    parts.join(" / ")
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
    fn http_status_message_は_ステータス別に文言を出し分ける() {
        assert!(http_status_message(401).contains("認証に失敗"));
        assert!(http_status_message(403).contains("認証に失敗"));
        assert!(http_status_message(404).contains("Webhook URL"));
        assert!(http_status_message(503).contains("サーバ側"));
        assert!(http_status_message(418).contains("HTTP 418"));
    }

    #[test]
    fn message_と_detail_は状態ごとに整形される() {
        assert_eq!(SendResult::Success.message(), "");
        assert_eq!(SendResult::Success.detail(), "");

        let http = SendResult::HttpError {
            status: 401,
            body_excerpt: "Unauthorized".to_string(),
        };
        assert!(http.message().contains("認証に失敗"));
        assert_eq!(http.detail(), "Unauthorized");

        let req = SendResult::RequestError {
            summary: "サーバに接続できません".to_string(),
            detail: "dns error / no such host".to_string(),
        };
        assert_eq!(req.message(), "サーバに接続できません");
        assert_eq!(req.detail(), "dns error / no such host");
    }
}
