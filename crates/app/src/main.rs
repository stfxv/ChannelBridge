#[cfg(target_os = "macos")]
mod input_meters;
#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
mod routing_controls;
mod startup;
#[cfg(target_os = "macos")]
mod status_menu;

fn main() {
    #[cfg(target_os = "macos")]
    native::run();
    #[cfg(not(target_os = "macos"))]
    eprintln!("ChannelBridge requires macOS");
}
