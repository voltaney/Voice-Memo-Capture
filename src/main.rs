// GUI アプリなので起動時にコンソール窓が点滅しないよう windows サブシステムにする。
#![windows_subsystem = "windows"]

mod audio;
mod config;
mod encoder;
mod sender;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Timer, TimerMode};

use audio::RecordingSession;
use config::Config;

/// UI に出す技術詳細の最大文字数（ウィンドウに収まる長さ）。
const DETAIL_LIMIT: usize = 120;

// build.rs が ui/app.slint から生成したコード（AppWindow / Screen）を取り込む。
slint::include_modules!();

/// 起動時に決まる初期状態。
///
/// 設定読込・デバイス存在チェック・録音開始まではウィンドウ生成前に行い、
/// その結果をこの enum で UI 構築側へ渡す。
enum AppInit {
    /// デバイスが見つかり録音を開始できた。
    Recording {
        config: Config,
        session: RecordingSession,
    },
    /// 指定デバイスが見つからなかった（誤録音防止のため録音しない）。
    DeviceMissing { device_name: String },
    /// 設定読込や録音開始に失敗した回復不能なエラー。
    /// `detail` は原因の技術詳細（無ければ空文字）。
    Fatal { message: String, detail: String },
}

/// 録音セッション中に、コールバック間で共有する状態。
///
/// UI はシングルスレッドなので `Rc<RefCell<...>>` で十分。送信のみ別スレッド。
struct Controller {
    ui: slint::Weak<AppWindow>,
    config: Config,
    /// 録音中のみ `Some`。停止時に `take` して `stop` する。
    session: RefCell<Option<RecordingSession>>,
    started_at: Instant,
}

fn main() -> Result<(), slint::PlatformError> {
    // .env を読み込む。解析エラーがあればメッセージを受け取り、UI に表示する。
    let env_error = load_env();
    // ウィンドウ生成前に、設定読込・デバイスチェック・録音開始まで済ませる。
    let init = build_init(env_error);

    let ui = AppWindow::new()?;
    // 経過秒更新用のタイマー。run() 実行中は生存させる必要があるため main で保持する。
    let timer = Timer::default();

    match init {
        AppInit::Recording { config, session } => {
            ui.set_screen(Screen::Recording);
            ui.set_elapsed("0:00".into());

            let controller = Rc::new(Controller {
                ui: ui.as_weak(),
                config,
                session: RefCell::new(Some(session)),
                started_at: Instant::now(),
            });

            // 経過秒の更新（200ms 周期）。
            {
                let controller = controller.clone();
                timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
                    if let Some(ui) = controller.ui.upgrade() {
                        ui.set_elapsed(format_elapsed(controller.started_at.elapsed()).into());
                    }
                });
            }
            // 送信 / 破棄 / 再送信 / 閉じる。
            {
                let controller = controller.clone();
                ui.on_finish_send(move || finish_and_send(&controller));
            }
            {
                let controller = controller.clone();
                ui.on_discard(move || discard(&controller));
            }
            {
                let controller = controller.clone();
                ui.on_retry(move || spawn_send(&controller));
            }
            ui.on_dismiss(|| {
                let _ = slint::quit_event_loop();
            });
        }
        AppInit::DeviceMissing { device_name } => {
            ui.set_screen(Screen::Blocked);
            ui.set_message(format!("デバイスが見つかりません: {device_name}").into());
            ui.on_dismiss(|| {
                let _ = slint::quit_event_loop();
            });
        }
        AppInit::Fatal { message, detail } => {
            ui.set_screen(Screen::Blocked);
            ui.set_message(message.into());
            ui.set_detail(detail.into());
            ui.on_dismiss(|| {
                let _ = slint::quit_event_loop();
            });
        }
    }

    // ウィンドウを表示 → メインモニタ中央へ移動 → イベントループ。
    ui.show()?;
    center_on_primary_monitor(&ui);
    slint::run_event_loop()?;
    Ok(())
}

/// ウィンドウをメインモニタ（プライマリディスプレイ）の中央へ移動する。
///
/// プライマリモニタの原点は物理座標で (0,0) なので、画面サイズとウィンドウの
/// 物理サイズから中央座標を求めて設定する。`show()` 後に呼ぶこと（サイズが確定する）。
#[cfg(windows)]
fn center_on_primary_monitor(ui: &AppWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    // SAFETY: GetSystemMetrics は副作用のない読み取りのみ。
    let (screen_w, screen_h) =
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    if screen_w <= 0 || screen_h <= 0 {
        return;
    }

    let size = ui.window().size();
    let x = ((screen_w - size.width as i32) / 2).max(0);
    let y = ((screen_h - size.height as i32) / 2).max(0);
    ui.window().set_position(slint::PhysicalPosition::new(x, y));
}

/// Windows 以外では中央配置は行わない（本ツールは Windows 専用）。
#[cfg(not(windows))]
fn center_on_primary_monitor(_ui: &AppWindow) {}

