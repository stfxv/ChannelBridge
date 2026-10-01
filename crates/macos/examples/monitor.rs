//! Counts the test microphone's samples without writing audio or saving samples.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let expected = std::env::args().nth(1);
    if !matches!(
        expected.as_deref(),
        None | Some("--expect-silence") | Some("--expect-signal")
    ) || std::env::args().count() > 2
    {
        return Err("usage: monitor [--expect-silence|--expect-signal]".into());
    }
    let report = channelbridge_macos::monitor::run(std::time::Duration::from_secs(3))?;
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
