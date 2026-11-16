use channelbridge_core::{Config, storage};
use std::fs;

#[test]
fn save_preserves_occupied_work_names_and_publishes_hidden_destinations() {
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("channelbridge-publication-{pid}"));
    fs::create_dir(&dir).expect("the test process owns its fixture directory");
    let occupied = dir.join(format!(".channelbridge-settings-{pid}-0.tmp"));
    fs::write(&occupied, b"previous writer").expect("fixture directory is writable");
    let config = Config {
        device_uid: "test".into(),
        ..Config::default()
    };
    let normal = dir.join("settings.json");
    let result = storage::save(&normal, &config);
    assert!(
        result.is_ok(),
        "an occupied staging name blocked a save: {result:?}"
    );
    assert_eq!(
        storage::load(&normal).expect("settings were committed"),
        config
    );
    assert_eq!(
        fs::read(&occupied).expect("original file remains"),
        b"previous writer"
    );
    let hidden = dir
        .join(".")
        .join(format!(".channelbridge-settings-{pid}-2.tmp"));
    storage::save(&hidden, &config).expect("a hidden destination can be saved");
    assert_eq!(
        storage::load(&hidden).expect("hidden destination remains"),
        config
    );
    fs::remove_dir_all(dir).expect("test owns its temporary directory");
}
