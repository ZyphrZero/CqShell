//! Unit tests for Docker CLI parsers, cross-linking, safety policies, and exec argument builders.

use super::{
    actions::{ConfirmPolicy, DockerOp, ExecSpec, build_exec_args},
    client::{
        cross_link_snapshot, enrich_networks_from_inspect, parse_compose_json,
        parse_container_inspect, parse_containers_ndjson, parse_daemon_json, parse_docker_top,
        parse_engine_info, parse_image_inspect, parse_images_ndjson, parse_networks_ndjson,
        parse_stats_ndjson, parse_system_df_ndjson, parse_system_df_verbose_volumes,
        parse_volumes_ndjson,
    },
    model::{
        ContainerHealth, ContainerState, DockerSnapshot, LogSeverity, is_sensitive_env_key,
        parse_log_line, parse_percent, parse_size_bytes,
    },
};

#[test]
fn size_and_percent_parsers_handle_si_binary_and_composite_docker_formats() {
    assert_eq!(parse_size_bytes("0B"), Some(0));
    assert_eq!(parse_size_bytes("42kB"), Some(42_000));
    assert_eq!(parse_size_bytes("318MiB"), Some(318 * 1_048_576));
    assert_eq!(parse_size_bytes("1.5GB"), Some(1_500_000_000));
    assert_eq!(parse_size_bytes("128MB (virtual 412MB)"), Some(128_000_000));
    assert_eq!(parse_size_bytes("--"), None);

    assert_eq!(parse_percent("8.42%"), Some(8.42));
    assert_eq!(parse_percent("0.00%"), Some(0.0));
    assert_eq!(parse_percent("--"), None);
}

#[test]
fn sensitive_env_detection_and_log_sanitization_protect_secrets_and_terminal() {
    assert!(is_sensitive_env_key("POSTGRES_PASSWORD"));
    assert!(is_sensitive_env_key("API_SECRET_KEY"));
    assert!(is_sensitive_env_key("JWT_SIGNING_TOKEN"));
    assert!(is_sensitive_env_key("DATABASE_DSN"));
    assert!(!is_sensitive_env_key("NODE_ENV"));
    assert!(!is_sensitive_env_key("TZ"));

    let line = parse_log_line(
        "2026-11-23T09:18:22.123456789Z \x1b]52;c;Zm9v\x07\x1b[31mERROR\x1b[0m upstream timeout",
    );
    assert_eq!(line.timestamp.as_deref(), Some("11-23 09:18:22"));
    assert_eq!(line.severity, LogSeverity::Error);
    assert!(!line.message.contains('\x1b'));
    assert!(line.message.contains("ERROR"));
}

