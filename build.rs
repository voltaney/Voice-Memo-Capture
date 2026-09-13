// ビルド時の処理:
//  1. ui/app.slint を Rust コードへコンパイルする。
//  2. Windows 向けビルドでは exe にアプリアイコン（assets/icon.ico）と
//     バージョン情報（プロパティの「詳細」タブに出る発行元・バージョン等）を埋め込む。
fn main() {
    slint_build::compile("ui/app.slint").expect("app.slint のコンパイルに失敗しました");

    // Explorer・タスクバー・ショートカット(.lnk) で表示される exe のアイコン。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");

        // exe のプロパティ「詳細」タブに表示される情報。
        // FileVersion / ProductVersion には Cargo.toml の version が自動で入る。
        res.set("ProductName", "VoiceHook（音声メモ）");
        // Windows はタスクマネージャ等でのアプリ名として FileDescription を使う。
        res.set("FileDescription", "VoiceHook（音声メモ）");
        res.set("OriginalFilename", "VoiceHook.exe");
        if let Err(err) = res.compile() {
            // 埋め込みに失敗してもビルド自体は続行する（警告のみ）。
            println!("cargo:warning=アイコン・バージョン情報の埋め込みに失敗しました: {err}");
        }
    }
}
