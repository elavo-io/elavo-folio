//! Applications that are playing audio, from PipeWire's PulseAudio interface.
//! Missing `pactl` means no marks, which is the same as the setting being off.

use std::collections::HashSet;

pub fn playing_names() -> HashSet<String> {
    let Ok(output) = std::process::Command::new("pactl")
        .args(["list", "sink-inputs"])
        .output()
    else {
        return HashSet::new();
    };
    if !output.status.success() {
        return HashSet::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut names = HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        for prefix in ["application.name = ", "application.process.binary = "] {
            if let Some(rest) = line.strip_prefix(prefix) {
                names.insert(rest.trim_matches('"').to_owned());
            }
        }
    }
    names
}

pub fn play_commit_sound() {
    let _ = std::process::Command::new("canberra-gtk-play")
        .args(["-i", "bell"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
