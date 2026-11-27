//! Structured JSON control plane shared by the CLI (`--json`, `status`, `describe`, `choices`)
//! and the native MCP stdio server.

pub mod snapshots;

use crate::{
    actions::{self, Field, Kind, Listing, Parameters, Source, Spec},
    error::{Error, Result},
    ops::Choice,
    ops::docker::actions::{ConfirmPolicy, DockerOp},
    runtime::{Context, Event, Outcome},
};
use serde_json::{Map, Value, json};
use std::{collections::BTreeSet, sync::mpsc::Receiver, time::Instant};

/// Stable version identifier included in every structured JSON envelope.
pub const SCHEMA_VERSION: &str = "1";

/// Upper bound on captured log lines returned in one structured JSON response.
pub const MAX_CAPTURED_LOGS: usize = 2000;

/// Access-control policy governing programmatic action and Docker mutations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPolicy {
    pub read_only: bool,
    pub allow_danger: bool,
    pub allow_groups: Vec<String>,
    pub deny_actions: Vec<String>,
}

impl ExecutionPolicy {
    /// Permissive policy used by direct CLI invocations (`cqs run --json` / `cqs batch --json`).
    pub fn permissive() -> Self {
        Self {
            read_only: false,
            allow_danger: true,
            allow_groups: Vec::new(),
            deny_actions: Vec::new(),
        }
    }

    /// Safe default policy for `cqs mcp`: read-only queries and reversible/standard mutations
    /// (with explicit `confirm: true`) are allowed; destructive/high-risk mutations require
    /// starting the server with `--allow-danger`.
    pub fn mcp_default() -> Self {
        Self {
            read_only: false,
            allow_danger: false,
            allow_groups: Vec::new(),
            deny_actions: Vec::new(),
        }
    }

    /// Validates configured group allow-lists and action deny-lists up front so typos never
    /// silently weaken security boundaries.
    pub fn validate(&self) -> Result<()> {
        for group in &self.allow_groups {
            if !actions::GROUPS.contains(&group.as_str()) {
                return Err(Error::new(
                    "error.input",
                    format!("unknown group in --allow-group: {group}"),
                ));
            }
        }
        for id in &self.deny_actions {
            if actions::find_ref(id).is_err() {
                return Err(Error::new(
                    "error.action",
                    format!("unknown action in --deny-action: {id}"),
                ));
            }
        }
        Ok(())
    }

    /// Checks whether `spec` with resolved `parameters` is permitted under this policy.
    pub fn check_action(&self, spec: &Spec, parameters: &Parameters) -> Result<()> {
        if !self.allow_groups.is_empty() && !self.allow_groups.iter().any(|g| g == spec.group) {
            return Err(Error::new(
                "error.policy",
                format!(
                    "action '{}' belongs to group '{}' which is outside --allow-group ({})",
                    spec.id,
                    spec.group,
                    self.allow_groups.join(", ")
                ),
            ));
        }
        if self.deny_actions.iter().any(|id| id == spec.id) {
            return Err(Error::new(
                "error.policy",
                format!("action '{}' is blocked by --deny-action", spec.id),
            ));
        }
        if spec.changes && self.read_only {
            return Err(Error::new(
                "error.policy",
                format!(
                    "action '{}' mutates system or user state, blocked by --read-only",
                    spec.id
                ),
            ));
        }
        if spec.changes && spec.is_danger(Some(parameters)) && !self.allow_danger {
            return Err(Error::new(
                "error.policy",
                format!(
                    "action '{}' is classified as high-risk/danger; restart server with --allow-danger to authorize it",
                    spec.id
                ),
            ));
        }
        Ok(())
    }

    /// Checks whether a typed `DockerOp` mutation is permitted under this policy.
    pub fn check_docker_op(&self, op: &DockerOp) -> Result<()> {
        if !self.allow_groups.is_empty() && !self.allow_groups.iter().any(|g| g == "docker") {
            return Err(Error::new(
                "error.policy",
                format!(
                    "docker mutations are outside --allow-group ({})",
                    self.allow_groups.join(", ")
                ),
            ));
        }
        if self.read_only {
            return Err(Error::new(
                "error.policy",
                "docker mutations are blocked by --read-only",
            ));
        }
        if op.confirm_policy() == ConfirmPolicy::Danger && !self.allow_danger {
            return Err(Error::new(
                "error.policy",
                format!(
                    "docker operation '{}' is classified as danger; restart server with --allow-danger to authorize it",
                    op.target_summary()
                ),
            ));
        }
        Ok(())
    }

    pub fn to_json(&self, ctx: &Context) -> Value {
        let platform_supported = ctx.platform().is_ok();
        json!({
            "schema_version": SCHEMA_VERSION,
            "ok": true,
            "cqs_version": env!("CARGO_PKG_VERSION"),
            "locale": ctx.i18n.locale(),
            "platform_supported": platform_supported,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "is_root": ctx.is_root(),
            "policy": {
                "read_only": self.read_only,
                "allow_danger": self.allow_danger,
                "allow_groups": self.allow_groups,
                "deny_actions": self.deny_actions,
            },
            "guarantees": {
                "secrets_redacted": true,
                "auto_reboot_disabled": true,
                "interactive_tty_blocked": true,
                "explicit_confirm_required_for_mutations": true,
            }
        })
    }
}

