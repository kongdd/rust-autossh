use super::*;
use std::process;

#[test]
fn startup_replaces_previous_log() {
    let directory = std::env::temp_dir().join(format!("rust-autossh-log-{}", process::id()));
    let path = directory.join("autossh.log");
    fs::create_dir_all(&directory).unwrap();
    fs::write(&path, "old log\n").unwrap();

    let logger = Logger::new(
        Path::new("config.toml"),
        &LogConfig {
            file: Some(path.clone()),
        },
    )
    .unwrap();
    logger.info("new log");
    drop(logger);

    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("old log"));
    assert!(text.contains("new log"));
    fs::remove_dir_all(directory).unwrap();
}
