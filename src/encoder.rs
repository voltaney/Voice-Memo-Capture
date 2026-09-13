//! WAV → Ogg Opus 変換モジュール。
//!
//! 録音で確定した WAV を読み込み、Opus（libopus）でエンコードして Ogg コンテナへ
//! 多重化する。目的は送信データ量の削減（WAV 無圧縮に対し桁違いに小さくなる）。
//!
//! Opus は 8/12/16/24/48kHz のみを受け付けるため、デバイス既定サンプルレート
//! （44100 等）の場合は 48kHz へリサンプルしてからエンコードする。ほとんどの
//! ケースはアップサンプリング（44100→48000 等）なので線形補間で十分。
//!
//! 出力は RFC 7845 準拠の Ogg Opus（`OpusHead` / `OpusTags` + 20ms フレーム）。

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate};
use ogg::PacketWriter;
use ogg::writing::PacketWriteEndInfo;

/// Opus のエンコード先サンプルレート（Opus は 48kHz 内部処理が基本）。
const OPUS_SAMPLE_RATE: u32 = 48_000;
/// 1 フレームの長さ（ms）。20ms は Opus の標準的なフレーム長。
const FRAME_DURATION_MS: usize = 20;
/// 1 フレームあたりのチャンネル毎サンプル数（20ms @ 48kHz = 960）。
const FRAME_SIZE_PER_CHANNEL: usize = OPUS_SAMPLE_RATE as usize * FRAME_DURATION_MS / 1000;
/// 1 パケットの出力バッファ上限（余裕を持った値。Opus パケットはこれより十分小さい）。
const MAX_PACKET_SIZE: usize = 4000;
/// Ogg 論理ストリームのシリアル番号（単一ストリームなので固定でよい）。
const OGG_SERIAL: u32 = 0x0056_454D; // 'V','E','M' 由来の任意固定値
/// `OpusTags` に載せるベンダ文字列。
const VENDOR: &str = "VoiceHook";

/// エンコードに関するエラー。
#[derive(Debug)]
pub enum EncodeError {
    /// WAV 読み込み（hound）由来のエラー。
    Wav(hound::Error),
    /// Opus エンコード（audiopus）由来のエラー。
    Opus(audiopus::Error),
    /// Ogg 書き出し・ファイル I/O 由来のエラー。
    Io(std::io::Error),
    /// 未対応の入力（チャンネル数 0 など）。
    Unsupported(String),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::Wav(err) => write!(f, "WAV読み込みエラー: {err}"),
            EncodeError::Opus(err) => write!(f, "Opusエンコードエラー: {err}"),
            EncodeError::Io(err) => write!(f, "Ogg書き出しエラー: {err}"),
            EncodeError::Unsupported(msg) => write!(f, "未対応の音声形式です: {msg}"),
        }
    }
}

impl std::error::Error for EncodeError {}

impl From<hound::Error> for EncodeError {
    fn from(err: hound::Error) -> Self {
        EncodeError::Wav(err)
    }
}

impl From<audiopus::Error> for EncodeError {
    fn from(err: audiopus::Error) -> Self {
        EncodeError::Opus(err)
    }
}

impl From<std::io::Error> for EncodeError {
    fn from(err: std::io::Error) -> Self {
        EncodeError::Io(err)
    }
}

