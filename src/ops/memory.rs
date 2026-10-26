use crate::error::{Error, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

fn unescape(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            let code = bytes
                .get(index + 1..index + 4)
                .ok_or_else(|| Error::new("error.memory", value))?;
            if !code.iter().all(|b| (b'0'..=b'7').contains(b)) {
                return Err(Error::new("error.memory", value));
            }
            let n = (code[0] - b'0') as u16 * 64
                + (code[1] - b'0') as u16 * 8
                + (code[2] - b'0') as u16;
            out.push(u8::try_from(n).map_err(|_| Error::new("error.memory", value))?);
            index += 4;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|e| Error::new("error.memory", e))
}

pub fn physical_kb(proc: &Path) -> Result<u64> {
    let text = fs::read_to_string(proc.join("meminfo"))?;
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.first() == Some(&"MemTotal:") && fields.len() == 3 && fields[2] == "kB" {
            return fields[1].parse().map_err(|e| Error::new("error.memory", e));
        }
    }
    Err(Error::new("error.memory", "MemTotal"))
}

pub fn effective_kb(proc: &Path) -> Result<u64> {
    let mut memory = physical_kb(proc)?;
    let mut memberships = BTreeMap::new();
    for line in fs::read_to_string(proc.join("self/cgroup"))?.lines() {
        let fields: Vec<_> = line.splitn(3, ':').collect();
        if fields.len() != 3 {
            return Err(Error::new("error.memory", line));
        }
        if fields[0] == "0" && fields[1].is_empty() {
            memberships.insert("cgroup2", fields[2].to_owned());
        } else if fields[1].split(',').any(|v| v == "memory") {
            memberships.insert("cgroup", fields[2].to_owned());
        }
    }
    let mut located = BTreeSet::new();
    for line in fs::read_to_string(proc.join("self/mountinfo"))?.lines() {
        let (left, right) = line
            .split_once(" - ")
            .ok_or_else(|| Error::new("error.memory", line))?;
        let left: Vec<_> = left.split_whitespace().collect();
        let right: Vec<_> = right.split_whitespace().collect();
        if left.len() < 5 || right.len() < 3 {
            return Err(Error::new("error.memory", line));
        }
        let kind = right[0];
        let Some(group) = memberships.get(kind) else {
            continue;
        };
        if kind == "cgroup" && !right[2].split(',').any(|v| v == "memory") {
            continue;
        }
        let group = Path::new(group);
        if !group.is_absolute() || group.components().any(|p| p == Component::ParentDir) {
            return Err(Error::new("error.memory", group.display()));
        }
        let root = PathBuf::from(unescape(left[3])?);
        let mount = PathBuf::from(unescape(left[4])?);
        let Ok(relative) = group.strip_prefix(&root) else {
            continue;
        };
        let mut directory = mount.join(relative);
        if !directory.is_dir() {
            continue;
        }
        located.insert(kind.to_owned());
        let filename = if kind == "cgroup2" {
            "memory.max"
        } else {
            "memory.limit_in_bytes"
        };
        loop {
            let path = directory.join(filename);
            match fs::read_to_string(&path) {
                Ok(value) => {
                    let value = value.trim();
                    if value != "max" {
                        if value.is_empty() || !value.bytes().all(|v| v.is_ascii_digit()) {
                            return Err(Error::new("error.memory", path.display()));
                        }
                        let value: u128 =
                            value.parse().map_err(|e| Error::new("error.memory", e))?;
                        memory = (memory as u128).min(value / 1024) as u64;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
            if directory == mount {
                break;
            }
            if !directory.pop() || !directory.starts_with(&mount) {
                return Err(Error::new("error.memory", mount.display()));
            }
        }
    }
    if memberships.keys().any(|kind| !located.contains(*kind)) {
        return Err(Error::new("error.memory", "cgroup mount"));
    }
    Ok(memory)
}

fn milli(value: &str, max: u64) -> Result<u64> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() > 2
        || parts
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
        || parts.first().is_none_or(|s| s.len() > 6)
        || parts.get(1).is_some_and(|s| s.len() > 3)
    {
        return Err(Error::new("error.input", value));
    }
    let whole: u64 = parts[0].parse().map_err(|e| Error::new("error.input", e))?;
    let fraction: u64 = match parts.get(1) {
        Some(s) => format!("{s:0<3}")
            .parse()
            .map_err(|e| Error::new("error.input", e))?,
        None => 0,
    };
    let amount = whole * 1000 + fraction;
    if amount == 0 || amount > max * 1000 {
        return Err(Error::new("error.input", value));
    }
    Ok(amount)
}

pub fn tcp_buffer(bandwidth: &str, rtt: &str, memory_kb: u64) -> Result<u64> {
    let bw = milli(bandwidth, 100000)? as u128;
    let rtt = milli(rtt, 10000)? as u128;
    if memory_kb < 65536 {
        return Err(Error::new("error.memory", memory_kb));
    }
    let mib = 1048576u128;
    let candidate = (bw * rtt).div_ceil(4000 * mib).max(1) * mib;
    let cap = ((memory_kb as u128 * 1024 / 32 / mib) * mib).min(64 * mib);
    Ok(candidate.min(cap) as u64)
}
