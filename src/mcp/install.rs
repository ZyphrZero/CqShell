//! Transactional, merge-preserving local AI client MCP installer & uninstaller for
//! Claude Code (`~/.claude.json`), Codex CLI (`~/.codex/config.toml`), Cursor
//! (`~/.cursor/mcp.json`), and any custom `mcpServers` JSON file (`--file`).

use super::config::{
    McpHttpConfig, PolicyMode, TransportMode, backups_dir, build_json_server_entry, ensure_token,
    load_config, lock_path, mcp_dir, preferred_cqs_command, validate_server_name,
};
use crate::{
    error::{Error, Result},
    runtime::Context,
    storage::{self, Lock, Saved},
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallTarget {
    Claude,
    Codex,
    Cursor,
    File(PathBuf),
}

impl InstallTarget {
    pub fn parse(client: &str, custom_file: Option<&str>) -> Result<Self> {
        match client.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claude_code" => Ok(Self::Claude),
            "codex" | "codex-cli" | "codex_cli" => Ok(Self::Codex),
            "cursor" => Ok(Self::Cursor),
            "file" | "json" | "custom" => {
                let raw = custom_file
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        Error::new(
                            "error.required",
                            "mcp_target_file (--file) is required when client=file",
                        )
                    })?;
                let path = PathBuf::from(raw);
                storage::check_path(&path, true)?;
                Ok(Self::File(path))
            }
            other => Err(Error::new(
                "error.input",
                format!("MCP install client must be claude, codex, cursor, or file: {other}"),
            )),
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::File(_) => "file",
        }
    }

    pub fn resolve_path(&self, ctx: &Context) -> PathBuf {
        match self {
            Self::Claude => ctx.paths.home.join(".claude.json"),
            Self::Codex => ctx.paths.home.join(".codex").join("config.toml"),
            Self::Cursor => ctx.paths.home.join(".cursor").join("mcp.json"),
            Self::File(path) => path.clone(),
        }
    }

    pub fn is_toml(&self) -> bool {
        matches!(self, Self::Codex)
    }
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub target: InstallTarget,
    pub transport: TransportMode,
    pub policy_override: Option<PolicyMode>,
    pub server_name: String,
    pub ssh_target_override: Option<String>,
    pub public_url_override: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub client: &'static str,
    pub path: PathBuf,
    pub server_name: String,
    pub transport: TransportMode,
    pub backup_path: Option<PathBuf>,
    pub created_token: bool,
}

#[derive(Debug, Clone)]
pub struct UninstallOutcome {
    pub client: &'static str,
    pub path: PathBuf,
    pub server_name: String,
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInstallStatus {
    pub client: &'static str,
    pub path: PathBuf,
    pub config_exists: bool,
    pub installed: bool,
    pub transport: Option<String>,
}

fn backup_file_for(ctx: &Context, target_path: &Path) -> Result<Option<PathBuf>> {
    let Some(saved) = Saved::capture(target_path)? else {
        return Ok(None);
    };
    let dir = backups_dir(ctx);
    storage::private_dir(&dir)?;
    let digest = Sha256::digest(target_path.to_string_lossy().as_bytes());
    let name = format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}.json",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5]
    );
    let backup = dir.join(name);
    let payload = serde_json::to_vec_pretty(&json!({
        "path": target_path.display().to_string(),
        "saved": saved,
    }))?;
    storage::atomic_write(&backup, &payload, 0o600)?;
    Ok(Some(backup))
}