#[test]
fn containers_compose_images_volumes_networks_stats_and_df_parse_and_cross_link() {
    let ps = r#"{"ID":"a1b2c3d4e5f6001122334455","Names":"nginx-proxy","Image":"nginx:1.27-alpine","Command":"\"nginx -g 'daemon off;'\"","CreatedAt":"2026-11-20 10:00:00 +0800 CST","RunningFor":"3 days ago","Status":"Up 3 days (healthy)","State":"running","Ports":"0.0.0.0:80->80/tcp, 0.0.0.0:443->443/tcp","Networks":"prod_net","Mounts":"/etc/nginx,nginx_logs","Size":"1.2MB (virtual 45MB)","Labels":"com.docker.compose.project=prod-web,com.docker.compose.service=proxy,com.docker.compose.project.config_files=/srv/prod/compose.yaml"}
{"ID":"ffeeddccbbaa998877665544","Names":"orphan-job","Image":"alpine:3.20","Command":"\"sh\"","CreatedAt":"2026-11-21 10:00:00 +0800 CST","RunningFor":"2 days ago","Status":"Exited (0) 2 hours ago","State":"exited","Ports":"","Networks":"bridge","Mounts":"","Size":"0B","Labels":"com.docker.compose.project=jobs,com.docker.compose.service=cron"}"#;

    let containers = parse_containers_ndjson(ps);
    assert_eq!(containers.len(), 2);
    assert_eq!(containers[0].name, "nginx-proxy");
    assert_eq!(containers[0].state, ContainerState::Running);
    assert_eq!(containers[0].health, ContainerHealth::Healthy);
    assert_eq!(containers[0].compact_ports(), "80,443");
    assert_eq!(containers[0].compose_project.as_deref(), Some("prod-web"));

    // Compose JSON array format
    let compose_arr =
        r#"[{"Name":"prod-web","Status":"running(1)","ConfigFiles":"/srv/prod/compose.yaml"}]"#;
    let compose = parse_compose_json(compose_arr);
    assert_eq!(compose.len(), 1);
    assert_eq!(compose[0].name, "prod-web");

    // Compose NDJSON fallback format
    let compose_ndjson =
        r#"{"Name":"prod-web","Status":"running(1)","ConfigFiles":"/srv/prod/compose.yaml"}"#;
    assert_eq!(parse_compose_json(compose_ndjson).len(), 1);

    let images = parse_images_ndjson(
        r#"{"ID":"sha256:1234567890abcdef1234567890","Repository":"nginx","Tag":"1.27-alpine","CreatedSince":"2 weeks ago","CreatedAt":"2026-11-01","Size":"44.8MB"}
{"ID":"sha256:abcdef0123456789abcdef0123","Repository":"<none>","Tag":"<none>","CreatedSince":"3 weeks ago","CreatedAt":"2026-10-20","Size":"12MB"}"#,
    );
    assert_eq!(images.len(), 2);
    assert!(!images[0].dangling);
    assert!(images[1].dangling);

    let mut snap = DockerSnapshot {
        containers,
        compose,
        images,
        ..DockerSnapshot::default()
    };
    cross_link_snapshot(&mut snap);
    // Image `nginx:1.27-alpine` cross-linked to `nginx-proxy`
    assert_eq!(snap.images[0].used_by, vec!["nginx-proxy".to_owned()]);
    // Labeled project `jobs` synthesized even though not returned by `docker compose ls`
    assert_eq!(snap.compose.len(), 2);
    assert!(
        snap.compose
            .iter()
            .any(|p| p.name == "jobs" && p.synthesized_from_labels)
    );

    let stats = parse_stats_ndjson(
        r#"{"ID":"a1b2c3d4e5f6001122334455","CPUPerc":"8.42%","MemUsage":"318MiB / 1.94GiB","MemPerc":"16.01%","NetIO":"142MB / 89.4MB","BlockIO":"24.1MB / 11.8MB","PIDs":"18"}"#,
    );
    let st = stats.get("a1b2c3d4e5f6001122334455").expect("parsed stats");
    assert!((st.cpu_percent - 8.42).abs() < 0.01);
    assert_eq!(st.pids, 18);

    let df = parse_system_df_ndjson(
        r#"{"Type":"Images","TotalCount":"14","Active":"5","Size":"4.82GB","Reclaimable":"2.1GB (43%)"}
{"Type":"Local Volumes","TotalCount":"8","Active":"4","Size":"1.9GB","Reclaimable":"620MB (32%)"}"#,
    );
    assert_eq!(df.len(), 2);
    assert_eq!(df[0].reclaimable_bytes, 2_100_000_000);

    let df_v = parse_system_df_verbose_volumes(
        r#"{"Volumes":[{"Name":"pg_data","Links":"1","Size":"612MB"}]}"#,
    );
    assert_eq!(df_v.get("pg_data").unwrap().1, 612_000_000);

    let vols = parse_volumes_ndjson(
        r#"{"Name":"pg_data","Driver":"local","Scope":"local","Mountpoint":"/var/lib/docker/volumes/pg_data/_data","Labels":""}
{"Name":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","Driver":"local","Scope":"local","Mountpoint":"/tmp","Labels":""}"#,
    );
    assert!(!vols[0].anonymous);
    assert!(vols[1].anonymous);

    let mut nets = parse_networks_ndjson(
        r#"{"ID":"1111222233334444","Name":"bridge","Driver":"bridge","Scope":"local","IPv6":"false","Internal":"false","Labels":""}
{"ID":"5555666677778888","Name":"prod_net","Driver":"bridge","Scope":"local","IPv6":"true","Internal":"false","Labels":""}"#,
    );
    assert!(nets[0].builtin);
    assert!(!nets[1].builtin);
    enrich_networks_from_inspect(
        &mut nets,
        r#"[{"Id":"5555666677778888","Name":"prod_net","EnableIPv6":true,"IPAM":{"Config":[{"Subnet":"172.18.0.0/16","Gateway":"172.18.0.1"}]},"Containers":{"cid":{"Name":"nginx-proxy","IPv4Address":"172.18.0.2/16"}}}]"#,
    );
    assert_eq!(nets[1].subnet, "172.18.0.0/16");
    assert_eq!(nets[1].containers.len(), 1);
}

#[test]
fn inspect_top_engine_and_daemon_json_parsers_work() {
    let engine = parse_engine_info(
        r#"{"ServerVersion":"27.3.1","OSType":"linux","Architecture":"x86_64","KernelVersion":"6.12.8","Driver":"overlay2","CgroupDriver":"systemd","CgroupVersion":"2","DockerRootDir":"/var/lib/docker","ContainersRunning":5,"ContainersPaused":0,"ContainersStopped":2,"Images":14}"#,
    );
    assert_eq!(engine.server_version, "27.3.1");
    assert_eq!(engine.containers_running, 5);

    let (mirrors, ipv6, cidr, err) = parse_daemon_json(
        r#"{"registry-mirrors":["https://mirror.example.com"],"ipv6":true,"fixed-cidr-v6":"fd00:1::/64"}"#,
    );
    assert!(err.is_none());
    assert_eq!(mirrors, vec!["https://mirror.example.com"]);
    assert_eq!(ipv6, Some(true));
    assert_eq!(cidr.as_deref(), Some("fd00:1::/64"));

    let insp = parse_container_inspect(
        r#"[{"Id":"abc123","Name":"/api-gateway","Platform":"linux","RestartCount":1,"Created":"2026-11-20T00:00:00Z","State":{"StartedAt":"2026-11-20T00:00:01Z","FinishedAt":"","ExitCode":0,"Health":{"Status":"healthy","Log":[{"ExitCode":0,"Output":"ok\n"}]}},"HostConfig":{"RestartPolicy":{"Name":"unless-stopped"}},"Config":{"Image":"ghcr.io/org/api:v4","Env":["NODE_ENV=production","DB_PASSWORD=supersecret"],"WorkingDir":"/app","User":"node","Cmd":["./gateway"]},"Mounts":[{"Type":"bind","Source":"/etc/app","Destination":"/app/etc","RW":false,"Mode":"ro"}],"NetworkSettings":{"Ports":{"8080/tcp":[{"HostIp":"0.0.0.0","HostPort":"8080"}]},"Networks":{"prod_net":{"IPAddress":"172.18.0.3","Gateway":"172.18.0.1","MacAddress":"02:42:ac:12:00:03"}}}}]"#,
    )
    .unwrap();
    assert_eq!(insp.name, "api-gateway");
    assert_eq!(insp.env.len(), 2);
    assert!(!insp.env[0].sensitive);
    assert!(insp.env[1].sensitive);
    assert_eq!(insp.mounts.len(), 1);
    assert!(!insp.mounts[0].rw);
    assert_eq!(insp.health_status.as_deref(), Some("healthy"));

    let img_insp = parse_image_inspect(
        r#"[{"Id":"sha256:123","RepoTags":["nginx:1.27"],"Os":"linux","Architecture":"amd64","Size":44800000,"Config":{"ExposedPorts":{"80/tcp":{}},"Env":["NGINX_VERSION=1.27"]}}]"#,
        r#"{"ID":"sha256:aaa","CreatedSince":"2 weeks ago","CreatedBy":"CMD [\"nginx\"]","Size":"0B"}"#,
    )
    .unwrap();
    assert_eq!(img_insp.exposed_ports, vec!["80/tcp"]);
    assert_eq!(img_insp.history.len(), 1);

    let top = parse_docker_top(
        "USER PID %CPU %MEM ELAPSED COMMAND\nroot 1 7.8 2.1 03:12:00 /app/gateway --prod\n",
    );
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].pid, "1");
    assert_eq!(top[0].command, "/app/gateway --prod");
}