/// Filter options for querying the structured action catalog.
#[derive(Debug, Default, Clone)]
pub struct ActionFilter {
    pub group: Option<String>,
    pub query: Option<String>,
    pub mutates: Option<bool>,
    pub root: Option<bool>,
    pub non_interactive_only: bool,
}

/// Maps a CqShell error to its canonical CLI exit code.
pub fn exit_code(error: &Error) -> i32 {
    if error.key == "error.cancelled" {
        130
    } else if matches!(
        error.key,
        "error.input" | "error.action" | "error.required" | "error.cli"
    ) {
        2
    } else {
        1
    }
}

/// Serializes a CqShell error into a stable structured object.
pub fn error_object(ctx: &Context, error: &Error) -> Value {
    let code = error.key.strip_prefix("error.").unwrap_or(error.key);
    json!({
        "key": error.key,
        "code": code,
        "message": ctx.i18n.error(error),
        "detail": error.detail,
        "exit_code": exit_code(error),
    })
}

/// Top-level structured error envelope for CLI `--json` error reporting.
pub fn error_envelope(ctx: &Context, error: &Error) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": false,
        "status": if error.key == "error.cancelled" { "cancelled" } else { "failed" },
        "error": error_object(ctx, error),
    })
}

fn source_json(source: Option<Source>) -> Value {
    match source {
        None => Value::Null,
        Some(Source::Listing(listing)) => {
            let name = match listing {
                Listing::Subscriptions => "subscriptions",
                Listing::ProxyGroups => "proxy_groups",
                Listing::DelayGroups => "delay_groups",
                Listing::BootKernels => "boot_kernels",
                Listing::RemovableKernels => "removable_kernels",
            };
            json!({ "kind": "listing", "name": name })
        }
        Some(Source::Children(parent)) => json!({ "kind": "children", "parent": parent }),
    }
}

fn field_json(ctx: &Context, field: &Field) -> Value {
    json!({
        "name": field.name,
        "label": ctx.i18n.text(&field.label()),
        "default": field.default,
        "choices": field.choices,
        "allow_custom": field.allow_custom,
        "editable": field.editable(),
        "required": field.required,
        "dynamic_source": source_json(field.source),
    })
}

/// Generates a standard JSON Schema (`type: "object"`) describing an action's parameter map.
pub fn action_input_schema(ctx: &Context, spec: &Spec) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in &spec.fields {
        let mut prop = Map::new();
        prop.insert("type".into(), json!("string"));
        prop.insert("description".into(), json!(ctx.i18n.text(&field.label())));
        if !field.default.is_empty() {
            prop.insert("default".into(), json!(field.default));
        }
        if !field.choices.is_empty() && !field.allow_custom {
            prop.insert("enum".into(), json!(field.choices));
        } else if !field.choices.is_empty() {
            prop.insert("examples".into(), json!(field.choices));
        }
        if field.required && field.default.is_empty() {
            required.push(json!(field.name));
        }
        properties.insert(field.name.to_owned(), Value::Object(prop));
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Concise structured summary of an action for catalog listings and execution responses.
pub fn action_summary_json(ctx: &Context, spec: &Spec, parameters: Option<&Parameters>) -> Value {
    json!({
        "id": spec.id,
        "group": spec.group,
        "group_label": ctx.i18n.text(&format!("group.{}", spec.group)),
        "label": ctx.i18n.text(&spec.label()),
        "description": ctx.i18n.text(&spec.description()),
        "root": spec.root,
        "mutates": spec.changes,
        "reversible": spec.reversible,
        "interactive": spec.external,
        "non_interactive_supported": spec.non_interactive_supported(),
        "risk": spec.risk_level(parameters),
        "in_optimization": actions::OPTIMIZATION.contains(&spec.id),
        "parameter_names": spec.fields.iter().map(|f| f.name).collect::<Vec<_>>(),
    })
}

/// Full structured specification of a single action, including field schemas and dynamic sources.
pub fn action_detail_json(ctx: &Context, spec: &Spec) -> Value {
    let has_dynamic_choices = spec.fields.iter().any(|f| f.source.is_some());
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "action": {
            "id": spec.id,
            "group": spec.group,
            "group_label": ctx.i18n.text(&format!("group.{}", spec.group)),
            "label": ctx.i18n.text(&spec.label()),
            "description": ctx.i18n.text(&spec.description()),
            "root": spec.root,
            "mutates": spec.changes,
            "reversible": spec.reversible,
            "interactive": spec.external,
            "non_interactive_supported": spec.non_interactive_supported(),
            "requires_platform": spec.requires_platform(),
            "risk": spec.risk_level(None),
            "in_optimization": actions::OPTIMIZATION.contains(&spec.id),
            "has_dynamic_choices": has_dynamic_choices,
            "defaults": spec.defaults(),
            "parameters": spec.fields.iter().map(|f| field_json(ctx, f)).collect::<Vec<_>>(),
            "input_schema": action_input_schema(ctx, spec),
        }
    })
}