fn build_codex_entry(
    cfg: &McpHttpConfig,
    transport: TransportMode,
    cqs_cmd: &str,
    url: &str,
    ssh_target: &str,
    policy_args: &[String],
    token: Option<&str>,
) -> toml::Table {
    let mut tbl = toml::Table::new();
    match transport {
        TransportMode::Http => {
            tbl.insert("url".into(), toml::Value::String(url.to_owned()));
            if cfg.auth_enabled
                && let Some(tok) = token
            {
                let mut headers = toml::Table::new();
                headers.insert(
                    "Authorization".into(),
                    toml::Value::String(format!("Bearer {tok}")),
                );
                tbl.insert("http_headers".into(), toml::Value::Table(headers));
            }
        }
        TransportMode::SshStdio => {
            let mut args = vec![
                toml::Value::String("-T".into()),
                toml::Value::String(ssh_target.to_owned()),
                toml::Value::String("cqs".into()),
                toml::Value::String("mcp".into()),
            ];
            args.extend(policy_args.iter().cloned().map(toml::Value::String));
            tbl.insert("command".into(), toml::Value::String("ssh".into()));
            tbl.insert("args".into(), toml::Value::Array(args));
        }
        TransportMode::LocalStdio => {
            let mut args = vec![toml::Value::String("mcp".into())];
            args.extend(policy_args.iter().cloned().map(toml::Value::String));
            tbl.insert("command".into(), toml::Value::String(cqs_cmd.to_owned()));
            tbl.insert("args".into(), toml::Value::Array(args));
        }
    }
    tbl
}

/// Merge CqShell's MCP entry into the selected local AI client config atomically.
pub fn install(ctx: &Context, opts: &InstallOptions) -> Result<InstallOutcome> {
    let server_name = validate_server_name(&opts.server_name)?;
    let target_path = opts.target.resolve_path(ctx);
    storage::check_path(&target_path, true)?;

    let mut cfg = load_config(ctx)?;
    if let Some(ref ssh) = opts.ssh_target_override
        && !ssh.trim().is_empty()
    {
        cfg.ssh_target = super::config::validate_ssh_target(ssh)?;
    }
    if let Some(ref url) = opts.public_url_override
        && !url.trim().is_empty()
    {
        cfg.public_url = super::config::validate_public_url(url)?;
    }
    cfg.validate()?;

    let mut created_token = false;
    let token = if opts.transport == TransportMode::Http && cfg.auth_enabled {
        let (tok, created) = ensure_token(ctx)?;
        created_token = created;
        Some(tok)
    } else {
        None
    };

    let dir = mcp_dir(ctx);
    storage::private_dir(&dir)?;
    let _lock = Lock::acquire(&lock_path(ctx))?;

    let backup_path = backup_file_for(ctx, &target_path)?;
    let cqs_cmd = preferred_cqs_command(ctx);
    let url = cfg.effective_http_url(None);
    let ssh_target = cfg.effective_ssh_target(None);
    let policy_args = cfg.policy_args(opts.policy_override);

    let contains_secret = token.is_some();
    let file_mode = if contains_secret {
        0o600
    } else {
        storage::mode(&target_path).unwrap_or(0o600)
    };

    if opts.target.is_toml() {
        let existing_raw = match fs::read_to_string(&target_path) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let mut root: toml::Table = if existing_raw.trim().is_empty() {
            toml::Table::new()
        } else {
            toml::from_str(&existing_raw).map_err(|e| {
                Error::new("error.config", format!("{}: {e}", target_path.display()))
            })?
        };
        let servers_val = root
            .entry("mcp_servers".to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let servers = servers_val.as_table_mut().ok_or_else(|| {
            Error::new(
                "error.config",
                format!(
                    "{}: mcp_servers must be a TOML table",
                    target_path.display()
                ),
            )
        })?;
        let entry = build_codex_entry(
            &cfg,
            opts.transport,
            &cqs_cmd,
            &url,
            &ssh_target,
            &policy_args,
            token.as_deref(),
        );
        servers.insert(server_name.clone(), toml::Value::Table(entry));
        let rendered = toml::to_string_pretty(&root)?;
        storage::atomic_write(&target_path, rendered.as_bytes(), file_mode)?;
    } else {
        let existing_raw = match fs::read_to_string(&target_path) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => "{}".into(),
            Err(e) => return Err(e.into()),
        };
        let mut root: Value = if existing_raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&existing_raw).map_err(|e| {
                Error::new("error.config", format!("{}: {e}", target_path.display()))
            })?
        };
        let obj = root.as_object_mut().ok_or_else(|| {
            Error::new(
                "error.config",
                format!("{}: JSON root must be an object", target_path.display()),
            )
        })?;
        let servers_val = obj
            .entry("mcpServers".to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        let servers = servers_val.as_object_mut().ok_or_else(|| {
            Error::new(
                "error.config",
                format!(
                    "{}: 'mcpServers' must be a JSON object",
                    target_path.display()
                ),
            )
        })?;
        let entry = build_json_server_entry(
            &cfg,
            opts.transport,
            &cqs_cmd,
            &url,
            &ssh_target,
            &policy_args,
            token.as_deref(),
        );
        servers.insert(server_name.clone(), entry);
        let mut rendered = serde_json::to_string_pretty(&root)?;
        rendered.push('\n');
        storage::atomic_write(&target_path, rendered.as_bytes(), file_mode)?;
    }

    Ok(InstallOutcome {
        client: opts.target.id(),
        path: target_path,
        server_name,
        transport: opts.transport,
        backup_path,
        created_token,
    })
}