#[test]
fn confirmation_policies_and_argument_validation_enforce_safety_tiers() {
    let start = DockerOp::ContainerStart {
        ids: vec!["abc".into()],
        names: vec!["web".into()],
    };
    assert_eq!(start.confirm_policy(), ConfirmPolicy::None);

    let stop = DockerOp::ContainerStop {
        ids: vec!["abc".into()],
        names: vec!["web".into()],
    };
    assert_eq!(stop.confirm_policy(), ConfirmPolicy::YesNo);

    let vol_rm = DockerOp::VolumeRemove {
        names: vec!["pg_data".into()],
    };
    assert_eq!(vol_rm.confirm_policy(), ConfirmPolicy::Token("REMOVE"));

    let batch_rm = DockerOp::ContainerRemove {
        ids: vec!["a".into(), "b".into(), "c".into()],
        names: vec!["a".into(), "b".into(), "c".into()],
        force: true,
    };
    assert_eq!(batch_rm.confirm_policy(), ConfirmPolicy::Token("REMOVE"));

    let prune_vols = DockerOp::Prune {
        containers: true,
        images: false,
        all_unused_images: false,
        volumes: true,
        networks: false,
        build_cache: false,
    };
    assert_eq!(prune_vols.confirm_policy(), ConfirmPolicy::Token("PRUNE"));

    // Reject option injection
    let bad_pull = DockerOp::ImagePull {
        reference: "--platform=linux/amd64".into(),
    };
    assert!(bad_pull.validate().is_err());

    let exec_ok = ExecSpec {
        container_id: "abc123".into(),
        container_name: "web".into(),
        shell: "auto".into(),
        user: "root".into(),
        workdir: "/app".into(),
    };
    let args = build_exec_args(&exec_ok, "/bin/bash").unwrap();
    assert_eq!(
        args,
        vec![
            "exec",
            "-it",
            "-u",
            "root",
            "-w",
            "/app",
            "abc123",
            "/bin/bash"
        ]
    );

    let exec_bad = ExecSpec {
        container_id: "-f".into(),
        container_name: "bad".into(),
        shell: "/bin/sh".into(),
        user: String::new(),
        workdir: String::new(),
    };
    assert!(build_exec_args(&exec_bad, "/bin/sh").is_err());
}
