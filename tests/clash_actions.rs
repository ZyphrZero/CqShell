use std::{
    fs,
    process::{Command, Stdio},
};

#[test]
fn clash_controls_have_bilingual_cli_metadata_without_writing_state() {
    for locale in ["zh-CN", "en-US"] {
        let temp = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_cqs"))
            .env("HOME", temp.path())
            .env("USERPROFILE", temp.path())
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env_remove("CQS_LANG")
            .stdin(Stdio::null())
            .args(["--lang", locale, "list", "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let rows = rows.as_array().unwrap();
        for (id, mutates) in [
            ("controller.status", false),
            ("controller.configure", true),
            ("proxies", false),
            ("proxy.select", true),
            ("proxy.delay", false),
            ("connections", true),
            ("mode", true),
            ("reload", true),
            ("secret.generate", true),
            ("secret.set", true),
            ("secret.clear", true),
            ("panel.status", false),
            ("panel.install", true),
            ("panel.remove", true),
            ("geodata.status", false),
            ("geodata.update", true),
            ("settings.configure", true),
            ("dns.status", false),
            ("dns.configure", true),
            ("dns.query", false),
            ("cache.flush", true),
            ("mixin.status", false),
            ("mixin.reset", true),
            ("socks.status", false),
            ("socks.configure", true),
            ("socks.auth.generate", true),
            ("socks.auth.set", true),
            ("tun.status", false),
            ("tun.configure", true),
            ("logs", false),
            ("service.status", false),
            ("service.install", true),
            ("service.remove", true),
        ] {
            let id = format!("clash.{id}");
            let row = rows.iter().find(|row| row["id"] == id).expect(&id);
            assert_eq!(row["group"], "clash");
            assert_eq!(row["mutates"], mutates);
            assert_eq!(row["root"], false); // TUN enable/system scope enforce privileges at execution.
            assert!(
                row["label"]
                    .as_str()
                    .is_some_and(|label| !label.starts_with("action."))
            );
        }
        let action = |id: &str| rows.iter().find(|row| row["id"] == id).unwrap();
        let field = |id: &str, name: &str| {
            action(id)["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .find(|field| field["name"] == name)
                .unwrap()
        };
        assert_eq!(
            field("clash.panel.install", "panel")["default"],
            "zashboard"
        );
        assert_eq!(
            field("clash.panel.install", "panel")["choices"],
            serde_json::json!(["zashboard", "metacubexd", "yacd", "razord"])
        );
        assert_eq!(
            field("clash.geodata.update", "geodata_preset")["choices"],
            serde_json::json!(["standard", "lite", "full"])
        );
        assert_eq!(field("clash.tun.configure", "enabled")["default"], "off");
        assert_eq!(
            field("clash.tun.configure", "tun_stack")["choices"],
            serde_json::json!(["system", "gvisor", "mixed"])
        );
        assert_eq!(
            field("clash.tun.configure", "tun_bypass_private")["default"],
            "on"
        );
        assert_eq!(
            field("clash.tun.configure", "tun_strict_route")["default"],
            "off"
        );
        assert_eq!(field("clash.stop", "stop_force")["default"], "off");
        assert_eq!(
            field("clash.settings.configure", "tcp_concurrent")["default"],
            "on"
        );
        assert_eq!(
            field("clash.dns.configure", "dns_preset")["choices"],
            serde_json::json!(["china", "global", "keep"])
        );
        assert_eq!(field("clash.logs", "follow")["default"], "off");
        assert_eq!(
            field("clash.service.install", "service_scope")["default"],
            "user"
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}
