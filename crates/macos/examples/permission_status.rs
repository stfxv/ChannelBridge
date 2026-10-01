//! Query only: does not request microphone access or open an audio device.
#[cfg(target_os = "macos")]
fn main() {
    println!(
        "Microphone permission: {:?}",
        channelbridge_macos::permission::status()
    );
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("microphone permission query requires macOS");
}