/// `wav_path` の WAV を Ogg Opus にエンコードして `ogg_path` へ書き出す。
///
/// `bitrate_kbps` は目標ビットレート（kbps）。チャンネルはモノラル/ステレオを
/// そのまま扱い、3ch 以上はモノラルへダウンミックスする。
pub fn encode_wav_to_ogg_opus(
    wav_path: &Path,
    ogg_path: &Path,
    bitrate_kbps: u32,
) -> Result<(), EncodeError> {
    // 1) WAV を読み、チャンネル毎の f32 サンプル列（[-1,1] 正規化）へ展開する。
    let (planar, in_sample_rate, in_channels) = read_wav_planar(wav_path)?;

    // 2) Opus が扱えるチャンネル構成へ整える（1=モノラル / 2=ステレオ、3ch 以上はモノラル化）。
    let (planar, out_channels) = fit_channels(planar, in_channels);

    // 3) 各チャンネルを 48kHz へリサンプルする（既に 48kHz なら素通り）。
    let resampled: Vec<Vec<f32>> = planar
        .iter()
        .map(|ch| resample_linear(ch, in_sample_rate, OPUS_SAMPLE_RATE))
        .collect();

    // チャンネル毎サンプル数（末尾のフレーム未満を切り詰めるための実サンプル数）。
    let samples_per_channel = resampled.first().map_or(0, |ch| ch.len());

    // 4) Opus エンコーダを構築（音声メモ用途なので VOIP モード）。
    let channels_enum = if out_channels == 2 {
        Channels::Stereo
    } else {
        Channels::Mono
    };
    let mut encoder = Encoder::new(SampleRate::Hz48000, channels_enum, Application::Voip)?;
    encoder.set_bitrate(Bitrate::BitsPerSecond((bitrate_kbps * 1000) as i32))?;
    // エンコーダの先読み（pre-skip）。デコーダが先頭で読み飛ばすサンプル数。
    let pre_skip = encoder.lookahead().unwrap_or(0) as u64;

    // 5) Ogg へ多重化しつつ、フレーム単位でエンコードする。
    if let Some(parent) = ogg_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = BufWriter::new(File::create(ogg_path)?);
    let mut writer = PacketWriter::new(file);

    // ヘッダページ: OpusHead（先頭ページ単独）→ OpusTags。
    let head = build_opus_head(out_channels as u8, pre_skip as u16, in_sample_rate);
    writer.write_packet(head, OGG_SERIAL, PacketWriteEndInfo::EndPage, 0)?;
    let tags = build_opus_tags();
    writer.write_packet(tags, OGG_SERIAL, PacketWriteEndInfo::EndPage, 0)?;

    // 音声データ: 48kHz のチャンネル列をインターリーブし、20ms ごとにエンコード。
    let interleaved = interleave(&resampled, out_channels);
    let frame_len = FRAME_SIZE_PER_CHANNEL * out_channels;
    let total_frames = interleaved.len().div_ceil(frame_len);
    let mut packet_buf = vec![0u8; MAX_PACKET_SIZE];
    let mut frame = vec![0f32; frame_len];

    for i in 0..total_frames {
        let start = i * frame_len;
        let end = (start + frame_len).min(interleaved.len());
        // フレームを組み立て、末尾フレームが満たない分は 0 で埋める。
        frame[..end - start].copy_from_slice(&interleaved[start..end]);
        frame[end - start..].fill(0.0);

        let len = encoder.encode_float(&frame, &mut packet_buf)?;
        let packet = packet_buf[..len].to_vec();

        let is_last = i + 1 == total_frames;
        // granulepos は 48kHz サンプル数の累積（pre-skip を含む）。
        // 末尾パケットは実サンプル数に合わせ、末尾フレームのパディング分を切り詰める。
        let granule = if is_last {
            pre_skip + samples_per_channel as u64
        } else {
            pre_skip + ((i + 1) * FRAME_SIZE_PER_CHANNEL) as u64
        };
        let info = if is_last {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        writer.write_packet(packet, OGG_SERIAL, info, granule)?;
    }

    Ok(())
}

/// WAV を読み込み、チャンネル毎の f32 サンプル列（[-1,1] 正規化）と
/// サンプルレート・チャンネル数を返す。
fn read_wav_planar(wav_path: &Path) -> Result<(Vec<Vec<f32>>, u32, usize), EncodeError> {
    let mut reader = hound::WavReader::open(wav_path)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    if channels == 0 {
        return Err(EncodeError::Unsupported(
            "チャンネル数が 0 です".to_string(),
        ));
    }

    // インターリーブされたサンプルを f32 へ正規化して読み込む。
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => {
            // ビット深度に応じた最大振幅で割り、[-1,1] へ正規化する。
            let scale = 1.0f32 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<Vec<_>, _>>()?
        }
    };

    // チャンネル毎に分解（インターリーブ → プレーナ）。
    let mut planar = vec![Vec::with_capacity(samples.len() / channels); channels];
    for (i, sample) in samples.into_iter().enumerate() {
        planar[i % channels].push(sample);
    }
    Ok((planar, spec.sample_rate, channels))
}

/// Opus が扱えるチャンネル構成へ整える。
///
/// 1ch/2ch はそのまま、3ch 以上は全チャンネルの平均でモノラルへダウンミックスする。
fn fit_channels(planar: Vec<Vec<f32>>, channels: usize) -> (Vec<Vec<f32>>, usize) {
    if channels <= 2 {
        return (planar, channels);
    }
    let len = planar.first().map_or(0, |ch| ch.len());
    let mut mono = Vec::with_capacity(len);
    for i in 0..len {
        let sum: f32 = planar
            .iter()
            .map(|ch| ch.get(i).copied().unwrap_or(0.0))
            .sum();
        mono.push(sum / channels as f32);
    }
    (vec![mono], 1)
}

/// 各チャンネルを線形補間で `from` → `to` Hz へリサンプルする。
///
/// 同一レートなら複製のみ。主な用途は 44100→48000 等のアップサンプリング。
fn resample_linear(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let last = input.len() - 1;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let a = input[idx.min(last)] as f64;
        let b = input[(idx + 1).min(last)] as f64;
        out.push((a + (b - a) * frac) as f32);
    }
    out
}