/// Remove only CqShell's named entry from the selected local AI client config atomically.
pub fn uninstall(
    ctx: &Context,
    target: &InstallTarget,
    server_name: &str,
) -> Result<UninstallOutcome> {
    let server_name = validate_server_name(server_name)?;
    let target_path = target.resolve_path(ctx);
    storage::check_path(&target_path, true)?;

    let dir = mcp_dir(ctx);
    storage::private_dir(&dir)?;
    let _lock = Lock::acquire(&lock_path(ctx))?;

    let existing_raw = match fs::read_to_string(&target_path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(UninstallOutcome {
                client: target.id(),
                path: target_path,
                server_name,
                removed: false,
            });
        }
        Err(e) => return Err(e.into()),
    };
    let file_mode = storage::mode(&target_path).unwrap_or(0o600);

    let removed = if target.is_toml() {
        let mut root: toml::Table = toml::from_str(&existing_raw)
            .map_err(|e| Error::new("error.config", format!("{}: {e}", target_path.display())))?;
        let did_remove = root
            .get_mut("mcp_servers")
            .and_then(toml::Value::as_table_mut)
            .is_some_and(|tbl| tbl.remove(&server_name).is_some());
        if did_remove {
            let _ = backup_file_for(ctx, &target_path)?;
            let rendered = toml::to_string_pretty(&root)?;
            storage::atomic_write(&target_path, rendered.as_bytes(), file_mode)?;
        }
        did_remove
    } else {
        let mut root: Value = serde_json::from_str(&existing_raw)
            .map_err(|e| Error::new("error.config", format!("{}: {e}", target_path.display())))?;
        let did_remove = root
            .as_object_mut()
            .and_then(|o| o.get_mut("mcpServers"))
            .and_then(Value::as_object_mut)
            .is_some_and(|servers| servers.remove(&server_name).is_some());
        if did_remove {
            let _ = backup_file_for(ctx, &target_path)?;
            let mut rendered = serde_json::to_string_pretty(&root)?;
            rendered.push('\n');
            storage::atomic_write(&target_path, rendered.as_bytes(), file_mode)?;
        }
        did_remove
    };

    Ok(UninstallOutcome {
        client: target.id(),
        path: target_path,
        server_name,
        removed,
    })
}

