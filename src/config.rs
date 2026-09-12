//! `.env` から実行時設定を読み込むモジュール。
//!
//! 送信先 Webhook の URL・Basic 認証、録音デバイスの指定、一時ファイルの
//! 固定パス決定など、設定に関する組み立てをここに集約する。
//!
//! 送信先は特定のサービスに依存しない汎用の Webhook を想定する。
//! クエリパラメータ等の受け口固有の事情は Webhook URL 自体に含める運用とし、
//! アプリ側では URL に手を加えない。

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

/// Basic 認証の資格情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicAuth {
    pub user: String,
    pub pass: String,
}

/// `.env` から読み込んだ実行時設定。
#[derive(Debug, Clone)]
pub struct Config {
    /// 送信先 Webhook の URL（`.env` に書かれたものをそのまま使う）。
    pub webhook_url: String,
    /// Basic 認証の資格情報。未設定なら `None`（認証なしで送信する）。
    pub basic_auth: Option<BasicAuth>,
    /// 録音に使う入力デバイス名（部分一致で検索する）。
    /// 未指定なら `None`＝デバイスチェックを行わず、OS の既定の入力デバイスで録音する。
    pub target_device_name: Option<String>,
    /// 録音の中間 WAV ファイルの固定パス（`%TEMP%\voice-memo-capture\capture.wav`）。
    pub wav_path: PathBuf,
    /// 送信する Ogg Opus ファイルの固定パス（`%TEMP%\voice-memo-capture\capture.ogg`）。
    pub ogg_path: PathBuf,
    /// Opus エンコードの目標ビットレート（kbps）。
    pub bitrate_kbps: u32,
}

/// 設定読み込み時のエラー。
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// 必須の環境変数が未設定。
    Missing(&'static str),
    /// Basic 認証のユーザー名／パスワードの片方だけが設定されている。
    IncompleteBasicAuth,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Missing(key) => {
                write!(f, "必須の環境変数が設定されていません: {key}")
            }
            ConfigError::IncompleteBasicAuth => write!(
                f,
                "Basic 認証は BASIC_AUTH_USER と BASIC_AUTH_PASS の両方を設定してください（認証が不要なら両方とも空にしてください）"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// `.env`（および環境変数）から設定を構築する。
    ///
    /// `.env` の読み込み自体は呼び出し側で `dotenvy::dotenv()` を実行しておく前提。
    /// 一時ファイルのパスは `std::env::temp_dir()` を基準に決定する。
    pub fn from_env() -> Result<Self, ConfigError> {
        let webhook_url = required("WEBHOOK_URL")?;
        let basic_auth = basic_auth(optional("BASIC_AUTH_USER"), optional("BASIC_AUTH_PASS"))?;
        let target_device_name = optional("TARGET_DEVICE_NAME");
        let bitrate_kbps = bitrate_from_env();

        let temp_dir = std::env::temp_dir().join(TEMP_SUBDIR);
        let wav_path = temp_dir.join(WAV_FILENAME);
        let ogg_path = temp_dir.join(OGG_FILENAME);

        Ok(Self {
            webhook_url,
            basic_auth,
            target_device_name,
            wav_path,
            ogg_path,
            bitrate_kbps,
        })
    }
}

/// 必須の環境変数を取得する。未設定・空文字なら `ConfigError::Missing` を返す。
fn required(key: &'static str) -> Result<String, ConfigError> {
    optional(key).ok_or(ConfigError::Missing(key))
}

/// 任意の環境変数を取得する。未設定・空文字なら `None`。
fn optional(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

/// ユーザー名／パスワードの組から Basic 認証設定を組み立てる。
///
/// 両方未設定なら認証なし（`None`）。片方だけの設定は設定ミスの可能性が高く、
/// 黙って認証なしで送ると 401 の原因が分かりにくいため、エラーとして扱う。
fn basic_auth(
    user: Option<String>,
    pass: Option<String>,
) -> Result<Option<BasicAuth>, ConfigError> {
    match (user, pass) {
        (Some(user), Some(pass)) => Ok(Some(BasicAuth { user, pass })),
        (None, None) => Ok(None),
        _ => Err(ConfigError::IncompleteBasicAuth),
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
    fn basic認証は_ユーザー名とパスワードが揃っていれば有効になる() {
        let auth = basic_auth(Some("u".to_string()), Some("p".to_string())).unwrap();
        assert_eq!(
            auth,
            Some(BasicAuth {
                user: "u".to_string(),
                pass: "p".to_string(),
            })
        );
    }

    #[test]
    fn basic認証は_両方未設定なら認証なしになる() {
        assert_eq!(basic_auth(None, None).unwrap(), None);
    }

    #[test]
    fn basic認証は_片方だけの設定をエラーにする() {
        assert_eq!(
            basic_auth(Some("u".to_string()), None),
            Err(ConfigError::IncompleteBasicAuth)
        );
        assert_eq!(
            basic_auth(None, Some("p".to_string())),
            Err(ConfigError::IncompleteBasicAuth)
        );
    }
}