/// 「送信」: 録音を停止し WAV を確定してから送信を開始する。
fn finish_and_send(controller: &Rc<Controller>) {
    if let Some(session) = controller.session.borrow_mut().take()
        && let Err(err) = session.stop()
    {
        if let Some(ui) = controller.ui.upgrade() {
            ui.set_screen(Screen::Failed);
            ui.set_message("録音の確定に失敗しました".into());
            ui.set_detail(err.to_string().into());
        }
        return;
    }
    spawn_send(controller);
}

/// 「破棄」: 録音を停止して（WAV は残すが送らず）ウィンドウを閉じる。
fn discard(controller: &Rc<Controller>) {
    if let Some(session) = controller.session.borrow_mut().take() {
        let _ = session.stop();
    }
    let _ = slint::quit_event_loop();
}

/// 送信スレッドを起動し、画面を「送信中」にする。結果は event loop 経由で UI へ返す。
fn spawn_send(controller: &Rc<Controller>) {
    if let Some(ui) = controller.ui.upgrade() {
        ui.set_screen(Screen::Sending);
        ui.set_message("".into());
        ui.set_detail("".into());
    }

    let url = controller.config.webhook_url.clone();
    let basic_auth = controller.config.basic_auth.clone();
    let wav_path = controller.config.wav_path.clone();
    let ogg_path = controller.config.ogg_path.clone();
    let bitrate_kbps = controller.config.bitrate_kbps;
    let weak = controller.ui.clone();

    std::thread::spawn(move || {
        // 送信前に WAV を Ogg Opus へ変換する（通信量削減）。変換は重い処理があり得るため
        // UI スレッドではなくこの送信スレッドで行う。再送信時は既存 OGG を使い回す。
        let auth = basic_auth
            .as_ref()
            .map(|auth| (auth.user.as_str(), auth.pass.as_str()));
        let result = match ensure_ogg(&wav_path, &ogg_path, bitrate_kbps) {
            Ok(()) => sender::send_ogg(&url, auth, &ogg_path),
            Err(err) => sender::SendResult::RequestError {
                summary: "音声の変換（OGG）に失敗しました".to_string(),
                detail: err.to_string(),
            },
        };
        // UI 更新は UI スレッドで行う（ウィンドウが生存していれば発火）。
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if result.is_success() {
                // 成功時のみ中間 WAV と送信 OGG を削除する（失敗/破棄時は保持）。
                let _ = std::fs::remove_file(&wav_path);
                let _ = std::fs::remove_file(&ogg_path);
                let _ = slint::quit_event_loop();
            } else {
                ui.set_message(result.message().into());
                ui.set_detail(result.detail().into());
                ui.set_screen(Screen::Failed);
            }
        });
    });
}

/// 送信用の OGG を用意する。
///
/// 未生成（初回送信）なら WAV から Opus へ変換する。既に生成済み（再送信）なら
/// スキップし、同じ録音を二重にエンコードしない。起動時に古い OGG は削除済みなので、
/// ここで見える OGG は必ず今回の録音セッションのもの。
fn ensure_ogg(
    wav_path: &Path,
    ogg_path: &Path,
    bitrate_kbps: u32,
) -> Result<(), encoder::EncodeError> {
    if ogg_path.exists() {
        return Ok(());
    }
    encoder::encode_wav_to_ogg_opus(wav_path, ogg_path, bitrate_kbps)
}

/// 経過時間を "m:ss" に整形する。
fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
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
/// 技術詳細（dotenvy のエラー文）として返して UI に見せる。
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
        // 主メッセージは固定文とし、可変長の dotenvy エラー文は技術詳細側へ回す
        // （小さな固定サイズのウィンドウに収めるため）。
        *error = Some(sender::excerpt(&err.to_string(), DETAIL_LIMIT));
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
    if let Some(detail) = env_error {
        return AppInit::Fatal {
            message: ".env の解析に失敗しました（スペースを含む値は \"...\" で囲ってください）"
                .to_string(),
            detail,
        };
    }

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            return AppInit::Fatal {
                message: err.to_string(),
                detail: String::new(),
            };
        }
    };

    // デバイス名が指定されているときだけ、その存在を先に確認する
    // （内蔵マイク等での誤録音を防ぐ安全装置）。未指定なら既定デバイスに任せる。
    if let Some(name) = &config.target_device_name
        && audio::find_input_device(name).is_none()
    {
        return AppInit::DeviceMissing {
            device_name: name.clone(),
        };
    }

    // 前回セッションの古い OGG が残っていると、再送信時の使い回し判定で誤って
    // 今回の録音の代わりに送られてしまう。録音開始前に必ず消しておく。
    let _ = std::fs::remove_file(&config.ogg_path);

    match RecordingSession::start(config.target_device_name.clone(), config.wav_path.clone()) {
        Ok(session) => AppInit::Recording { config, session },
        Err(err) => AppInit::Fatal {
            message: "録音を開始できませんでした".to_string(),
            detail: sender::excerpt(&err.to_string(), DETAIL_LIMIT),
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
