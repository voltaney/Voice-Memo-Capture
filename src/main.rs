// モジュールは段階的に実装中。GUI 実装ステップで各モジュールを配線するまで
// バイナリからは未使用のため、それまで dead_code を許可する。
#[allow(dead_code)]
mod audio;
#[allow(dead_code)]
mod config;
#[allow(dead_code)]
mod sender;

fn main() {
    println!("Hello, world!");
}
