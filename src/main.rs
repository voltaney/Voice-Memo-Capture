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
    // .env を読み込む（存在しなくてもエラーにはしない。環境変数が直接あればそれを使う）。
    let _ = dotenvy::dotenv();

    // ウィンドウ生成前に、設定読込・デバイスチェック・録音開始まで済ませる。
    let init = build_init();

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

/// 設定読込 → デバイス存在チェック → 録音開始 を行い、初期状態を組み立てる。
///
/// - 設定読込に失敗: `Fatal`
/// - デバイス未検出: `DeviceMissing`（誤録音防止のため録音しない）
/// - 録音開始失敗: `Fatal`
/// - すべて成功: `Recording`
fn build_init() -> AppInit {
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