/// Inspect standard local AI client configuration files without reading or exposing secrets.
pub fn detect_installations(ctx: &Context, server_name: &str) -> Vec<ClientInstallStatus> {
    let mut out = Vec::new();
    for target in [
        InstallTarget::Claude,
        InstallTarget::Codex,
        InstallTarget::Cursor,
    ] {
        let path = target.resolve_path(ctx);
        let config_exists = path.is_file();
        let mut installed = false;
        let mut transport = None;
        if config_exists
            && storage::check_path(&path, true).is_ok()
            && let Ok(raw) = fs::read_to_string(&path)
        {
            if target.is_toml() {
                if let Ok(root) = toml::from_str::<toml::Table>(&raw)
                    && let Some(entry) = root
                        .get("mcp_servers")
                        .and_then(toml::Value::as_table)
                        .and_then(|m| m.get(server_name))
                        .and_then(toml::Value::as_table)
                {
                    installed = true;
                    transport = Some(if entry.contains_key("url") {
                        "http".into()
                    } else if entry.get("command").and_then(toml::Value::as_str) == Some("ssh") {
                        "ssh_stdio".into()
                    } else {
                        "local_stdio".into()
                    });
                }
            } else if let Ok(root) = serde_json::from_str::<Value>(&raw)
                && let Some(entry) = root
                    .get("mcpServers")
                    .and_then(Value::as_object)
                    .and_then(|m| m.get(server_name))
                    .and_then(Value::as_object)
            {
                installed = true;
                transport = Some(if entry.contains_key("url") {
                    "http".into()
                } else if entry.get("command").and_then(Value::as_str) == Some("ssh") {
                    "ssh_stdio".into()
                } else {
                    "local_stdio".into()
                });
            }
        }
        out.push(ClientInstallStatus {
            client: target.id(),
            path,
            config_exists,
            installed,
            transport,
        });
    }
    out
}

/// Action implementation for `ai.mcp.install`.
pub fn action_install(
    ctx: &Context,
    client: &str,
    transport: &str,
    policy: &str,
    server_name: &str,
    target_file: &str,
) -> Result<()> {
    let target = InstallTarget::parse(client, (!target_file.is_empty()).then_some(target_file))?;
    let transport = TransportMode::parse(transport)?;
    let policy_override = Some(PolicyMode::parse(policy)?);
    let outcome = install(
        ctx,
        &InstallOptions {
            target,
            transport,
            policy_override,
            server_name: server_name.to_owned(),
            ssh_target_override: None,
            public_url_override: None,
        },
    )?;
    ctx.message(
        "notice.mcp_installed",
        format!(
            "{} ({}) -> {} [{}]",
            outcome.client,
            outcome.transport.as_str(),
            outcome.path.display(),
            outcome.server_name
        ),
    );
    if let Some(backup) = outcome.backup_path {
        ctx.message("notice.backup", backup.display());
    }
    if outcome.created_token {
        ctx.message(
            "notice.mcp_token_ready",
            super::config::token_path(ctx).display(),
        );
    }
    Ok(())
}

