use super::*;
use crate::tests::context;
use serde_json::json;

fn release(tool: Tool, arch: Architecture, hash: &str, bytes: u64) -> Value {
    let (tag, name) = match tool {
        Tool::Pi => ("v1.2.3", format!("pi-{}.tar.gz", arch.platform())),
        Tool::Codex => (
            "rust-v1.2.3",
            format!("codex-package-{}.tar.gz", arch.codex_target()),
        ),
        Tool::Claude => unreachable!(),
    };
    let repo = tool.repository().unwrap();
    json!({"tag_name":tag,"draft":false,"prerelease":false,"assets":[{
        "name":name,"size":bytes,"digest":format!("sha256:{hash}"),
        "browser_download_url":format!("https://github.com/{repo}/releases/download/{tag}/{name}")
    }]})
}

#[test]
fn github_assets_require_stable_versions_exact_sources_and_checksums() {
    for tool in [Tool::Pi, Tool::Codex] {
        for arch in [Architecture::X64, Architecture::Arm64] {
            let good = release(tool, arch, &"a".repeat(64), 100);
            let package = github_package(tool, arch, &good).unwrap();
            assert!(package.url.ends_with(".tar.gz"));
            assert_eq!(package.version, "1.2.3");
            for mutation in [
                "digest",
                "url",
                "tag",
                "draft",
                "prerelease",
                "size",
                "duplicate",
                "missing",
            ] {
                let mut bad = good.clone();
                match mutation {
                    "digest" => bad["assets"][0]["digest"] = Value::Null,
                    "url" => {
                        bad["assets"][0]["browser_download_url"] =
                            "https://example.org/package".into()
                    }
                    "tag" => bad["tag_name"] = "v1.2.3/../other".into(),
                    "draft" => bad["draft"] = true.into(),
                    "prerelease" => bad["prerelease"] = true.into(),
                    "size" => bad["assets"][0]["size"] = (MAX_FILE + 1).into(),
                    "duplicate" => {
                        let asset = bad["assets"][0].clone();
                        bad["assets"].as_array_mut().unwrap().push(asset);
                    }
                    _ => bad["assets"] = json!([]),
                }
                assert!(github_package(tool, arch, &bad).is_err(), "{mutation}");
            }
        }
    }
    assert!(Architecture::parse("armv7l").is_err());
}

#[test]
fn claude_manifest_requires_a_matching_platform_and_valid_digest() {
    for arch in [Architecture::X64, Architecture::Arm64] {
        let manifest = json!({"version":"1.2.3","platforms":{arch.platform():{"checksum":"b".repeat(64),"size":100}}});
        let package = claude_package(arch, "1.2.3", &manifest).unwrap();
        assert_eq!(
            package.url,
            format!("{CLAUDE_RELEASES}/1.2.3/{}/claude", arch.platform())
        );
        assert!(claude_package(arch, "2.0.0", &manifest).is_err());
        assert!(claude_package(arch, "../1.2.3", &manifest).is_err());
        assert!(claude_package(arch, "1.2.3", &json!({"platforms":{}})).is_err());
        let mut invalid = manifest;
        invalid["platforms"][arch.platform()]["checksum"] = "bad".into();
        assert!(claude_package(arch, "1.2.3", &invalid).is_err());
    }
}

