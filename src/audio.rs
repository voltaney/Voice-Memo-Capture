//! 録音まわりのモジュール。
//!
//! - 入力デバイスの列挙・存在チェック（`find_input_device`）
//!   デバイス名が指定されていない場合はチェックを行わず、OS の既定の入力デバイスを使う。
//! - 録音セッションの開始／停止（`RecordingSession`）
//! - cpal のコールバックで受け取ったサンプルを hound で WAV に書き出す
//!
//! cpal の `Stream` は `!Send` なので、ストリームの構築・保持・破棄はすべて
//! 録音スレッド内で完結させる。呼び出し側とはチャネルと停止フラグでやり取りする。

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SupportedStreamConfig};

/// 停止フラグを確認する間隔。
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// hound の WAV ライターを複数スレッドで共有するためのハンドル。
type WavWriterHandle = Arc<Mutex<Option<hound::WavWriter<BufWriter<File>>>>>;

/// 録音に関するエラー。
#[derive(Debug)]
pub enum AudioError {
    /// 指定名に一致する入力デバイスが見つからない。
    DeviceNotFound(String),
    /// 既定の入力デバイスが存在しない（マイクが 1 つも無い）。
    NoDefaultDevice,
    /// 未対応のサンプルフォーマット。
    UnsupportedSampleFormat(SampleFormat),
    /// cpal 由来のエラー。
    Cpal(String),
    /// WAV 書き出し（hound）由来のエラー。
    Wav(hound::Error),
    /// ファイル I/O 由来のエラー。
    Io(std::io::Error),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::DeviceNotFound(name) => {
                write!(f, "入力デバイスが見つかりません: {name}")
            }
            AudioError::NoDefaultDevice => {
                write!(
                    f,
                    "既定の入力デバイスが見つかりません（マイクが接続されていません）"
                )
            }
            AudioError::UnsupportedSampleFormat(fmt) => {
                write!(f, "未対応のサンプルフォーマットです: {fmt}")
            }
            AudioError::Cpal(msg) => write!(f, "音声デバイスのエラー: {msg}"),
            AudioError::Wav(err) => write!(f, "WAV書き出しエラー: {err}"),
            AudioError::Io(err) => write!(f, "ファイルI/Oエラー: {err}"),
        }
    }
}

impl std::error::Error for AudioError {}

impl From<hound::Error> for AudioError {
    fn from(err: hound::Error) -> Self {
        AudioError::Wav(err)
    }
}

impl From<std::io::Error> for AudioError {
    fn from(err: std::io::Error) -> Self {
        AudioError::Io(err)
    }
}

/// `target` を名前に部分一致で含む入力デバイスを探し、その表示名を返す。
///
/// 大文字小文字は無視して比較する。見つからなければ `None`。
/// 誤って内蔵マイク等で録音しないよう、起動時の存在チェックに使う。
pub fn find_input_device(target: &str) -> Option<String> {
    let host = cpal::default_host();
    let devices = host.input_devices().ok()?;
    let needle = target.to_lowercase();
    for device in devices {
        if let Ok(desc) = device.description() {
            let name = desc.name();
            if name.to_lowercase().contains(&needle) {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// 進行中の録音セッション。
///
/// `start` で録音スレッドを起動し、`stop` で停止フラグを立てて join する。
pub struct RecordingSession {
    /// 録音スレッドへ停止を伝えるフラグ。
    stop: Arc<AtomicBool>,
    /// 録音スレッドのハンドル。停止時に join して finalize 結果を受け取る。
    handle: Option<JoinHandle<Result<(), AudioError>>>,
}

impl RecordingSession {
    /// 録音を開始し、`wav_path` へ書き出す録音スレッドを起動する。
    ///
    /// `target_device_name` が `None` のときは OS の既定の入力デバイスを使う。
    /// ストリームの構築・再生開始まで成功したことを確認してから返すため、
    /// この関数が `Ok` を返した時点で実際に録音が始まっている。
    pub fn start(
        target_device_name: Option<String>,
        wav_path: PathBuf,
    ) -> Result<Self, AudioError> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        // 録音スレッドの初期化結果（ストリーム再生開始まで）を受け取るチャネル。
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), AudioError>>();

        let handle = std::thread::spawn(move || {
            record_loop(
                target_device_name.as_deref(),
                &wav_path,
                &stop_for_thread,
                &ready_tx,
            )
        });

        // 初期化結果を待つ。送信元が落ちた場合も初期化失敗として扱う。
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                handle: Some(handle),
            }),
            Ok(Err(err)) => {
                // 初期化に失敗しているのでスレッドは既に終了している。
                let _ = handle.join();
                Err(err)
            }
            Err(_) => {
                let _ = handle.join();
                Err(AudioError::Cpal(
                    "録音スレッドの初期化に失敗しました".to_string(),
                ))
            }
        }
    }

    /// 録音を停止し、WAV を finalize する。finalize 結果を返す。
    pub fn stop(mut self) -> Result<(), AudioError> {
        self.stop.store(true, Ordering::SeqCst);
        match self.handle.take() {
            Some(handle) => handle.join().unwrap_or_else(|_| {
                Err(AudioError::Cpal(
                    "録音スレッドが異常終了しました".to_string(),
                ))
            }),
            None => Ok(()),
        }
    }
}

