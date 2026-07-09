// ビルド時の処理:
//  1. ui/app.slint を Rust コードへコンパイルする。
//  2. Windows 向けビルドでは exe にアプリアイコン（assets/icon.ico）を埋め込む。
fn main() {
    slint_build::compile("ui/app.slint").expect("app.slint のコンパイルに失敗しました");

    // Explorer・タスクバー・ショートカット(.lnk) で表示される exe のアイコン。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(err) = res.compile() {
            // アイコン埋め込みに失敗してもビルド自体は続行する（警告のみ）。
            println!("cargo:warning=アイコンの埋め込みに失敗しました: {err}");
        }
    }
}
