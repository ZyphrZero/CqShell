use crate::{
    error::{Error, Result},
    runtime::Context,
    storage::{self, Saved},
};
use std::{fs, path::Path};

/// Caller holds the configuration lock across reading, candidate generation and commit.
pub fn update(
    ctx: &Context,
    target: &Path,
    data: &[u8],
    service: &str,
    was_active: bool,
    validate: impl FnOnce(&Path) -> Result<()>,
    activate: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let pending = storage::suffix(target, ".cqs-pending.json");
    storage::check_path(&pending, true)?;
    if pending.exists() {
        return Err(Error::new("error.pending", pending.display()));
    }
    let original = Saved::capture(target)?;
    fs::create_dir_all(target.parent().unwrap())?;
    let candidate = tempfile::NamedTempFile::new_in(target.parent().unwrap())?;
    fs::write(candidate.path(), data)?;
    validate(candidate.path())?;
    ctx.check()?;
    storage::atomic_write(
        &pending,
        &serde_json::to_vec_pretty(
            &serde_json::json!({"file": original, "service": service, "was_active": was_active}),
        )?,
        0o600,
    )?;
    let result = (|| {
        storage::atomic_write(target, data, original.as_ref().map_or(0o644, |s| s.mode))?;
        activate()?;
        ctx.check()
    })();
    if let Err(error) = result {
        let recovery = ctx.recovery();
        let restored = Saved::write(target, &original).and_then(|_| {
            recovery.run(
                "systemctl",
                &[if was_active { "restart" } else { "stop" }, service],
            )?;
            if recovery.active(service)? != was_active {
                return Err(Error::new("error.verify", service));
            }
            Ok(())
        });
        if let Err(e) = restored {
            return Err(error.with_recovery(e, &pending));
        }
        storage::remove_file(&pending)?;
        return Err(error);
    }
    storage::remove_file(&pending)?;
    ctx.message("notice.saved", target.display());
    Ok(())
}
