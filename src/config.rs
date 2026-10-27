use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub language: Option<String>,
    pub locale_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub category_order: Vec<String>,
}

impl Preferences {
    pub fn read(path: &Path, explicit: bool) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(s) => Ok(toml::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => Ok(Self::default()),
            Err(e) => Err(Error::new(
                "error.config",
                format!("{}: {e}", path.display()),
            )),
        }
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let contents = toml::to_string(self)?;
        crate::storage::atomic_write(path, contents.as_bytes(), 0o600)
    }
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub state: PathBuf,
    pub preferences: PathBuf,
    pub terminal: PathBuf,
    pub bashrc: PathBuf,
    pub zshrc: PathBuf,
    pub bbr: PathBuf,
    pub tcp: PathBuf,
    pub kernel_lock: PathBuf,
    pub kernel_backup: PathBuf,
    pub boot: PathBuf,
    pub install: PathBuf,
    pub ssh: PathBuf,
    pub docker: PathBuf,
    pub clash_home: PathBuf,
    pub clash_data: PathBuf,
    pub clash_resources: PathBuf,
    pub clash_binary: PathBuf,
    pub clash_config: PathBuf,
    pub clash_mixin: PathBuf,
    pub clash_runtime: PathBuf,
    pub clash_profiles: PathBuf,
    pub clash_pid: PathBuf,
    pub clash_pid_start: PathBuf,
    pub clash_log: PathBuf,
    pub clash_lock: PathBuf,
}

fn env_path(name: &str, default: PathBuf) -> PathBuf {
    env::var_os(name)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default)
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .ok_or_else(|| Error::new("error.config", "HOME"))?;
        let config = env_path("XDG_CONFIG_HOME", home.join(".config")).join("cqs");
        let clash_home = env_path(
            "CQS_CLASH_HOME",
            env_path("XDG_DATA_HOME", home.join(".local/share")).join("cqs/clash"),
        );
        let clash_data = env_path("CQS_CLASH_DATA_DIR", clash_home.join("data"));
        let clash_resources = env_path("CQS_CLASH_RESOURCES_DIR", clash_home.join("resources"));
        let clash_binary = env_path("CQS_CLASH_BINARY", clash_home.join("bin").join("mihomo"));
        Ok(Self {
            state: env_path(
                "CQS_STATE_DIR",
                env_path("XDG_STATE_HOME", home.join(".local/state")).join("cqs"),
            ),
            preferences: config.join("config.toml"),
            terminal: env_path("CQS_TERMINAL_CONFIG", config.join("terminal.toml")),
            bashrc: home.join(".bashrc"),
            zshrc: env_path("ZDOTDIR", home.clone()).join(".zshrc"),
            bbr: env_path("CQS_BBR_CONFIG", "/etc/sysctl.d/99-cqs-bbr.conf".into()),
            tcp: env_path("CQS_TCP_CONFIG", "/etc/sysctl.d/99-cqs-tcp.conf".into()),
            kernel_lock: env_path("CQS_KERNEL_LOCK", "/run/lock/cqs-bbrv3.lock".into()),
            kernel_backup: env_path("CQS_KERNEL_BACKUP", "/var/backups/cqs-bbrv3".into()),
            boot: "/boot".into(),
            install: env_path("CQS_INSTALL_PATH", "/usr/local/bin/cqs".into()),
            ssh: "/etc/ssh/sshd_config".into(),
            docker: "/etc/docker/daemon.json".into(),
            clash_config: env_path("CQS_CLASH_CONFIG", clash_data.join("config.yaml")),
            clash_mixin: env_path("CQS_CLASH_MIXIN", clash_data.join("mixin.yaml")),
            clash_runtime: env_path("CQS_CLASH_RUNTIME", clash_data.join("runtime.yaml")),
            clash_profiles: env_path("CQS_CLASH_PROFILES", clash_data.join("profiles.yaml")),
            clash_pid: env_path("CQS_CLASH_PID", clash_data.join("mihomo.pid")),
            clash_pid_start: env_path("CQS_CLASH_PID_START", clash_data.join("mihomo.start")),
            clash_log: env_path("CQS_CLASH_LOG", clash_data.join("mihomo.log")),
            clash_lock: env_path("CQS_CLASH_LOCK", clash_data.join("clash.lock")),
            clash_home,
            clash_data,
            clash_resources,
            clash_binary,
            home,
        })
    }
}
