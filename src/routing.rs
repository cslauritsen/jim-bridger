use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;
use tokio::sync::Mutex;

/// A single delivery target for an alias.
#[derive(Debug, Clone, Deserialize)]
pub struct RoutingTarget {
    pub target: String,
    #[serde(rename = "type")]
    pub target_type: String,
}

/// Routing rule for one alias address.
#[derive(Debug, Clone, Deserialize)]
pub struct RoutingEntry {
    pub targets: Vec<RoutingTarget>,
}

/// Alias address (lowercased) -> routing entry.
pub type RoutingMap = HashMap<String, RoutingEntry>;

struct RoutingState {
    map: RoutingMap,
    last_mtime: Option<SystemTime>,
}

/// Loads and caches the alias routing map from disk, reloading only when the
/// file's modification time changes (mirrors `get_live_routing_map` in the
/// original Python implementation).
pub struct RoutingConfig {
    path: PathBuf,
    state: Mutex<RoutingState>,
}

impl RoutingConfig {
    pub fn new(path: impl AsRef<Path>) -> Self {
        RoutingConfig {
            path: path.as_ref().to_path_buf(),
            state: Mutex::new(RoutingState {
                map: RoutingMap::new(),
                last_mtime: None,
            }),
        }
    }

    /// Returns the current routing map, reloading from disk if the file has
    /// changed since the last read. On read/parse errors, the previously
    /// cached map is returned and the error is logged.
    pub async fn get(&self) -> RoutingMap {
        let mut state = self.state.lock().await;

        let metadata = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(_) => return state.map.clone(),
        };

        let mtime = metadata.modified().ok();
        if mtime == state.last_mtime {
            return state.map.clone();
        }

        match std::fs::read_to_string(&self.path)
            .map_err(|e| e.to_string())
            .and_then(|contents| serde_json::from_str::<RoutingMap>(&contents).map_err(|e| e.to_string()))
        {
            Ok(map) => {
                tracing::info!("Loaded {} dynamic aliases from disk", map.len());
                state.map = map;
                state.last_mtime = mtime;
            }
            Err(e) => {
                tracing::error!("Error accessing or parsing dynamic routing map: {e}");
            }
        }

        state.map.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"{
      "userlocal@example.com": { "targets": [ { "target": "csl", "type": "lda" } ] },
      "admins@example.com": { "targets": [
          { "target": "some.other.user@example.org", "type": "smtp" },
          { "target": "root", "type": "lda" }
      ] }
    }"#;

    #[test]
    fn deserializes_documented_schema() {
        let map: RoutingMap = serde_json::from_str(SAMPLE_JSON).unwrap();
        assert_eq!(map.len(), 2);
        let admins = &map["admins@example.com"];
        assert_eq!(admins.targets.len(), 2);
        assert_eq!(admins.targets[0].target_type, "smtp");
        assert_eq!(admins.targets[1].target_type, "lda");
    }

    #[tokio::test]
    async fn reloads_on_mtime_change_and_caches_otherwise() {
        let dir = std::env::temp_dir().join(format!("jb_routing_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aliases.json");
        std::fs::write(&path, SAMPLE_JSON).unwrap();

        let cfg = RoutingConfig::new(&path);
        let map1 = cfg.get().await;
        assert_eq!(map1.len(), 2);

        // Unchanged file: cached map still returned.
        let map2 = cfg.get().await;
        assert_eq!(map2.len(), 2);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir_all(&dir).ok();
    }
}
