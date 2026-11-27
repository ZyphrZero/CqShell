//! File-fingerprint-keyed summary cache for Mihomo runtime YAML files.
//!
//! Avoids re-reading and re-parsing multi-megabyte `runtime.yaml` / `config.yaml` multiple times
//! per monitor tick while staying strictly coherent across file edits and atomic replacements.

use super::{ProxyConfig, controller, yaml_prep};
use crate::{
    error::{Error, Result},
    storage,
};
use serde_yaml::Value;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::UNIX_EPOCH,
};

const MAX_CACHED_FILES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileFingerprint {
    pub len: u64,
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub ino: u64,
}

#[derive(Debug, Clone)]
pub(in crate::ops::clash) struct CachedRuntime {
    pub controller: Option<controller::Settings>,
    pub proxy: Option<ProxyConfig>,
    pub external_ui: Option<String>,
    pub tun_enabled: bool,
}

type CacheMap = HashMap<PathBuf, (FileFingerprint, Arc<CachedRuntime>)>;

fn cache() -> &'static Mutex<CacheMap> {
    static CACHE: OnceLock<Mutex<CacheMap>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn parse_counts() -> &'static Mutex<HashMap<PathBuf, usize>> {
    static PARSE_COUNTS: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();
    PARSE_COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn parse_count_for(path: &Path) -> usize {
    parse_counts()
        .lock()
        .ok()
        .and_then(|g| g.get(path).copied())
        .unwrap_or(0)
}

fn fingerprint(path: &Path) -> Result<FileFingerprint> {
    storage::check_path(path, true)?;
    let meta = fs::metadata(path)?;
    let (mtime_secs, mtime_nanos) = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (d.as_secs(), d.subsec_nanos()))
        .unwrap_or((0, 0));
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    #[cfg(not(unix))]
    let ino = 0u64;
    Ok(FileFingerprint {
        len: meta.len(),
        mtime_secs,
        mtime_nanos,
        ino,
    })
}

fn extract_summary(value: &Value) -> Result<CachedRuntime> {
    let map = value
        .as_mapping()
        .ok_or_else(|| Error::new("error.data", "Clash config root must be a map"))?;
    let controller = controller::from_config(value)?;
    let proxy = super::parse_proxy_config_from_map(map).ok();
    let external_ui = match map.get(Value::String("external-ui".into())) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return Err(Error::new(
                "error.data",
                "external-ui must be a path string",
            ));
        }
    };
    let tun_enabled = map
        .get(Value::String("tun".into()))
        .and_then(Value::as_mapping)
        .and_then(|t| t.get(Value::String("enable".into())))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(CachedRuntime {
        controller,
        proxy,
        external_ui,
        tun_enabled,
    })
}

/// Load a cached summary of `path`, parsing YAML only when the file fingerprint changes.
pub(in crate::ops::clash) fn load_file(path: &Path) -> Result<Arc<CachedRuntime>> {
    let fp = fingerprint(path)?;
    if let Ok(guard) = cache().lock()
        && let Some((cached_fp, entry)) = guard.get(path)
        && *cached_fp == fp
    {
        return Ok(Arc::clone(entry));
    }
    let bytes = fs::read(path)?;
    #[cfg(test)]
    if let Ok(mut counts) = parse_counts().lock() {
        *counts.entry(path.to_path_buf()).or_insert(0) += 1;
    }
    let value = yaml_prep::parse_yaml_safe(&bytes)?;
    let summary = Arc::new(extract_summary(&value)?);
    if let Ok(mut guard) = cache().lock() {
        if guard.len() >= MAX_CACHED_FILES && !guard.contains_key(path) {
            guard.clear();
        }
        guard.insert(path.to_path_buf(), (fp, Arc::clone(&summary)));
    }
    Ok(summary)
}

/// Drop cached entry for a single path (called after atomic writes/commits).
pub(in crate::ops::clash) fn invalidate(path: &Path) {
    if let Ok(mut guard) = cache().lock() {
        guard.remove(path);
    }
}

/// Clear all cached runtime summaries (e.g. on TUI task completion).
pub(crate) fn invalidate_all() {
    if let Ok(mut guard) = cache().lock() {
        guard.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_cache_hits_on_unchanged_file_and_invalidates_on_change() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("runtime.yaml");
        fs::write(
            &file,
            "mode: rule\nmixed-port: 7890\nexternal-controller: 127.0.0.1:9090\nsecret: cache-key\ntun:\n  enable: true\n",
        )
        .unwrap();

        invalidate(&file);
        let before = parse_count_for(&file);
        let first = load_file(&file).unwrap();
        let after_first = parse_count_for(&file);
        assert_eq!(after_first, before + 1);
        assert!(first.tun_enabled);
        assert_eq!(first.proxy.as_ref().unwrap().http_port, Some(7890));
        assert!(first.controller.as_ref().unwrap().has_secret());

        // Second call without file change must hit the fingerprint cache with zero re-parse.
        let second = load_file(&file).unwrap();
        assert_eq!(parse_count_for(&file), after_first);
        assert!(Arc::ptr_eq(&first, &second));

        // Explicit invalidation forces a fresh parse on next load.
        invalidate(&file);
        fs::write(&file, "mode: direct\nmixed-port: 7899\n").unwrap();
        let third = load_file(&file).unwrap();
        assert_eq!(parse_count_for(&file), after_first + 1);
        assert!(!Arc::ptr_eq(&first, &third));
        assert!(!third.tun_enabled);
        assert_eq!(third.proxy.as_ref().unwrap().http_port, Some(7899));
        assert!(third.controller.is_none());
    }
}
