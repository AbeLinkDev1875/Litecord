//! Saved copies of servers (channels and chat history), one JSON file per server in
//! `rooms/` next to state.json. Like the web app keeps them in the browser: whoever hosts
//! a server next brings everyone's copies together.

use std::path::PathBuf;

use serde_json::Value;

fn path(room: &str) -> PathBuf {
    let safe: String = room.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    crate::store::data_dir().join("rooms").join(format!("{safe}.json"))
}

pub fn save(room: &str, snapshot: &Value) {
    let p = path(room);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, snapshot.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

pub fn load(room: &str) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path(room)).ok()?).ok()
}

pub fn forget(room: &str) {
    let _ = std::fs::remove_file(path(room));
}