fn tarball(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (name, kind, data) in entries {
        let mut header = tar::Header::new_gnu();
        // Write raw names so traversal fixtures are not sanitized by the builder.
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_entry_type(*kind);
        header.set_mode(0o4755);
        header.set_size(data.len() as u64);
        if kind.is_symlink() || kind.is_hard_link() {
            header.set_link_name("outside").unwrap();
        }
        header.set_cksum();
        builder.append(&header, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn unpack_rejects_traversal_links_special_files_and_duplicates() {
    for entries in [
        vec![("../escape", tar::EntryType::Regular, &b"x"[..])],
        vec![("/escape", tar::EntryType::Regular, &b"x"[..])],
        vec![("link", tar::EntryType::Symlink, &b""[..])],
        vec![("link", tar::EntryType::Link, &b""[..])],
        vec![("pipe", tar::EntryType::Fifo, &b""[..])],
        vec![
            ("same", tar::EntryType::Regular, &b"x"[..]),
            ("same", tar::EntryType::Regular, &b"y"[..]),
        ],
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("out");
        fs::create_dir(&root).unwrap();
        let archive = temp.path().join("package.tar.gz");
        fs::write(&archive, tarball(&entries)).unwrap();
        assert!(unpack(&context(temp.path()), &archive, &root).is_err());
        assert!(!temp.path().join("escape").exists());
    }
}

#[test]
fn checksum_and_elf_checks_reject_corruption_scripts_and_wrong_architecture() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("binary");
    fs::write(&file, b"#!/bin/sh\necho 1.2.3\n").unwrap();
    assert!(elf(&file, Architecture::X64).is_err());
    let mut header = [0u8; 20];
    header[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    header[16] = 3;
    header[18] = 62;
    fs::write(&file, header).unwrap();
    elf(&file, Architecture::X64).unwrap();
    assert!(elf(&file, Architecture::Arm64).is_err());
    let package = claude_package(
        Architecture::X64,
        "1.2.3",
        &json!({"platforms":{"linux-x64":{"checksum":sha256(&file).unwrap(),"size":20}}}),
    )
    .unwrap();
    validate_download(&package, &file).unwrap();
    header[19] = 1;
    fs::write(&file, header).unwrap();
    assert_eq!(
        validate_download(&package, &file).unwrap_err().key,
        "error.digest"
    );
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        process::Command,
    };

    fn executable(path: &Path, bytes: impl AsRef<[u8]>) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn binary(root: &Path) -> PathBuf {
        let source = root.join("fixture.c");
        fs::write(
            &source,
            r#"#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(int argc, char **argv) {
    if (argc != 2 || strcmp(argv[1], "--version")) return 91;
    const char *fail = getenv("FAIL_TARGET");
    if (fail && !strcmp(argv[0], fail)) return 23;
    const char *version = getenv("FIXTURE_VERSION");
    puts(version ? version : "1.2.3");
    return 0;
}
"#,
        )
        .unwrap();
        let output = root.join("fixture-binary");
        let result = Command::new("cc")
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        output
    }

    fn boundary(root: &Path) -> Context {
        let bin = root.join("commands");
        fs::create_dir_all(&bin).unwrap();
        // Network boundary only: extraction, ELF validation, execution and atomic
        // publication are real. This fixture cannot claim an upstream install passed.
        executable(
            &bin.join("curl"),
            r#"#!/usr/bin/python3
import json, os, pathlib, shutil, sys
root = pathlib.Path(os.environ['CQS_TEST_ROOT'])
args = sys.argv[1:]
assert args[args.index('--proto') + 1] == '=https'
assert args[args.index('--proto-redir') + 1] == '=https'
url = args[-1]
with (root / 'calls').open('a') as f: f.write(url + '\n')
if (root / 'fail-download').exists(): sys.exit(22)
routes = json.loads((root / 'routes.json').read_text())
shutil.copyfile(routes[url], args[args.index('--output') + 1])
"#,
        );
        let mut ctx = context(root);
        ctx.interactive = false;
        ctx.environment
            .insert("PATH".into(), bin.display().to_string());
        ctx.environment
            .insert("CQS_TEST_ROOT".into(), root.display().to_string());
        ctx.environment.insert("SUDO_USER".into(), "root".into());
        ctx.environment.insert(
            "CODEX_INSTALL_DIR".into(),
            root.join(".local/bin").display().to_string(),
        );
        ctx
    }

    fn routes(root: &Path, tool: Tool, payload: &[u8]) {
        let arch = Architecture::parse(std::env::consts::ARCH).unwrap();
        let asset = root.join("asset");
        fs::write(&asset, payload).unwrap();
        let hash = sha256(&asset).unwrap();
        let metadata = root.join("metadata.json");
        let (package, mut routes) = if tool == Tool::Claude {
            let manifest = json!({"version":"1.2.3","platforms":{arch.platform():{"checksum":hash,"size":payload.len()}}});
            fs::write(&metadata, serde_json::to_vec(&manifest).unwrap()).unwrap();
            let latest = root.join("latest");
            fs::write(&latest, "1.2.3\n").unwrap();
            (
                claude_package(arch, "1.2.3", &manifest).unwrap(),
                json!({
                    format!("{CLAUDE_RELEASES}/latest"):latest,
                    format!("{CLAUDE_RELEASES}/1.2.3/manifest.json"):metadata
                }),
            )
        } else {
            let release = release(tool, arch, &hash, payload.len() as u64);
            fs::write(&metadata, serde_json::to_vec(&release).unwrap()).unwrap();
            (
                github_package(tool, arch, &release).unwrap(),
                json!({
                    format!("https://api.github.com/repos/{}/releases/latest", tool.repository().unwrap()):metadata
                }),
            )
        };
        routes[package.url] = json!(asset);
        fs::write(
            root.join("routes.json"),
            serde_json::to_vec(&routes).unwrap(),
        )
        .unwrap();
    }

    fn payload(tool: Tool, binary: &[u8]) -> Vec<u8> {
        if tool == Tool::Claude {
            return binary.to_vec();
        }
        let arch = Architecture::parse(std::env::consts::ARCH).unwrap();
        let metadata = if tool == Tool::Pi {
            json!({"version":"1.2.3"})
        } else {
            json!({"layoutVersion":1,"version":"1.2.3","target":arch.codex_target(),"variant":"codex","entrypoint":"bin/codex","resourcesDir":"codex-resources","pathDir":"codex-path"})
        };
        let metadata = serde_json::to_vec(&metadata).unwrap();
        let file = tar::EntryType::Regular;
        let directory = tar::EntryType::Directory;
        let entries = if tool == Tool::Pi {
            vec![
                ("pi/pi", file, binary),
                ("pi/package.json", file, metadata.as_slice()),
                ("pi/photon_rs_bg.wasm", file, &b"wasm"[..]),
                ("pi/theme", directory, &b""[..]),
                ("pi/assets", directory, &b""[..]),
                ("pi/export-html", directory, &b""[..]),
            ]
        } else {
            vec![
                ("bin/codex", file, binary),
                ("codex-package.json", file, metadata.as_slice()),
                ("bin/codex-code-mode-host", file, binary),
                ("codex-resources/bwrap", file, binary),
                ("codex-path/rg", file, binary),
            ]
        };
        tarball(&entries)
    }

    #[test]
    fn all_tools_install_and_update_without_node_npm_shells_or_a_terminal() {
        for tool in [Tool::Pi, Tool::Claude, Tool::Codex] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("user's home");
            fs::create_dir(&root).unwrap();
            let ctx = boundary(&root);
            let bytes = fs::read(binary(&root)).unwrap();
            routes(&root, tool, &payload(tool, &bytes));
            let target = launcher(&ctx, tool);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, "previous unmanaged binary").unwrap();
            install(&ctx, tool).unwrap();
            let first = fs::read_link(&target).unwrap();
            assert_eq!(fs::read(&first).unwrap(), bytes);
            let backups: Vec<_> = fs::read_dir(target.parent().unwrap())
                .unwrap()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".cqs-ai-backup-")
                })
                .collect();
            assert_eq!(
                fs::read(backups[0].path().join("previous")).unwrap(),
                b"previous unmanaged binary"
            );
            install(&ctx, tool).unwrap();
            assert_ne!(fs::read_link(&target).unwrap(), first);
            assert!(first.is_file());
            assert!(ctx.require(&["node"]).is_err());
            assert!(ctx.require(&["npm"]).is_err());
            assert!(ctx.require(&["bash"]).is_err());
            let calls = fs::read_to_string(root.join("calls")).unwrap();
            assert!(!calls.contains("install.sh"));
            assert!(!calls.contains("npm"));
            status(&ctx).unwrap();
        }
    }

    #[test]
    fn installation_failures_preserve_the_previous_entrypoint() {
        for failure in ["download", "checksum", "elf", "version", "confirmation"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let mut ctx = boundary(root);
            let bytes = if failure == "elf" {
                vec![0u8; 32]
            } else {
                fs::read(binary(root)).unwrap()
            };
            routes(root, Tool::Claude, &bytes);
            let target = launcher(&ctx, Tool::Claude);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, "old").unwrap();
            match failure {
                "download" => fs::write(root.join("fail-download"), "").unwrap(),
                "checksum" => {
                    let mut corrupted = bytes;
                    corrupted[0] ^= 1;
                    fs::write(root.join("asset"), corrupted).unwrap();
                }
                "version" => {
                    ctx.environment
                        .insert("FIXTURE_VERSION".into(), "9.9.9".into());
                }
                "confirmation" => ctx.yes = false,
                _ => (),
            }
            assert!(install(&ctx, Tool::Claude).is_err(), "{failure}");
            assert_eq!(fs::read(&target).unwrap(), b"old");
            assert!(
                !fs::symlink_metadata(&target)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
    }

    #[test]
    fn publication_failure_restores_regular_relative_symlink_and_absent_entries() {
        for previous in ["regular", "symlink", "absent"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let mut ctx = boundary(root);
            let candidate = binary(root);
            let target = root.join("claude");
            match previous {
                "regular" => fs::write(&target, "old").unwrap(),
                "symlink" => {
                    fs::write(root.join("original"), "old").unwrap();
                    symlink("original", &target).unwrap();
                }
                _ => (),
            }
            ctx.environment
                .insert("FAIL_TARGET".into(), target.display().to_string());
            assert_eq!(
                publish(&ctx, &candidate, &target, "1.2.3").unwrap_err().key,
                "error.command"
            );
            if previous == "absent" {
                assert!(!target.exists());
            } else {
                assert_eq!(fs::read(&target).unwrap(), b"old");
            }
            if previous == "symlink" {
                assert_eq!(fs::read_link(&target).unwrap(), Path::new("original"));
            }
        }
    }

    #[test]
    fn locks_and_unsafe_launcher_parents_stop_before_downloads() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let ctx = boundary(root);
        let lock = Lock::acquire(&ctx.paths.state.join("ai/install.lock")).unwrap();
        assert_eq!(install(&ctx, Tool::Pi).unwrap_err().key, "error.lock");
        drop(lock);
        fs::create_dir(root.join(".local")).unwrap();
        symlink(root.join("commands"), root.join(".local/bin")).unwrap();
        assert_eq!(install(&ctx, Tool::Pi).unwrap_err().key, "error.path");
        assert!(!root.join("calls").exists());
    }

    #[test]
    #[ignore = "requires verified official archives; set CQS_AI_SMOKE_DIR and CQS_AI_SMOKE_TOOL"]
    fn official_binary_package_runs_without_node() {
        let cache = PathBuf::from(std::env::var("CQS_AI_SMOKE_DIR").expect("CQS_AI_SMOKE_DIR"));
        let tool = match std::env::var("CQS_AI_SMOKE_TOOL").unwrap().as_str() {
            "pi" => Tool::Pi,
            "codex" => Tool::Codex,
            _ => panic!("pi or codex"),
        };
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let ctx = boundary(root);
        let metadata = cache.join(format!("{}-release.json", tool.command()));
        let release: Value = serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        let package = github_package(
            tool,
            Architecture::parse(std::env::consts::ARCH).unwrap(),
            &release,
        )
        .unwrap();
        let routes = json!({format!("https://api.github.com/repos/{}/releases/latest", tool.repository().unwrap()):metadata,
            package.url:cache.join(format!("{}.tar.gz", tool.command()))});
        fs::write(
            root.join("routes.json"),
            serde_json::to_vec(&routes).unwrap(),
        )
        .unwrap();
        install(&ctx, tool).unwrap();
        assert!(ctx.require(&["node"]).is_err());
        assert!(ctx.require(&["npm"]).is_err());
        assert!(ctx.require(&["sh"]).is_err());
    }
}
