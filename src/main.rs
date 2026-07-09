// GUI アプリなので起動時にコンソール窓が点滅しないよう windows サブシステムにする。
#![windows_subsystem = "windows"]

mod app;
mod audio;
mod config;
mod sender;

use eframe::egui;

use app::{App, AppInit};
use audio::RecordingSession;
use config::Config;

fn main() -> eframe::Result {
    // .env を読み込む。解析エラーがあればメッセージを受け取り、UI に表示する。
    let env_error = load_env();

    // ウィンドウ生成前に、設定読込・デバイスチェック・録音開始まで済ませる。
    let init = build_init(env_error);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("音声メモ")
            .with_inner_size([360.0, 170.0])
            .with_min_inner_size([320.0, 150.0])
            .with_resizable(false)
            .with_always_on_top(),
        ..Default::default()
    };

    eframe::run_native(
        "音声メモ",
        native_options,
        Box::new(|cc| Ok(Box::new(App::new(cc, init)))),
    )
}

/// `.env` を読み込む。
///
/// `.lnk`/タスクバー起動では CWD が不定なため、まず実行ファイルと同じ
/// ディレクトリの `.env` を最優先で読む。dotenvy は既存の変数を上書きしないので、
/// その後の `dotenv()`（CWD 側。`cargo run` 時など）は不足分の補完に留まる。
///
/// ファイルが無いのは正常（環境変数を直接使う運用もあり得る）なので無視するが、
/// 解析エラー（例: スペースを含む値がクォートされていない）は黙殺すると
/// 「必須の環境変数が未設定」という分かりにくい形で表面化するため、
/// メッセージとして返して UI に見せる。
fn load_env() -> Option<String> {
    let mut error = None;

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        record_env_error(dotenvy::from_path(dir.join(".env")).map(|_| ()), &mut error);
    }
    record_env_error(dotenvy::dotenv().map(|_| ()), &mut error);

    error
}

/// dotenvy の結果を検査し、解析エラーのみ `error` に記録する（ファイル無しは無視）。
fn record_env_error(result: Result<(), dotenvy::Error>, error: &mut Option<String>) {
    if let Err(err) = result
        && !err.not_found()
        && error.is_none()
    {
        *error = Some(format!(
            ".env の解析に失敗しました（スペースを含む値は \"...\" で囲ってください）: {err}"
        ));
    }
}

/// 設定読込 → デバイス存在チェック → 録音開始 を行い、初期状態を組み立てる。
///
/// - `.env` 解析エラー / 設定読込に失敗: `Fatal`
/// - デバイス未検出: `DeviceMissing`（誤録音防止のため録音しない）
/// - 録音開始失敗: `Fatal`
/// - すべて成功: `Recording`
fn build_init(env_error: Option<String>) -> AppInit {
    // .env の解析エラーがあれば、最優先で表示する。
    if let Some(message) = env_error {
        return AppInit::Fatal { message };
    }

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            return AppInit::Fatal {
                message: err.to_string(),
            };
        }
    };

    // 指定デバイスの存在を先に確認する（内蔵マイク等での誤録音を防ぐ安全装置）。
    if audio::find_input_device(&config.target_device_name).is_none() {
        return AppInit::DeviceMissing {
            device_name: config.target_device_name.clone(),
        };
    }

    match RecordingSession::start(config.target_device_name.clone(), config.wav_path.clone()) {
        Ok(session) => AppInit::Recording { config, session },
        Err(err) => AppInit::Fatal {
            message: format!("録音を開始できませんでした: {err}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    /// 一時ディレクトリに `.env` を書き出す。
    fn write_temp_env(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn dotenvy_はクォート無しのスペース入り値を解析エラーにする() {
        // 今回のバグの根本原因: スペースを含む値をクォートしないと解析エラーになる。
        let path = write_temp_env(
            "vmc_test_unquoted.env",
            "VMC_TEST_UNQUOTED=Microphone (USB)\n",
        );
        let result = dotenvy::from_path(&path);
        let _ = std::fs::remove_file(&path);
        assert!(
            result.is_err(),
            "クォート無しのスペース入り値は解析エラーになるはず"
        );
    }

    #[test]
    fn dotenvy_はクォート付きのスペース入り値を読める() {
        let path = write_temp_env(
            "vmc_test_quoted.env",
            "VMC_TEST_QUOTED=\"Microphone (USB)\"\n",
        );
        let result = dotenvy::from_path(&path);
        let value = std::env::var("VMC_TEST_QUOTED");
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok(), "クォート付きなら読めるはず: {result:?}");
        assert_eq!(value.unwrap(), "Microphone (USB)");
    }
}