/// 録音スレッドの本体。デバイスを開き、ストリームを構築・再生し、
/// 停止フラグが立つまでコールバックで WAV へ書き込む。
///
/// ストリーム再生開始の成否を `ready_tx` で呼び出し側へ通知してから、
/// 停止フラグの監視ループに入る。
fn record_loop(
    target_device_name: Option<&str>,
    wav_path: &Path,
    stop: &AtomicBool,
    ready_tx: &mpsc::Sender<Result<(), AudioError>>,
) -> Result<(), AudioError> {
    // セットアップ（デバイス取得〜再生開始）。失敗したら通知して終了する。
    let (stream, writer) = match setup_stream(target_device_name, wav_path) {
        Ok(pair) => pair,
        Err(err) => {
            // 呼び出し側へ失敗を通知（受信側が落ちていても無視）。
            let _ = ready_tx.send(Err(clone_error(&err)));
            return Err(err);
        }
    };

    if let Err(err) = stream.play() {
        let err = AudioError::Cpal(err.to_string());
        let _ = ready_tx.send(Err(clone_error(&err)));
        return Err(err);
    }

    // ここまで来たら録音開始成功。
    let _ = ready_tx.send(Ok(()));

    // 停止フラグが立つまで待機（ストリームは別スレッドのコールバックで駆動される）。
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(STOP_POLL_INTERVAL);
    }

    // ストリームを止めてから WAV を finalize する。
    drop(stream);
    if let Some(writer) = writer.lock().unwrap().take() {
        writer.finalize()?;
    }
    Ok(())
}

/// デバイスを取得し、WAV ライターと入力ストリームを構築して返す。
///
/// `target_device_name` が `None` のときは既定の入力デバイスを使う。
fn setup_stream(
    target_device_name: Option<&str>,
    wav_path: &Path,
) -> Result<(cpal::Stream, WavWriterHandle), AudioError> {
    let host = cpal::default_host();
    let device = match target_device_name {
        Some(name) => host
            .input_devices()
            .map_err(|e| AudioError::Cpal(e.to_string()))?
            .find(|d| {
                d.description()
                    .map(|desc| desc.name().to_lowercase().contains(&name.to_lowercase()))
                    .unwrap_or(false)
            })
            .ok_or_else(|| AudioError::DeviceNotFound(name.to_string()))?,
        None => host
            .default_input_device()
            .ok_or(AudioError::NoDefaultDevice)?,
    };

    let config = device
        .default_input_config()
        .map_err(|e| AudioError::Cpal(e.to_string()))?;

    // 出力先ディレクトリを用意し、固定パスの WAV を作成（毎回上書き）。
    if let Some(parent) = wav_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = wav_spec_from_config(&config)?;
    let writer = hound::WavWriter::create(wav_path, spec)?;
    let writer: WavWriterHandle = Arc::new(Mutex::new(Some(writer)));
    let writer_for_cb = writer.clone();

    let err_fn = |err: cpal::Error| {
        eprintln!("ストリームエラー: {err}");
    };

    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();
    let stream = match sample_format {
        SampleFormat::I16 => device.build_input_stream(
            stream_config,
            move |data: &[i16], _: &_| write_input_data::<i16, i16>(data, &writer_for_cb),
            err_fn,
            None,
        ),
        SampleFormat::I32 => device.build_input_stream(
            stream_config,
            move |data: &[i32], _: &_| write_input_data::<i32, i32>(data, &writer_for_cb),
            err_fn,
            None,
        ),
        SampleFormat::F32 => device.build_input_stream(
            stream_config,
            move |data: &[f32], _: &_| write_input_data::<f32, f32>(data, &writer_for_cb),
            err_fn,
            None,
        ),
        other => return Err(AudioError::UnsupportedSampleFormat(other)),
    }
    .map_err(|e| AudioError::Cpal(e.to_string()))?;

    Ok((stream, writer))
}

/// cpal の `SupportedStreamConfig` から hound の `WavSpec` を組み立てる。
fn wav_spec_from_config(config: &SupportedStreamConfig) -> Result<hound::WavSpec, AudioError> {
    let sample_format = config.sample_format();
    let hound_format = if sample_format.is_float() {
        hound::SampleFormat::Float
    } else {
        hound::SampleFormat::Int
    };
    Ok(hound::WavSpec {
        channels: config.channels(),
        sample_rate: config.sample_rate(),
        bits_per_sample: (sample_format.sample_size() * 8) as u16,
        sample_format: hound_format,
    })
}

/// コールバックで受け取ったサンプル列を WAV ライターへ書き込む。
///
/// ロックが取れない場合（finalize 中など）はそのフレームを捨てる。
fn write_input_data<T, U>(input: &[T], writer: &WavWriterHandle)
where
    T: Sample,
    U: Sample + hound::Sample + FromSample<T>,
{
    if let Ok(mut guard) = writer.try_lock()
        && let Some(writer) = guard.as_mut()
    {
        for &sample in input.iter() {
            let sample: U = U::from_sample(sample);
            writer.write_sample(sample).ok();
        }
    }
}

/// エラーを表示文字列経由で複製する（`AudioError` は元エラーを含み `Clone` 不可のため、
/// チャネル送信用に文字列化したものを `Cpal` として持ち回る）。
fn clone_error(err: &AudioError) -> AudioError {
    match err {
        AudioError::DeviceNotFound(name) => AudioError::DeviceNotFound(name.clone()),
        AudioError::NoDefaultDevice => AudioError::NoDefaultDevice,
        AudioError::UnsupportedSampleFormat(fmt) => AudioError::UnsupportedSampleFormat(*fmt),
        other => AudioError::Cpal(other.to_string()),
    }
}
