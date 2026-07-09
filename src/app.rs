//! egui による UI と状態機械。
//!
//! 都度起動なので状態の寿命は「1 回の録音セッション」= プロセス寿命に一致する。
//! 送信は別スレッドで行い、結果を `mpsc` チャネル経由でこのスレッドへ返す。

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::audio::RecordingSession;
use crate::config::Config;
use crate::sender::{self, SendResult};

/// 送信中の結果ポーリング間隔。
const SEND_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// 録音中の経過秒表示を更新する間隔。
const RECORDING_REPAINT_INTERVAL: Duration = Duration::from_millis(200);

/// 起動時に main が組み立てる初期状態。
///
/// 設定読込・デバイス存在チェック・録音開始まではウィンドウ生成前に行い、
/// その結果をこの enum で `App` へ渡す。
pub enum AppInit {
    /// デバイスが見つかり録音を開始できた。
    Recording {
        config: Config,
        session: RecordingSession,
    },
    /// 指定デバイスが見つからなかった（誤録音防止のため録音しない）。
    DeviceMissing { device_name: String },
    /// 設定読込や録音開始に失敗した回復不能なエラー。
    Fatal { message: String },
}

/// UI の状態。
enum State {
    /// 録音中。`started_at` から経過秒を算出して表示する。
    Recording { started_at: Instant },
    /// 送信中。結果チャネルをポーリングする。
    Sending,
    /// 送信失敗。再送信で確定済み WAV を送り直せる。
    SendFailed { message: String },
    /// 指定デバイスが見つからない。
    DeviceMissing { device_name: String },
    /// 回復不能なエラー。閉じるのみ。
    Fatal { message: String },
}

/// ボタン操作を UI 描画クロージャの外へ持ち出すための中間表現。
///
/// 描画中は `self.state` を不変借用しているため、状態を変える操作は
/// 一度この enum に退避してからクロージャの外で適用する。
enum Action {
    None,
    FinishAndSend,
    Discard,
    Retry,
    Close,
}

/// アプリ本体。
pub struct App {
    /// 実行時設定。設定読込に失敗した Fatal 起動時のみ `None`。
    config: Option<Config>,
    state: State,
    /// 録音中のみ `Some`。停止時に `take` して `stop` する。
    session: Option<RecordingSession>,
    /// 送信中のみ `Some`。送信スレッドからの結果を受け取る。
    result_rx: Option<mpsc::Receiver<SendResult>>,
}

impl App {
    /// `AppInit` と `CreationContext` から `App` を構築する。
    ///
    /// ここで日本語フォントを差し込む（既定フォントは日本語グリフを持たないため）。
    pub fn new(cc: &eframe::CreationContext<'_>, init: AppInit) -> Self {
        install_japanese_font(&cc.egui_ctx);

        match init {
            AppInit::Recording { config, session } => Self {
                config: Some(config),
                state: State::Recording {
                    started_at: Instant::now(),
                },
                session: Some(session),
                result_rx: None,
            },
            AppInit::DeviceMissing { device_name } => Self {
                config: None,
                state: State::DeviceMissing { device_name },
                session: None,
                result_rx: None,
            },
            AppInit::Fatal { message } => Self {
                config: None,
                state: State::Fatal { message },
                session: None,
                result_rx: None,
            },
        }
    }

    /// 「終了して送信」: 録音を停止し WAV を確定してから送信を開始する。
    fn finish_and_send(&mut self) {
        if let Some(session) = self.session.take()
            && let Err(err) = session.stop()
        {
            self.state = State::Fatal {
                message: format!("録音の確定に失敗しました: {err}"),
            };
            return;
        }
        self.spawn_send();
    }

    /// 「破棄」: 録音を停止して（WAV は残すが送らず）ウィンドウを閉じる。
    fn discard(&mut self, ctx: &egui::Context) {
        if let Some(session) = self.session.take() {
            let _ = session.stop();
        }
        close_window(ctx);
    }