/// Action implementation for `ai.mcp.uninstall`.
pub fn action_uninstall(
    ctx: &Context,
    client: &str,
    server_name: &str,
    target_file: &str,
) -> Result<()> {
    let target = InstallTarget::parse(client, (!target_file.is_empty()).then_some(target_file))?;
    let outcome = uninstall(ctx, &target, server_name)?;
    if outcome.removed {
        ctx.message(
            "notice.mcp_uninstalled",
            format!(
                "{} [{}] <- {}",
                outcome.client,
                outcome.server_name,
                outcome.path.display()
            ),
        );
    } else {
        ctx.message(
            "notice.mcp_not_registered",
            format!(
                "{} [{}] ({})",
                outcome.client,
                outcome.server_name,
                outcome.path.display()
            ),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_uninstall_preserve_existing_servers_for_claude_codex_cursor_and_file() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());

        // Pre-populate ~/.claude.json with an unrelated server and setting.
        let claude_path = ctx.paths.home.join(".claude.json");
        fs::write(
            &claude_path,
            r#"{"theme":"dark","mcpServers":{"other":{"command":"echo","args":["hi"]}}}"#,
        )
        .unwrap();

        let out = install(
            &ctx,
            &InstallOptions {
                target: InstallTarget::Claude,
                transport: TransportMode::LocalStdio,
                policy_override: Some(PolicyMode::ReadOnly),
                server_name: "cqs".into(),
                ssh_target_override: None,
                public_url_override: None,
            },
        )
        .unwrap();
        assert!(out.backup_path.is_some());
        let saved: Value =
            serde_json::from_str(&fs::read_to_string(&claude_path).unwrap()).unwrap();
        assert_eq!(saved["theme"], "dark");
        assert_eq!(saved["mcpServers"]["other"]["command"], "echo");
        assert!(
            saved["mcpServers"]["cqs"]["args"]
                .as_array()
                .unwrap()
                .contains(&json!("--read-only"))
        );

        // Pre-populate ~/.codex/config.toml and install HTTP transport (creates token).
        let codex_path = ctx.paths.home.join(".codex/config.toml");
        fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
        fs::write(
            &codex_path,
            "model = \"gpt-5\"\n\n[mcp_servers.other]\ncommand = \"echo\"\n",
        )
        .unwrap();
        let codex_out = install(
            &ctx,
            &InstallOptions {
                target: InstallTarget::Codex,
                transport: TransportMode::Http,
                policy_override: None,
                server_name: "cqs".into(),
                ssh_target_override: None,
                public_url_override: Some("https://mcp.example.com/mcp".into()),
            },
        )
        .unwrap();
        assert!(codex_out.created_token);
        let codex_toml: toml::Table =
            toml::from_str(&fs::read_to_string(&codex_path).unwrap()).unwrap();
        assert_eq!(codex_toml["model"].as_str(), Some("gpt-5"));
        assert!(codex_toml["mcp_servers"]["other"].is_table());
        assert_eq!(
            codex_toml["mcp_servers"]["cqs"]["url"].as_str(),
            Some("https://mcp.example.com/mcp")
        );
        assert!(
            codex_toml["mcp_servers"]["cqs"]["http_headers"]["Authorization"]
                .as_str()
                .unwrap()
                .starts_with("Bearer cqs_mcp_")
        );

        // Cursor + custom file
        let custom_file = temp.path().join("antigravity_mcp.json");
        install(
            &ctx,
            &InstallOptions {
                target: InstallTarget::File(custom_file.clone()),
                transport: TransportMode::SshStdio,
                policy_override: Some(PolicyMode::Standard),
                server_name: "prod_vps".into(),
                ssh_target_override: Some("admin@server.example.com".into()),
                public_url_override: None,
            },
        )
        .unwrap();
        let custom_json: Value =
            serde_json::from_str(&fs::read_to_string(&custom_file).unwrap()).unwrap();
        assert_eq!(custom_json["mcpServers"]["prod_vps"]["command"], "ssh");

        // Detect installations reports Claude & Codex installed, Cursor absent.
        let detected = detect_installations(&ctx, "cqs");
        assert_eq!(detected.len(), 3);
        assert!(detected[0].installed && detected[0].transport.as_deref() == Some("local_stdio"));
        assert!(detected[1].installed && detected[1].transport.as_deref() == Some("http"));
        assert!(!detected[2].installed);

        // Uninstall removes only cqs and preserves unrelated entries.
        let u1 = uninstall(&ctx, &InstallTarget::Claude, "cqs").unwrap();
        assert!(u1.removed);
        let after: Value =
            serde_json::from_str(&fs::read_to_string(&claude_path).unwrap()).unwrap();
        assert_eq!(after["theme"], "dark");
        assert!(after["mcpServers"]["other"].is_object());
        assert!(after["mcpServers"].get("cqs").is_none());

        let u2 = uninstall(&ctx, &InstallTarget::Codex, "cqs").unwrap();
        assert!(u2.removed);
        let codex_after: toml::Table =
            toml::from_str(&fs::read_to_string(&codex_path).unwrap()).unwrap();
        assert_eq!(codex_after["model"].as_str(), Some("gpt-5"));
        assert!(codex_after["mcp_servers"]["other"].is_table());
        assert!(
            codex_after["mcp_servers"]
                .as_table()
                .unwrap()
                .get("cqs")
                .is_none()
        );
    }
}
