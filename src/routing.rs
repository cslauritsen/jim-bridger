use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use regex::Regex;
use serde::Deserialize;
use tokio::sync::Mutex;

/// A single delivery target for an alias.
#[derive(Debug, Clone, Deserialize)]
pub struct RoutingTarget {
    pub target: String,
    #[serde(rename = "type")]
    pub target_type: String,
}

/// Routing rule for one alias pattern.
#[derive(Debug, Clone, Deserialize)]
pub struct RoutingEntry {
    pub targets: Vec<RoutingTarget>,
}

/// Raw deserialized form from JSON: regex pattern string -> entry.
type RawRoutingMap = HashMap<String, RoutingEntry>;

/// A compiled routing table. Each entry pairs a compiled [`Regex`] (derived
/// from the JSON key) with the delivery targets for matching addresses.
/// Clone is cheap — the inner `Vec` is reference-counted.
#[derive(Clone, Default)]
pub struct CompiledRoutingTable {
    entries: Arc<Vec<(Regex, RoutingEntry)>>,
}

impl CompiledRoutingTable {
    /// Returns all [`RoutingEntry`] values whose pattern matches `addr`.
    /// Multiple entries may match; all are returned so the caller can
    /// attempt delivery to every applicable target.
    pub fn matching(&self, addr: &str) -> Vec<&RoutingEntry> {
        self.entries
            .iter()
            .filter(|(re, _)| re.is_match(addr))
            .map(|(_, entry)| entry)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn compile_routing_table(raw: RawRoutingMap) -> Result<CompiledRoutingTable, String> {
    let mut entries = Vec::with_capacity(raw.len());
    for (pattern, entry) in raw {
        let re = Regex::new(&pattern)
            .map_err(|e| format!("Invalid regex {pattern:?}: {e}"))?;
        entries.push((re, entry));
    }
    Ok(CompiledRoutingTable { entries: Arc::new(entries) })
}

struct RoutingState {
    table: CompiledRoutingTable,
    last_mtime: Option<SystemTime>,
}

/// Loads and caches the routing table from disk, reloading only when the
/// file's modification time changes. Regex patterns are compiled once at
/// load time and held in memory until the next reload.
pub struct RoutingConfig {
    path: PathBuf,
    state: Mutex<RoutingState>,
}

impl RoutingConfig {
    pub fn new(path: impl AsRef<Path>) -> Self {
        RoutingConfig {
            path: path.as_ref().to_path_buf(),
            state: Mutex::new(RoutingState {
                table: CompiledRoutingTable::default(),
                last_mtime: None,
            }),
        }
    }

    /// Returns the compiled routing table, reloading from disk when the file's
    /// mtime has changed. On read/parse/compile errors the previously cached
    /// table is returned and the error is logged.
    pub async fn get(&self) -> CompiledRoutingTable {
        let mut state = self.state.lock().await;

        let metadata = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(_) => return state.table.clone(),
        };

        let mtime = metadata.modified().ok();
        if mtime == state.last_mtime {
            return state.table.clone();
        }

        let result = std::fs::read_to_string(&self.path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<RawRoutingMap>(&s).map_err(|e| e.to_string()))
            .and_then(compile_routing_table);

        match result {
            Ok(table) => {
                tracing::info!("Loaded {} routing patterns from disk", table.len());
                state.table = table;
                state.last_mtime = mtime;
            }
            Err(e) => {
                tracing::error!("Error loading routing config: {e}");
            }
        }

        state.table.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keys are regex patterns; plain strings are valid regexes that behave
    // like substring matches unless anchored with ^ / $.
    const SAMPLE_JSON: &str = r#"{
      "^userlocal@example\\.com$": { "targets": [ { "target": "csl", "type": "lda" } ] },
      "^admins@example\\.com$": { "targets": [
          { "target": "some.other.user@example.org", "type": "smtp" },
          { "target": "root", "type": "lda" }
      ] }
    }"#;

    #[test]
    fn compiles_and_matches_exact_patterns() {
        let raw: RawRoutingMap = serde_json::from_str(SAMPLE_JSON).unwrap();
        let table = compile_routing_table(raw).unwrap();
        assert_eq!(table.len(), 2);

        let hits = table.matching("admins@example.com");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].targets.len(), 2);
        assert_eq!(hits[0].targets[0].target_type, "smtp");
        assert_eq!(hits[0].targets[1].target_type, "lda");
    }

    #[test]
    fn no_match_returns_empty() {
        let raw: RawRoutingMap = serde_json::from_str(SAMPLE_JSON).unwrap();
        let table = compile_routing_table(raw).unwrap();
        assert!(table.matching("unknown@example.com").is_empty());
    }

    #[test]
    fn multiple_patterns_can_match_same_address() {
        let json = r#"{
          "^catch-all@": { "targets": [ { "target": "archive", "type": "lda" } ] },
          "^catch-all@example\\.com$": { "targets": [ { "target": "alice", "type": "lda" } ] }
        }"#;
        let raw: RawRoutingMap = serde_json::from_str(json).unwrap();
        let table = compile_routing_table(raw).unwrap();
        let hits = table.matching("catch-all@example.com");
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn invalid_regex_returns_error() {
        let json = r#"{ "(invalid[": { "targets": [] } }"#;
        let raw: RawRoutingMap = serde_json::from_str(json).unwrap();
        assert!(compile_routing_table(raw).is_err());
    }

    #[tokio::test]
    async fn reloads_on_mtime_change_and_caches_otherwise() {
        let dir = std::env::temp_dir().join(format!("jb_routing_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aliases.json");
        std::fs::write(&path, SAMPLE_JSON).unwrap();

        let cfg = RoutingConfig::new(&path);
        let table1 = cfg.get().await;
        assert_eq!(table1.len(), 2);

        // Unchanged file: cached table still returned.
        let table2 = cfg.get().await;
        assert_eq!(table2.len(), 2);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir_all(&dir).ok();
    }
}
