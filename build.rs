// ui/app.slint をビルド時に Rust コードへコンパイルする。
fn main() {
    slint_build::compile("ui/app.slint").expect("app.slint のコンパイルに失敗しました");
}