/// Lists actions matching `filter` and annotates policy availability.
pub fn list_actions_json(
    ctx: &Context,
    filter: &ActionFilter,
    policy: Option<&ExecutionPolicy>,
) -> Value {
    let query = filter
        .query
        .as_deref()
        .map(|q| q.trim().to_ascii_lowercase())
        .filter(|q| !q.is_empty());

    let mut rows = Vec::new();
    for spec in actions::specs_slice() {
        if let Some(group) = filter.group.as_deref()
            && !group.is_empty()
            && spec.group != group
        {
            continue;
        }
        if let Some(mutates) = filter.mutates
            && spec.changes != mutates
        {
            continue;
        }
        if let Some(root) = filter.root
            && spec.root != root
        {
            continue;
        }
        if filter.non_interactive_only && !spec.non_interactive_supported() {
            continue;
        }
        if let Some(q) = query.as_deref() {
            let label = ctx.i18n.text(&spec.label()).to_ascii_lowercase();
            let desc = ctx.i18n.text(&spec.description()).to_ascii_lowercase();
            if !spec.id.to_ascii_lowercase().contains(q)
                && !spec.group.to_ascii_lowercase().contains(q)
                && !label.contains(q)
                && !desc.contains(q)
            {
                continue;
            }
        }

        let mut item = action_summary_json(ctx, spec, None);
        if let Some(pol) = policy
            && let Some(obj) = item.as_object_mut()
        {
            let allowed = pol.check_action(spec, &spec.defaults()).is_ok();
            obj.insert("allowed_by_policy".into(), json!(allowed));
        }
        rows.push(item);
    }

    let groups = actions::GROUPS
        .iter()
        .map(|g| {
            json!({
                "id": g,
                "label": ctx.i18n.text(&format!("group.{g}")),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "total": rows.len(),
        "groups": groups,
        "optimization_order": actions::OPTIMIZATION,
        "actions": rows,
    })
}

fn choice_to_json(choice: &Choice) -> Value {
    json!({
        "value": choice.value,
        "detail": choice.detail,
        "current": choice.current,
        "children": choice.children.iter().map(choice_to_json).collect::<Vec<_>>(),
    })
}

/// Resolves live host choices for every field on `action_id` that declares a `Source::Listing`.
pub fn action_choices_json(ctx: &Context, action_id: &str) -> Result<Value> {
    let spec = actions::find_ref(action_id)?;
    let probe = ctx.silent();
    let mut fields = Vec::new();
    for field in &spec.fields {
        match field.source {
            Some(Source::Listing(listing)) => match actions::choices(&probe, listing) {
                Ok(items) => {
                    fields.push(json!({
                        "field": field.name,
                        "source": source_json(field.source),
                        "ok": true,
                        "choices": items.iter().map(choice_to_json).collect::<Vec<_>>(),
                        "error": Value::Null,
                    }));
                }
                Err(err) => {
                    fields.push(json!({
                        "field": field.name,
                        "source": source_json(field.source),
                        "ok": false,
                        "choices": [],
                        "error": error_object(ctx, &err),
                    }));
                }
            },
            Some(Source::Children(parent)) => {
                fields.push(json!({
                    "field": field.name,
                    "source": source_json(field.source),
                    "ok": true,
                    "note": format!("values are nested under each item's `children` in field `{parent}`"),
                }));
            }
            None if !field.choices.is_empty() => {
                fields.push(json!({
                    "field": field.name,
                    "source": { "kind": "static" },
                    "ok": true,
                    "allow_custom": field.allow_custom,
                    "choices": field.choices.iter().map(|v| json!({
                        "value": v,
                        "detail": "",
                        "current": *v == field.default,
                        "children": [],
                    })).collect::<Vec<_>>(),
                    "error": Value::Null,
                }));
            }
            None => {}
        }
    }
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "action_id": spec.id,
        "fields": fields,
    }))
}

/// Converts a JSON object `{ "key": scalar }` into [`Parameters`], coercing JSON numbers and
/// booleans ergonomically for AI callers (booleans map to `"on"`/`"off"` when the field's enum
/// uses `"on"`/`"off"`).
pub fn parameters_from_json(spec: &Spec, raw: &Value) -> Result<Parameters> {
    let obj = match raw {
        Value::Null => return Ok(Parameters::new()),
        Value::Object(map) => map,
        _ => {
            return Err(Error::new(
                "error.input",
                format!("{}: parameters must be a JSON object", spec.id),
            ));
        }
    };
    let mut out = Parameters::new();
    for (key, value) in obj {
        if value.is_null() {
            continue;
        }
        let field = spec.fields.iter().find(|f| f.name == key);
        let text = match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => {
                let uses_on_off =
                    field.is_some_and(|f| f.choices.contains(&"on") && f.choices.contains(&"off"));
                if uses_on_off {
                    if *b { "on".into() } else { "off".into() }
                } else if *b {
                    "true".into()
                } else {
                    "false".into()
                }
            }
            _ => {
                return Err(Error::new(
                    "error.input",
                    format!(
                        "{}: parameter '{key}' must be a string, number, or boolean",
                        spec.id
                    ),
                ));
            }
        };
        out.insert(key.clone(), text);
    }
    Ok(out)
}

/// Parses a JSON object mapping `{ "<action_id>": { "<param>": scalar } }` into validated batch
/// jobs matching the order of `ids`.
pub fn batch_jobs_from_json(ids: &[&str], raw: &Value) -> Result<Vec<(Spec, Parameters)>> {
    let map = match raw {
        Value::Null => None,
        Value::Object(m) => Some(m),
        _ => {
            return Err(Error::new(
                "error.input",
                "batch params-json must be a JSON object keyed by action ID",
            ));
        }
    };
    let id_set: BTreeSet<&str> = ids.iter().copied().collect();
    if let Some(m) = map {
        for key in m.keys() {
            if !id_set.contains(key.as_str()) {
                return Err(Error::new(
                    "error.input",
                    format!("params-json contains unlisted action ID: {key}"),
                ));
            }
        }
    }
    let mut jobs = Vec::with_capacity(ids.len());
    for id in ids {
        let spec = actions::find(id)?;
        let supplied = match map.and_then(|m| m.get(*id)) {
            Some(val) => parameters_from_json(&spec, val)?,
            None => Parameters::new(),
        };
        let resolved = spec.parameters(&supplied)?;
        jobs.push((spec, resolved));
    }
    Ok(jobs)
}