/// チャンネル毎（プレーナ）の列をインターリーブした 1 本の列へまとめる。
fn interleave(planar: &[Vec<f32>], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return planar[0].clone();
    }
    let len = planar.iter().map(|ch| ch.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(len * channels);
    for i in 0..len {
        for ch in planar.iter().take(channels) {
            out.push(ch[i]);
        }
    }
    out
}

/// `OpusHead` パケット（RFC 7845 §5.1）を組み立てる。
fn build_opus_head(channels: u8, pre_skip: u16, input_sample_rate: u32) -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1); // バージョン
    head.push(channels); // チャンネル数
    head.extend_from_slice(&pre_skip.to_le_bytes()); // pre-skip
    head.extend_from_slice(&input_sample_rate.to_le_bytes()); // 元のサンプルレート（情報用）
    head.extend_from_slice(&0i16.to_le_bytes()); // 出力ゲイン（0）
    head.push(0); // チャンネルマッピングファミリ（0=モノ/ステレオ）
    head
}

/// `OpusTags` パケット（RFC 7845 §5.2）を組み立てる（コメントは無し）。
fn build_opus_tags() -> Vec<u8> {
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
    tags.extend_from_slice(VENDOR.as_bytes());
    tags.extend_from_slice(&0u32.to_le_bytes()); // ユーザーコメント数（0）
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_linear_は同一レートで素通しする() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_linear(&input, 48000, 48000), input);
    }

    #[test]
    fn resample_linear_はアップサンプルで長さが比率どおり増える() {
        let input = vec![0.0, 1.0]; // 2 サンプル
        let out = resample_linear(&input, 24000, 48000); // 2 倍
        assert_eq!(out.len(), 4);
        // 先頭は元の値、間は補間される。
        assert!((out[0] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn fit_channels_は3ch以上をモノラル化する() {
        let planar = vec![vec![1.0, 1.0], vec![2.0, 2.0], vec![3.0, 3.0]];
        let (out, ch) = fit_channels(planar, 3);
        assert_eq!(ch, 1);
        assert_eq!(out.len(), 1);
        // (1+2+3)/3 = 2.0
        assert!((out[0][0] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn interleave_はステレオを交互に並べる() {
        let planar = vec![vec![1.0, 3.0], vec![2.0, 4.0]];
        assert_eq!(interleave(&planar, 2), vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn opus_head_は19バイトでマジックを持つ() {
        let head = build_opus_head(1, 312, 48000);
        assert_eq!(head.len(), 19);
        assert_eq!(&head[..8], b"OpusHead");
        assert_eq!(head[9], 1); // チャンネル数
    }

    /// 実際の libopus + Ogg 多重化までを通すスモークテスト。
    ///
    /// 44100Hz モノラル i16 の合成 WAV（正弦波）を作り、変換して
    /// 出力が Ogg Opus として妥当（`OggS` ページ + `OpusHead`/`OpusTags`）かを確認する。
    /// リサンプル（44100→48000）・i16 正規化・エンコード・多重化を実地で検証する。
    #[test]
    fn wavからoggへ変換すると妥当なopusストリームになる() {
        let dir = std::env::temp_dir().join("vmc_encoder_test");
        std::fs::create_dir_all(&dir).unwrap();
        let wav_path = dir.join("smoke.wav");
        let ogg_path = dir.join("smoke.ogg");

        // 0.2 秒・440Hz の正弦波を 44100Hz モノラル i16 で書き出す。
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut w = hound::WavWriter::create(&wav_path, spec).unwrap();
            let n = 44_100 / 5; // 0.2 秒
            for i in 0..n {
                let t = i as f32 / 44_100.0;
                let v = (t * 440.0 * std::f32::consts::TAU).sin();
                w.write_sample((v * i16::MAX as f32) as i16).unwrap();
            }
            w.finalize().unwrap();
        }

        encode_wav_to_ogg_opus(&wav_path, &ogg_path, 256).expect("変換に失敗");

        let bytes = std::fs::read(&ogg_path).unwrap();
        // Ogg ページはマジック "OggS" で始まる。
        assert_eq!(&bytes[..4], b"OggS", "先頭が Ogg ページでない");
        // 識別ヘッダ・コメントヘッダが含まれる。
        assert!(
            bytes.windows(8).any(|w| w == b"OpusHead"),
            "OpusHead が無い"
        );
        assert!(
            bytes.windows(8).any(|w| w == b"OpusTags"),
            "OpusTags が無い"
        );
        // ヘッダのみでなく音声ページも書かれている（ある程度のサイズがある）。
        assert!(
            bytes.len() > 200,
            "出力が小さすぎる: {} バイト",
            bytes.len()
        );

        let _ = std::fs::remove_file(&wav_path);
        let _ = std::fs::remove_file(&ogg_path);
    }
}
