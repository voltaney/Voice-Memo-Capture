//! `.env` から実行時設定を読み込むモジュール。
//!
//! Webhook URL への `?source=pc` 付与や、一時 WAV の固定パス決定など、
//! 設定に関する組み立てをここに集約する。

use std::path::PathBuf;

/// 一時ファイルを置くサブディレクトリ名（`%TEMP%` 配下）。
const TEMP_SUBDIR: &str = "voice-memo-capture";
/// 一時 WAV ファイル名（録音の中間ファイル）。起動のたびに上書きする固定名。
const WAV_FILENAME: &str = "capture.wav";
/// 送信する Ogg Opus ファイル名。WAV から変換して作る固定名。
const OGG_FILENAME: &str = "capture.ogg";
/// ビットレート未設定時の既定値（kbps）。音声メモ用途では 64kbps で実用十分。
const DEFAULT_BITRATE_KBPS: u32 = 64;
/// ビットレートの許容範囲（kbps）。Opus の実用域に収める。
const BITRATE_RANGE_KBPS: std::ops::RangeInclusive<u32> = 6..=510;

/// `.env` から読み込んだ実行時設定。
#[derive(Debug, Clone)]
pub struct Config {
    /// n8n Webhook の素の URL（`?source=pc` は未付与）。
    webhook_url: String,
    /// Basic 認証のユーザー名。
    pub basic_user: String,
    /// Basic 認証のパスワード。
    pub basic_pass: String,
    /// 録音に使う入力デバイス名（部分一致で検索する）。
    pub target_device_name: String,
    /// 録音の中間 WAV ファイルの固定パス（`%TEMP%\voice-memo-capture\capture.wav`）。
    pub wav_path: PathBuf,
    /// 送信する Ogg Opus ファイルの固定パス（`%TEMP%\voice-memo-capture\capture.ogg`）。
    pub ogg_path: PathBuf,
    /// Opus エンコードの目標ビットレート（kbps）。
    pub bitrate_kbps: u32,
}

/// 設定読み込み時のエラー。
#[derive(Debug)]
pub enum ConfigError {
    /// 必須の環境変数が未設定。
    Missing(&'static str),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Missing(key) => {
                write!(f, "必須の環境変数が設定されていません: {key}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// `.env`（および環境変数）から設定を構築する。
    ///
    /// `.env` の読み込み自体は呼び出し側で `dotenvy::dotenv()` を実行しておく前提。
    /// 一時 WAV パスは `std::env::temp_dir()` を基準に決定する。
    pub fn from_env() -> Result<Self, ConfigError> {
        let webhook_url = required("N8N_WEBHOOK_URL")?;
        let basic_user = required("N8N_BASIC_AUTH_USER")?;
        let basic_pass = required("N8N_BASIC_AUTH_PASS")?;
        let target_device_name = required("TARGET_DEVICE_NAME")?;
        let bitrate_kbps = bitrate_from_env();

        let temp_dir = std::env::temp_dir().join(TEMP_SUBDIR);
        let wav_path = temp_dir.join(WAV_FILENAME);
        let ogg_path = temp_dir.join(OGG_FILENAME);

        Ok(Self {
            webhook_url,
            basic_user,
            basic_pass,
            target_device_name,
            wav_path,
            ogg_path,
            bitrate_kbps,
        })
    }

    /// 送信先 URL に `?source=pc` を付与して返す。
    ///
    /// 素の URL に既存のクエリがある場合は `&source=pc` として連結する。
    pub fn webhook_url_with_source(&self) -> String {
        let separator = if self.webhook_url.contains('?') {
            '&'
        } else {
            '?'
        };
        format!("{}{}source=pc", self.webhook_url, separator)
    }
}

/// 必須の環境変数を取得する。未設定・空文字なら `ConfigError::Missing` を返す。
fn required(key: &'static str) -> Result<String, ConfigError> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::Missing(key)),
    }
}

/// `AUDIO_BITRATE_KBPS` を読み、Opus の実用域へ丸めて返す。
///
/// 未設定・空文字・数値でない場合は既定値（64kbps）。範囲外は範囲内へクランプする。
/// ビットレートは任意設定なので、不正値でも起動を止めず妥当な値へ寄せる。
fn bitrate_from_env() -> u32 {
    std::env::var("AUDIO_BITRATE_KBPS")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .map(|kbps| kbps.clamp(*BITRATE_RANGE_KBPS.start(), *BITRATE_RANGE_KBPS.end()))
        .unwrap_or(DEFAULT_BITRATE_KBPS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_pc_は_クエリ無しurl_に_疑問符付きで付与される() {
        let config = Config {
            webhook_url: "https://example.com/webhook/abc".to_string(),
            basic_user: "u".to_string(),
            basic_pass: "p".to_string(),
            target_device_name: "mic".to_string(),
            wav_path: PathBuf::from("capture.wav"),
            ogg_path: PathBuf::from("capture.ogg"),
            bitrate_kbps: 256,
        };
        assert_eq!(
            config.webhook_url_with_source(),
            "https://example.com/webhook/abc?source=pc"
        );
    }

    #[test]
    fn source_pc_は_既存クエリ付きurl_に_アンパサンドで付与される() {
        let config = Config {
            webhook_url: "https://example.com/webhook/abc?foo=bar".to_string(),
            basic_user: "u".to_string(),
            basic_pass: "p".to_string(),
            target_device_name: "mic".to_string(),
            wav_path: PathBuf::from("capture.wav"),
            ogg_path: PathBuf::from("capture.ogg"),
            bitrate_kbps: 256,
        };
        assert_eq!(
            config.webhook_url_with_source(),
            "https://example.com/webhook/abc?foo=bar&source=pc"
        );
    }
}