pub(crate) fn check_non_interactive_action(spec: &Spec, resolved: &Parameters) -> Result<()> {
    if !spec.non_interactive_supported() {
        return Err(Error::new(
            "error.tty",
            format!(
                "action '{}' launches an interactive terminal program and cannot run in headless JSON/MCP mode",
                spec.id
            ),
        ));
    }
    if spec.kind == Kind::ClashLogs && resolved.get("follow").map(String::as_str) == Some("on") {
        return Err(Error::new(
            "error.tty",
            "clash.logs with follow=on streams indefinitely and is not supported in JSON/MCP mode; use follow=off",
        ));
    }
    Ok(())
}

pub(crate) fn preflight_json(ctx: &Context, spec: &Spec, resolved: &Parameters) -> Value {
    let mut blockers = Vec::new();
    let platform_ok = !spec.requires_platform() || ctx.platform().is_ok();
    if spec.requires_platform()
        && let Err(err) = ctx.platform()
    {
        blockers.push(error_object(ctx, &err));
    }
    let is_root = ctx.is_root();
    if spec.root
        && platform_ok
        && let Err(err) = ctx.root()
    {
        blockers.push(error_object(ctx, &err));
    }
    if let Err(err) = check_non_interactive_action(spec, resolved) {
        blockers.push(error_object(ctx, &err));
    }
    json!({
        "platform_supported": platform_ok,
        "requires_root": spec.root,
        "is_root": is_root,
        "ready": blockers.is_empty(),
        "blockers": blockers,
    })
}

pub(crate) struct CapturedLogs {
    pub lines: Vec<String>,
    pub total: usize,
    pub truncated: bool,
}

pub(crate) fn drain_logs(rx: Receiver<Event>) -> CapturedLogs {
    let mut lines = Vec::new();
    let mut total = 0usize;
    while let Ok(event) = rx.try_recv() {
        if let Event::Log(line) = event {
            total += 1;
            if lines.len() < MAX_CAPTURED_LOGS {
                lines.push(line);
            }
        }
    }
    let truncated = total > lines.len();
    CapturedLogs {
        lines,
        total,
        truncated,
    }
}

/// Previews or executes a single CqShell action and returns `(json_payload, cli_exit_code)`.
pub fn execute_action_json(
    ctx: &Context,
    policy: &ExecutionPolicy,
    action_id: &str,
    supplied: &Parameters,
    confirm: bool,
    dry_run: bool,
) -> (Value, i32) {
    let spec = match actions::find(action_id) {
        Ok(spec) => spec,
        Err(err) => {
            let code = exit_code(&err);
            return (error_envelope(ctx, &err), code);
        }
    };
    let resolved = match spec.parameters(supplied) {
        Ok(p) => p,
        Err(err) => {
            let code = exit_code(&err);
            return (
                json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": false,
                    "status": "failed",
                    "action": action_summary_json(ctx, &spec, None),
                    "error": error_object(ctx, &err),
                }),
                code,
            );
        }
    };

    if let Err(err) = check_non_interactive_action(&spec, &resolved) {
        let code = exit_code(&err);
        return (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "status": "failed",
                "action": action_summary_json(ctx, &spec, Some(&resolved)),
                "parameters": resolved,
                "error": error_object(ctx, &err),
            }),
            code,
        );
    }

    if let Err(err) = policy.check_action(&spec, &resolved) {
        let code = exit_code(&err);
        return (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "status": "policy_denied",
                "action": action_summary_json(ctx, &spec, Some(&resolved)),
                "parameters": resolved,
                "preflight": preflight_json(ctx, &spec, &resolved),
                "error": error_object(ctx, &err),
            }),
            code,
        );
    }

    let preflight = preflight_json(ctx, &spec, &resolved);
    if dry_run {
        return (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "status": "dry_run",
                "action": action_summary_json(ctx, &spec, Some(&resolved)),
                "parameters": resolved,
                "requires_confirmation": spec.changes,
                "preflight": preflight,
            }),
            0,
        );
    }

    if spec.changes && !confirm {
        let err = Error::new("error.confirm", "--yes / confirm=true");
        let code = exit_code(&err);
        return (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "status": "confirmation_required",
                "action": action_summary_json(ctx, &spec, Some(&resolved)),
                "parameters": resolved,
                "requires_confirmation": true,
                "preflight": preflight,
                "confirmation": {
                    "risk": spec.risk_level(Some(&resolved)),
                    "reversible": spec.reversible,
                    "hint": "Re-run with --yes (CLI) or confirm=true (MCP) after reviewing parameters and impact.",
                },
                "error": error_object(ctx, &err),
            }),
            code,
        );
    }

    let (cap_ctx, rx) = ctx.captured(confirm);
    let started = Instant::now();
    let result = actions::execute(&cap_ctx, &spec, &resolved);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    drop(cap_ctx);
    let captured = drain_logs(rx);

    match result {
        Ok(()) => (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "status": "completed",
                "action": action_summary_json(ctx, &spec, Some(&resolved)),
                "parameters": resolved,
                "elapsed_ms": elapsed_ms,
                "logs": captured.lines,
                "logs_total": captured.total,
                "logs_truncated": captured.truncated,
            }),
            0,
        ),
        Err(err) => {
            let code = exit_code(&err);
            let status = if err.key == "error.cancelled" {
                "cancelled"
            } else {
                "failed"
            };
            (
                json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": false,
                    "status": status,
                    "action": action_summary_json(ctx, &spec, Some(&resolved)),
                    "parameters": resolved,
                    "elapsed_ms": elapsed_ms,
                    "logs": captured.lines,
                    "logs_total": captured.total,
                    "logs_truncated": captured.truncated,
                    "error": error_object(ctx, &err),
                }),
                code,
            )
        }
    }
}