    /// 送信スレッドを起動し、状態を `Sending` にする。
    fn spawn_send(&mut self) {
        let Some(config) = &self.config else {
            self.state = State::Fatal {
                message: "設定が読み込まれていません".to_string(),
            };
            return;
        };

        let url = config.webhook_url_with_source();
        let user = config.basic_user.clone();
        let pass = config.basic_pass.clone();
        let wav_path = config.wav_path.clone();

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(sender::send_wav(&url, &user, &pass, &wav_path));
        });

        self.result_rx = Some(rx);
        self.state = State::Sending;
    }

    /// 送信スレッドの結果をポーリングし、届いていれば状態遷移する。
    ///
    /// 成功時は WAV を削除してウィンドウを閉じ、失敗時は `SendFailed` へ遷移する。
    fn poll_send_result(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.result_rx else { return };
        match rx.try_recv() {
            Ok(result) => {
                self.result_rx = None;
                if result.is_success() {
                    // 成功時のみ一時 WAV を削除する（失敗/破棄時は保持）。
                    if let Some(config) = &self.config {
                        let _ = std::fs::remove_file(&config.wav_path);
                    }
                    close_window(ctx);
                } else {
                    let mut message = result.message();
                    let detail = result.detail();
                    if !detail.is_empty() {
                        message.push('\n');
                        message.push_str(&detail);
                    }
                    self.state = State::SendFailed { message };
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                // まだ結果待ち。ポーリングを継続する。
                ctx.request_repaint_after(SEND_POLL_INTERVAL);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.result_rx = None;
                self.state = State::SendFailed {
                    message: "送信スレッドが異常終了しました".to_string(),
                };
            }
        }
    }
}

impl eframe::App for App {
    // eframe 0.35 では App::ui にルートの Ui が直接渡される（Context ではない）。
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Context は Arc なので安価に clone でき、ui の借用と分離して使える。
        let ctx = ui.ctx().clone();
        let mut action = Action::None;

        ui.add_space(8.0);
        match &self.state {
            State::Recording { started_at } => {
                let secs = started_at.elapsed().as_secs();
                ui.heading("🎙 音声メモ");
                ui.add_space(8.0);
                ui.label(format!("録音中… {}:{:02}", secs / 60, secs % 60));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("終了して送信").clicked() {
                        action = Action::FinishAndSend;
                    }
                    if ui.button("破棄").clicked() {
                        action = Action::Discard;
                    }
                });
            }
            State::Sending => {
                ui.heading("🎙 音声メモ");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("送信中…");
                });
            }
            State::SendFailed { message } => {
                ui.heading("送信に失敗しました");
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), message);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("再送信").clicked() {
                        action = Action::Retry;
                    }
                    if ui.button("閉じる").clicked() {
                        action = Action::Close;
                    }
                });
            }
            State::DeviceMissing { device_name } => {
                ui.heading("デバイスが見つかりません");
                ui.add_space(8.0);
                ui.label(format!("指定デバイス: {device_name}"));
                ui.add_space(12.0);
                if ui.button("閉じる").clicked() {
                    action = Action::Close;
                }
            }
            State::Fatal { message } => {
                ui.heading("エラー");
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), message);
                ui.add_space(12.0);
                if ui.button("閉じる").clicked() {
                    action = Action::Close;
                }
            }
        }

        // 描画を終えてから状態を変える操作を適用する。
        match action {
            Action::None => {}
            Action::FinishAndSend => self.finish_and_send(),
            Action::Discard => self.discard(&ctx),
            Action::Retry => self.spawn_send(),
            Action::Close => close_window(&ctx),
        }

        // 送信中は結果をポーリングし、録音中は経過秒表示のため再描画を予約する。
        if matches!(self.state, State::Sending) {
            self.poll_send_result(&ctx);
        }
        if matches!(self.state, State::Recording { .. }) {
            ctx.request_repaint_after(RECORDING_REPAINT_INTERVAL);
        }
    }
}

/// ウィンドウを閉じてプロセスを終了させる。
fn close_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

/// Windows のシステム日本語フォントを egui に差し込む。
///
/// 既定フォントは日本語グリフを持たず豆腐（□）になるため、候補を順に試して
/// 最初に読めたものを最優先フォントに設定する。いずれも無ければ既定のまま。
fn install_japanese_font(ctx: &egui::Context) {
    // 一般的な Windows 同梱の日本語フォント（TTC は index 0 の face を使う）。
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\YuGothR.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ];

    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "jp".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "jp".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("jp".to_owned());
        ctx.set_fonts(fonts);
        return;
    }
}
