use channelbridge_hal_abi::interface::{DriverInterface, HostInterface};
use channelbridge_hal_abi::*;
use std::mem::offset_of;
fn main() {
    println!("AudioTimestamp {}", size_of::<AudioTimestamp>());
    println!("IoCycleInfo {}", size_of::<IoCycleInfo>());
    println!(
        "IoCycleInfo.current_time {}",
        offset_of!(IoCycleInfo, current_time)
    );
    println!(
        "IoCycleInfo.output_time {}",
        offset_of!(IoCycleInfo, output_time)
    );
    println!("ClientInfo {}", size_of::<ClientInfo>());
    println!("ClientInfo.bundle_id {}", offset_of!(ClientInfo, bundle_id));
    println!("PropertyAddress {}", size_of::<PropertyAddress>());
    println!("HostInterface {}", size_of::<HostInterface>());
    println!("DriverInterface {}", size_of::<DriverInterface>());
    println!(
        "DriverInterface.do_io {}",
        offset_of!(DriverInterface, do_io)
    );
    println!("StreamFormat {}", size_of::<StreamFormat>());
    println!("RangedFormat {}", size_of::<RangedFormat>());
    println!("AudioBuffer {}", size_of::<AudioBuffer>());
    println!("AudioBufferList {}", size_of::<AudioBufferList>());
    println!(
        "AudioBufferList.buffers {}",
        offset_of!(AudioBufferList, buffers)
    );
}