/// Previews or executes an ordered batch of CqShell actions and returns `(json_payload, exit_code)`.
pub fn execute_batch_json(
    ctx: &Context,
    policy: &ExecutionPolicy,
    jobs: &[(Spec, Parameters)],
    confirm: bool,
    dry_run: bool,
) -> (Value, i32) {
    if jobs.is_empty() {
        let err = Error::new("error.input", "empty selection");
        let code = exit_code(&err);
        return (error_envelope(ctx, &err), code);
    }

    let mut unique = BTreeSet::new();
    let mut resolved_jobs = Vec::with_capacity(jobs.len());
    let mut plan_items = Vec::with_capacity(jobs.len());
    let mut any_mutates = false;

    for (spec, supplied) in jobs {
        if !unique.insert(spec.id) {
            let err = Error::new("error.input", format!("duplicate action ID: {}", spec.id));
            let code = exit_code(&err);
            return (error_envelope(ctx, &err), code);
        }
        let resolved = match spec.parameters(supplied) {
            Ok(p) => p,
            Err(err) => {
                let code = exit_code(&err);
                return (error_envelope(ctx, &err), code);
            }
        };
        if let Err(err) = check_non_interactive_action(spec, &resolved) {
            let code = exit_code(&err);
            return (error_envelope(ctx, &err), code);
        }
        if let Err(err) = policy.check_action(spec, &resolved) {
            let code = exit_code(&err);
            return (
                json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": false,
                    "status": "policy_denied",
                    "action": action_summary_json(ctx, spec, Some(&resolved)),
                    "parameters": resolved,
                    "error": error_object(ctx, &err),
                }),
                code,
            );
        }
        if spec.changes {
            any_mutates = true;
        }
        plan_items.push(json!({
            "action": action_summary_json(ctx, spec, Some(&resolved)),
            "parameters": resolved,
            "preflight": preflight_json(ctx, spec, &resolved),
        }));
        resolved_jobs.push((spec.clone(), resolved));
    }

    if dry_run {
        return (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "status": "dry_run",
                "total": resolved_jobs.len(),
                "requires_confirmation": any_mutates,
                "jobs": plan_items,
            }),
            0,
        );
    }

    if any_mutates && !confirm {
        let err = Error::new("error.confirm", "--yes / confirm=true");
        let code = exit_code(&err);
        return (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "status": "confirmation_required",
                "total": resolved_jobs.len(),
                "requires_confirmation": true,
                "jobs": plan_items,
                "confirmation": {
                    "hint": "Re-run with --yes (CLI) or confirm=true (MCP) after reviewing every planned job.",
                },
                "error": error_object(ctx, &err),
            }),
            code,
        );
    }

    // Batch always asks `confirm.batch` before starting; for read-only batches in headless mode,
    // pass `yes = true` so `confirm.batch` does not block non-mutating queries.
    let (cap_ctx, rx) = ctx.captured(confirm || !any_mutates);
    let started = Instant::now();
    let report = actions::batch_report(&cap_ctx, &resolved_jobs);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    drop(cap_ctx);
    let captured = drain_logs(rx);

    let steps = report
        .steps
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let outcome = match record.outcome {
                Some(Outcome::Done) => "done",
                Some(Outcome::Failed) => "failed",
                Some(Outcome::Cancelled) => "cancelled",
                None => "skipped",
            };
            json!({
                "index": index,
                "id": record.id,
                "label": ctx.i18n.text(&format!("action.{}", record.id)),
                "outcome": outcome,
                "elapsed_ms": record.elapsed.map(|d| d.as_millis() as u64),
            })
        })
        .collect::<Vec<_>>();

    match report.result {
        Ok(()) => (
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "status": "completed",
                "total": resolved_jobs.len(),
                "elapsed_ms": elapsed_ms,
                "steps": steps,
                "logs": captured.lines,
                "logs_total": captured.total,
                "logs_truncated": captured.truncated,
            }),
            0,
        ),
        Err(err) => {
            let code = exit_code(&err);
            let status = if err.key == "error.cancelled" {
                "cancelled"
            } else {
                "failed"
            };
            (
                json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": false,
                    "status": status,
                    "total": resolved_jobs.len(),
                    "elapsed_ms": elapsed_ms,
                    "steps": steps,
                    "logs": captured.lines,
                    "logs_total": captured.total,
                    "logs_truncated": captured.truncated,
                    "error": error_object(ctx, &err),
                }),
                code,
            )
        }
    }
}

