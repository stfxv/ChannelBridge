//! Counts the test microphone's samples without writing audio or saving samples.
use std::error::Error;
use std::time::Duration;
fn main() -> Result<(), Box<dyn Error>> {
    let mut expected = None;
    let mut seconds = 3_u64;
    let mut duration_supplied = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--expect-silence" | "--expect-signal" if expected.is_none() => expected = Some(arg),
            "--seconds" if !duration_supplied => {
                seconds = args.next().ok_or("--seconds requires a value")?.parse()?;
                if !(1..=28_800).contains(&seconds) {
                    return Err("--seconds must be between 1 and 28800".into());
                }
                duration_supplied = true;
            }
            _ => {
                return Err(
                    "usage: monitor [--expect-silence|--expect-signal] [--seconds 1..28800]".into(),
                );
            }
        }
    }
    println!(
        "Observing test microphone for {seconds} seconds; samples are not saved. This does not establish signal fidelity or uninterrupted callback timing."
    );
    let report = channelbridge_macos::monitor::run(Duration::from_secs(seconds))?;
    println!("{report:?}");
    if !report.valid() {
        return Err("invalid or insufficient input callbacks".into());
    }
    match expected.as_deref() {
        Some("--expect-silence") if report.nonzero_frames != [0, 0] => {
            Err("expected silence".into())
        }
        Some("--expect-signal") if report.nonzero_frames == [0, 0] => {
            Err("expected a signal".into())
        }
        _ => Ok(()),
    }
}
