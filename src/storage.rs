use crate::error::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

pub fn check_path(path: &Path, file: bool) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || path.to_string_lossy().chars().any(char::is_control)
    {
        return Err(Error::new("error.path", path.display()));
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::new("error.path", ancestor.display()));
            }
            Ok(meta) if ancestor == path && file && !meta.is_file() => {
                return Err(Error::new("error.path", path.display()));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub fn private_dir(path: &Path) -> Result<()> {
    check_path(path, false)?;
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn mode(path: &Path) -> Result<u32> {
    #[cfg(unix)]
    {
        Ok(fs::metadata(path)?.mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(0o600)
    }
}

pub fn atomic_write(path: &Path, data: &[u8], permissions: u32) -> Result<()> {
    // POSIX mode bits are only applied on Unix; system actions are Linux-only.
    #[cfg(not(unix))]
    let _ = permissions;
    check_path(path, true)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("error.path", path.display()))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(data)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if let Ok(old) = fs::metadata(path) {
            // A replacement must retain ownership as well as permission bits.
            if unsafe { libc::fchown(temporary.as_file().as_raw_fd(), old.uid(), old.gid()) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(permissions))?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| Error::from(e.error))?;
    sync_parent(path)
}

pub fn sync_parent(path: &Path) -> Result<()> {
    // Directory fsync is only implemented for Unix; keep the file fsync on all platforms.
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    File::open(
        path.parent()
            .ok_or_else(|| Error::new("error.path", path.display()))?,
    )?
    .sync_all()?;
    Ok(())
}

pub fn remove_file(path: &Path) -> Result<()> {
    check_path(path, true)?;
    match fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub struct Lock {
    _file: File,
}
impl Drop for Lock {
    fn drop(&mut self) {
        // Closing alone can leave an inherited open-file description locked until a
        // concurrently forked process execs. Release the owner's lock explicitly.
        if let Err(error) = FileExt::unlock(&self._file) {
            eprintln!("error.lock: {error}");
        }
    }
}
impl Lock {
    pub fn acquire(path: &Path) -> Result<Self> {
        check_path(path, true)?;
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| Error::new("error.path", path.display()))?,
        )?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = options.open(path)?;
        FileExt::try_lock_exclusive(&file)
            .map_err(|e| Error::new("error.lock", format!("{}: {e}", path.display())))?;
        Ok(Self { _file: file })
    }
}

pub fn suffix(path: &Path, value: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(value);
    PathBuf::from(name)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub data: String,
    pub mode: u32,
}

impl Saved {
    pub fn new(data: &[u8], mode: u32) -> Self {
        Self {
            data: STANDARD.encode(data),
            mode,
        }
    }
    pub fn bytes(&self) -> Result<Vec<u8>> {
        if self.mode > 0o7777 {
            return Err(Error::new("error.data", "file mode"));
        }
        STANDARD
            .decode(&self.data)
            .map_err(|e| Error::new("error.data", e))
    }
    pub fn capture(path: &Path) -> Result<Option<Self>> {
        check_path(path, true)?;
        match fs::read(path) {
            Ok(data) => Ok(Some(Self::new(&data, mode(path)?))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn write(path: &Path, saved: &Option<Self>) -> Result<()> {
        match saved {
            Some(s) => atomic_write(path, &s.bytes()?, s.mode),
            None => remove_file(path),
        }
    }
}

pub fn json_saved<T: Serialize>(value: &T) -> Result<Option<Saved>> {
    Ok(Some(Saved::new(&serde_json::to_vec_pretty(value)?, 0o600)))
}

/// Durable before-images survive SIGKILL/power loss; the caller owns recovery policy.
pub fn write_transaction(
    updates: &[(PathBuf, Option<Saved>)],
    pending: &Path,
    check: impl Fn() -> Result<()>,
) -> Result<()> {
    check_path(pending, true)?;
    if pending.exists() {
        return Err(Error::new("error.pending", pending.display()));
    }
    let mut before = BTreeMap::new();
    for (path, saved) in updates {
        if path == pending || before.contains_key(path) {
            return Err(Error::new("error.path", path.display()));
        }
        if let Some(saved) = saved {
            saved.bytes()?;
        }
        before.insert(path.clone(), Saved::capture(path)?);
    }
    atomic_write(pending, &serde_json::to_vec_pretty(&before)?, 0o600)?;
    let mut changed = Vec::new();
    let result = (|| {
        for (path, saved) in updates {
            check()?;
            changed.push(path);
            Saved::write(path, saved)?;
        }
        check()?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut failures = Vec::new();
        for path in changed.into_iter().rev() {
            if let Err(e) = Saved::write(path, &before[path]) {
                failures.push(e.to_string());
            }
        }
        if !failures.is_empty() {
            return Err(Error::new(
                "error.recovery",
                format!("{error}; {}; {}", failures.join("; "), pending.display()),
            ));
        }
        remove_file(pending)?;
        return Err(error);
    }
    remove_file(pending)
}