/// Formats a dry-run preview for human-readable CLI output (`cqs run --dry-run` / `cqs batch --dry-run`).
pub fn format_dry_run_text(ctx: &Context, payload: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(action) = payload.get("action") {
        let id = action["id"].as_str().unwrap_or("");
        let label = action["label"].as_str().unwrap_or("");
        let risk = action["risk"].as_str().unwrap_or("");
        let root = action["root"].as_bool().unwrap_or(false);
        let mutates = action["mutates"].as_bool().unwrap_or(false);
        lines.push(format!("[DRY-RUN] {id}  {label}"));
        lines.push(format!("  risk={risk} mutates={mutates} root={root}"));
        if let Some(params) = payload.get("parameters").and_then(Value::as_object)
            && !params.is_empty()
        {
            let pairs = params
                .iter()
                .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("  parameters: {pairs}"));
        }
        if let Some(blockers) = payload
            .pointer("/preflight/blockers")
            .and_then(Value::as_array)
            && !blockers.is_empty()
        {
            for b in blockers {
                if let Some(msg) = b.get("message").and_then(Value::as_str) {
                    lines.push(format!("  blocker: {msg}"));
                }
            }
        }
    } else if let Some(jobs) = payload.get("jobs").and_then(Value::as_array) {
        lines.push(format!("[DRY-RUN] batch ({} actions)", jobs.len()));
        for (i, job) in jobs.iter().enumerate() {
            let id = job["action"]["id"].as_str().unwrap_or("");
            let label = job["action"]["label"].as_str().unwrap_or("");
            let risk = job["action"]["risk"].as_str().unwrap_or("");
            lines.push(format!("  {}. {id}  {label} (risk={risk})", i + 1));
            if let Some(params) = job.get("parameters").and_then(Value::as_object)
                && !params.is_empty()
            {
                let pairs = params
                    .iter()
                    .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!("     parameters: {pairs}"));
            }
            if let Some(blockers) = job.pointer("/preflight/blockers").and_then(Value::as_array) {
                for b in blockers {
                    if let Some(msg) = b.get("message").and_then(Value::as_str) {
                        lines.push(format!("     blocker: {msg}"));
                    }
                }
            }
        }
    } else if let Some(msg) = payload.pointer("/error/message").and_then(Value::as_str) {
        lines.push(msg.to_owned());
    } else {
        lines.push(ctx.i18n.text("ui.ready").to_owned());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx(dir: &std::path::Path) -> Context {
        crate::tests::context(dir)
    }

    #[test]
    fn execution_policy_validates_groups_actions_and_enforces_risk_boundaries() {
        let mut bad_group = ExecutionPolicy::mcp_default();
        bad_group.allow_groups = vec!["nonexistent".into()];
        assert_eq!(bad_group.validate().unwrap_err().key, "error.input");

        let mut bad_action = ExecutionPolicy::mcp_default();
        bad_action.deny_actions = vec!["nope.id".into()];
        assert_eq!(bad_action.validate().unwrap_err().key, "error.action");

        let mut scoped = ExecutionPolicy::mcp_default();
        scoped.allow_groups = vec!["system".into(), "network".into()];
        scoped.deny_actions = vec!["github.proxy".into()];
        scoped.validate().unwrap();

        let bbr = actions::find("bbr.enable").unwrap();
        let gh = actions::find("github.proxy").unwrap();
        let ssh = actions::find("ssh.port").unwrap();
        let clean = actions::find("system.clean").unwrap();
        let info = actions::find("system.info").unwrap();

        assert!(scoped.check_action(&bbr, &bbr.defaults()).is_ok());
        assert_eq!(
            scoped.check_action(&gh, &gh.defaults()).unwrap_err().key,
            "error.policy"
        );
        assert_eq!(
            scoped.check_action(&ssh, &ssh.defaults()).unwrap_err().key,
            "error.policy"
        );

        // Conditional danger: `system.clean` is standard by default, danger when `scope=full`.
        let default_mcp = ExecutionPolicy::mcp_default();
        assert!(default_mcp.check_action(&clean, &clean.defaults()).is_ok());
        let full_clean = Parameters::from([("scope".into(), "full".into())]);
        assert_eq!(
            default_mcp
                .check_action(&clean, &full_clean)
                .unwrap_err()
                .key,
            "error.policy"
        );
        assert_eq!(
            default_mcp
                .check_action(&ssh, &ssh.defaults())
                .unwrap_err()
                .key,
            "error.policy"
        );

        let mut allow_danger = ExecutionPolicy::mcp_default();
        allow_danger.allow_danger = true;
        assert!(allow_danger.check_action(&clean, &full_clean).is_ok());
        assert!(allow_danger.check_action(&ssh, &ssh.defaults()).is_ok());

        let mut ro = ExecutionPolicy::mcp_default();
        ro.read_only = true;
        assert!(ro.check_action(&info, &info.defaults()).is_ok());
        assert_eq!(
            ro.check_action(&bbr, &bbr.defaults()).unwrap_err().key,
            "error.policy"
        );

        // DockerOp policy checks across allow_groups, read_only, and danger tiers.
        let restart = DockerOp::ContainerRestart {
            ids: vec!["web".into()],
            names: vec!["web".into()],
        };
        let vol_rm = DockerOp::VolumeRemove {
            names: vec!["pg-data".into()],
        };
        let bulk_rm = DockerOp::ContainerRemove {
            ids: vec!["c1".into(), "c2".into(), "c3".into()],
            names: vec!["c1".into(), "c2".into(), "c3".into()],
            force: false,
        };
        assert!(default_mcp.check_docker_op(&restart).is_ok());
        assert_eq!(
            default_mcp.check_docker_op(&vol_rm).unwrap_err().key,
            "error.policy"
        );
        assert_eq!(
            default_mcp.check_docker_op(&bulk_rm).unwrap_err().key,
            "error.policy"
        );
        assert!(allow_danger.check_docker_op(&vol_rm).is_ok());
        assert_eq!(
            ro.check_docker_op(&restart).unwrap_err().key,
            "error.policy"
        );
        assert_eq!(
            scoped.check_docker_op(&restart).unwrap_err().key,
            "error.policy"
        );

        let temp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(temp.path());
        let pol_json = scoped.to_json(&ctx);
        assert_eq!(pol_json["schema_version"], SCHEMA_VERSION);
        assert_eq!(pol_json["ok"], true);
        assert_eq!(
            pol_json["policy"]["allow_groups"],
            json!(["system", "network"])
        );
        assert_eq!(pol_json["guarantees"]["secrets_redacted"], true);
        assert_eq!(pol_json["guarantees"]["auto_reboot_disabled"], true);
    }

    #[test]
    fn exit_code_and_error_envelope_map_every_error_category() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(temp.path());
        assert_eq!(exit_code(&Error::new("error.cancelled", "")), 130);
        for key in ["error.input", "error.action", "error.required", "error.cli"] {
            assert_eq!(exit_code(&Error::new(key, "x")), 2, "{key}");
        }
        for key in [
            "error.confirm",
            "error.policy",
            "error.tty",
            "error.command",
        ] {
            assert_eq!(exit_code(&Error::new(key, "x")), 1, "{key}");
        }
        let cancelled = error_envelope(&ctx, &Error::new("error.cancelled", "sigint"));
        assert_eq!(cancelled["ok"], false);
        assert_eq!(cancelled["status"], "cancelled");
        assert_eq!(cancelled["error"]["code"], "cancelled");
        assert_eq!(cancelled["error"]["exit_code"], 130);

        let failed = error_envelope(&ctx, &Error::new("error.input", "bad"));
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["error"]["exit_code"], 2);
    }

    #[test]
    fn catalog_filters_schemas_and_choices_cover_all_action_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(temp.path());

        let all = list_actions_json(&ctx, &ActionFilter::default(), None);
        assert_eq!(all["total"], 106);

        let non_int = list_actions_json(
            &ctx,
            &ActionFilter {
                non_interactive_only: true,
                ..Default::default()
            },
            None,
        );
        assert_eq!(non_int["total"], 100);

        let mut ro_policy = ExecutionPolicy::mcp_default();
        ro_policy.read_only = true;
        let filtered = list_actions_json(
            &ctx,
            &ActionFilter {
                group: Some("network".into()),
                query: Some("BBR".into()),
                mutates: Some(false),
                root: Some(false),
                non_interactive_only: true,
            },
            Some(&ro_policy),
        );
        let actions_arr = filtered["actions"].as_array().unwrap();
        assert_eq!(actions_arr.len(), 1);
        assert_eq!(actions_arr[0]["id"], "bbr.status");
        assert_eq!(actions_arr[0]["allowed_by_policy"], true);

        // Detail schema: `allow_custom` uses `examples`, fixed enum uses `enum`, dynamic sources serialize cleanly.
        let tz = action_detail_json(&ctx, actions::find_ref("system.timezone").unwrap());
        assert!(
            tz["action"]["input_schema"]["properties"]["timezone"]
                .get("examples")
                .is_some()
        );
        let select = action_detail_json(&ctx, actions::find_ref("clash.proxy.select").unwrap());
        assert_eq!(select["action"]["has_dynamic_choices"], true);
        assert_eq!(
            select["action"]["parameters"][0]["dynamic_source"],
            json!({ "kind": "listing", "name": "proxy_groups" })
        );
        assert_eq!(
            select["action"]["parameters"][1]["dynamic_source"],
            json!({ "kind": "children", "parent": "proxy_group" })
        );

        // Choices: static, dynamic + children note, no-choice action, and unknown action.
        let bbr_choices = action_choices_json(&ctx, "bbr.enable").unwrap();
        assert_eq!(bbr_choices["fields"][0]["source"]["kind"], "static");
        let sel_choices = action_choices_json(&ctx, "clash.proxy.select").unwrap();
        assert_eq!(sel_choices["fields"].as_array().unwrap().len(), 2);
        assert!(
            sel_choices["fields"][1]["note"]
                .as_str()
                .unwrap()
                .contains("proxy_group")
        );
        let info_choices = action_choices_json(&ctx, "system.info").unwrap();
        assert!(info_choices["fields"].as_array().unwrap().is_empty());
        assert_eq!(
            action_choices_json(&ctx, "missing.id").unwrap_err().key,
            "error.action"
        );
    }

    #[test]
    fn json_parameter_coercion_and_batch_parsing_handle_valid_and_invalid_inputs() {
        let preview = actions::find("terminal.preview").unwrap();
        assert!(
            parameters_from_json(&preview, &Value::Null)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            parameters_from_json(&preview, &json!(["not", "an", "object"]))
                .unwrap_err()
                .key,
            "error.input"
        );
        let parsed = parameters_from_json(
            &preview,
            &json!({ "style": "box", "time": true, "git": false, "shell": null }),
        )
        .unwrap();
        assert_eq!(parsed["style"], "box");
        assert_eq!(parsed["time"], "on");
        assert_eq!(parsed["git"], "off");
        assert!(!parsed.contains_key("shell"));

        let ssh = actions::find("ssh.port").unwrap();
        let port = parameters_from_json(&ssh, &json!({ "port": 2222 })).unwrap();
        assert_eq!(port["port"], "2222");
        let bool_non_onoff = parameters_from_json(&ssh, &json!({ "port": true })).unwrap();
        assert_eq!(bool_non_onoff["port"], "true");
        let bool_false = parameters_from_json(&ssh, &json!({ "port": false })).unwrap();
        assert_eq!(bool_false["port"], "false");
        assert_eq!(
            parameters_from_json(&ssh, &json!({ "port": { "nested": 1 } }))
                .unwrap_err()
                .key,
            "error.input"
        );

        // Batch JSON parsing
        let defaults = batch_jobs_from_json(&["terminal.preview"], &Value::Null).unwrap();
        assert_eq!(defaults.len(), 1);
        assert_eq!(
            batch_jobs_from_json(&["terminal.preview"], &json!(123))
                .unwrap_err()
                .key,
            "error.input"
        );
        assert_eq!(
            batch_jobs_from_json(
                &["terminal.preview"],
                &json!({ "bbr.enable": { "qdisc": "fq" } })
            )
            .unwrap_err()
            .key,
            "error.input"
        );
        let jobs = batch_jobs_from_json(
            &["terminal.preview", "github.proxy"],
            &json!({
                "terminal.preview": { "style": "compact", "time": false },
                "github.proxy": { "gh_proxy_state": false }
            }),
        )
        .unwrap();
        assert_eq!(jobs[0].1["time"], "off");
        assert_eq!(jobs[1].1["gh_proxy_state"], "off");
    }

    #[test]
    fn execute_action_batch_drain_logs_and_dry_run_text_cover_all_branches() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(temp.path());
        let policy = ExecutionPolicy::mcp_default();

        // Unknown action & invalid params
        let (unknown, code) = execute_action_json(
            &ctx,
            &policy,
            "nope.action",
            &Parameters::new(),
            false,
            false,
        );
        assert_eq!(code, 2);
        assert_eq!(unknown["error"]["key"], "error.action");

        let bad_p = Parameters::from([("qdisc".into(), "invalid".into())]);
        let (invalid, code) =
            execute_action_json(&ctx, &policy, "bbr.enable", &bad_p, false, false);
        assert_eq!(code, 2);
        assert_eq!(invalid["status"], "failed");

        // Empty batch & duplicate batch IDs & interactive TTY rejection in batch
        let (empty, code) = execute_batch_json(&ctx, &policy, &[], false, false);
        assert_eq!(code, 2);
        assert_eq!(empty["error"]["key"], "error.input");

        let tp = actions::find("terminal.preview").unwrap();
        let (dup, code) = execute_batch_json(
            &ctx,
            &policy,
            &[(tp.clone(), tp.defaults()), (tp.clone(), tp.defaults())],
            false,
            false,
        );
        assert_eq!(code, 2);
        assert!(
            dup["error"]["detail"]
                .as_str()
                .unwrap()
                .contains("duplicate")
        );

        let tip = actions::find("test.ip").unwrap();
        let (tty_batch, code) =
            execute_batch_json(&ctx, &policy, &[(tip.clone(), tip.defaults())], true, false);
        assert_eq!(code, 1);
        assert_eq!(tty_batch["error"]["key"], "error.tty");

        // Log truncation in `drain_logs`
        let (tx, rx) = std::sync::mpsc::channel();
        for idx in 0..(MAX_CAPTURED_LOGS + 25) {
            tx.send(Event::Log(format!("line-{idx}"))).unwrap();
        }
        drop(tx);
        let drained = drain_logs(rx);
        assert_eq!(drained.lines.len(), MAX_CAPTURED_LOGS);
        assert_eq!(drained.total, MAX_CAPTURED_LOGS + 25);
        assert!(drained.truncated);

        // `format_dry_run_text` across all 4 payload variants
        let single = json!({
            "action": { "id": "bbr.enable", "label": "Enable BBR", "risk": "reversible", "root": true, "mutates": true },
            "parameters": { "qdisc": "fq_codel" },
            "preflight": { "blockers": [{ "message": "Root required" }] }
        });
        let s_txt = format_dry_run_text(&ctx, &single);
        assert!(s_txt.contains("[DRY-RUN] bbr.enable"));
        assert!(s_txt.contains("qdisc=fq_codel"));
        assert!(s_txt.contains("blocker: Root required"));

        let batch_payload = json!({
            "jobs": [{
                "action": { "id": "bbr.enable", "label": "Enable BBR", "risk": "reversible" },
                "parameters": { "qdisc": "fq" },
                "preflight": { "blockers": [{ "message": "Debian/Ubuntu only" }] }
            }]
        });
        let b_txt = format_dry_run_text(&ctx, &batch_payload);
        assert!(b_txt.contains("[DRY-RUN] batch (1 actions)"));
        assert!(b_txt.contains("qdisc=fq"));
        assert!(b_txt.contains("blocker: Debian/Ubuntu only"));

        let err_txt = format_dry_run_text(&ctx, &json!({ "error": { "message": "boom" } }));
        assert_eq!(err_txt, "boom");

        let fallback_txt = format_dry_run_text(&ctx, &json!({}));
        assert_eq!(fallback_txt, ctx.i18n.text("ui.ready"));
    }
}
