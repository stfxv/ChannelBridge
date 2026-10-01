#[cfg(target_os = "macos")]
mod native;
mod startup;

fn main() {
    #[cfg(target_os = "macos")]
    native::run();
    #[cfg(not(target_os = "macos"))]
    eprintln!("ChannelBridge requires macOS");
}
