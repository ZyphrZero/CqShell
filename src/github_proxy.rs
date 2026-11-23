//! GitHub acceleration proxy validation, URL rewriting, precedence resolution, and persistence.

use crate::{
    config::Preferences,
    error::{Error, Result},
    runtime::Context,
};

/// Default GitHub acceleration proxy prefix offered when the user enables acceleration.
pub const DEFAULT_URL: &str = "https://gh-proxy.com/";

const GITHUB_HOSTS: &[&str] = &[
    "github.com",
    "api.github.com",
    "raw.githubusercontent.com",
    "gist.githubusercontent.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// Returns `true` when `raw` is a canonical HTTPS GitHub URL eligible for proxy acceleration.
pub fn is_github_url(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    GITHUB_HOSTS
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
}

/// Returns `true` when `raw` is a canonical HTTPS `api.github.com` metadata endpoint.
pub fn is_github_api_url(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("api.github.com"))
}

/// Validates and normalizes a GitHub acceleration proxy prefix to a canonical HTTPS URL ending
/// with a single `/`.
pub fn normalize(raw: &str) -> Result<String> {
    let Some(after_scheme) = raw.strip_prefix("https://") else {
        return Err(Error::new(
            "error.url",
            "GitHub proxy must be an HTTPS URL without credentials, query, fragment or whitespace",
        ));
    };
    if after_scheme.is_empty()
        || after_scheme.starts_with('/')
        || raw
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '\\' | '?' | '#' | '@'))
    {
        return Err(Error::new(
            "error.url",
            "GitHub proxy must be an HTTPS URL without credentials, query, fragment or whitespace",
        ));
    }
    let parsed = url::Url::parse(raw).map_err(|e| Error::new("error.url", e))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port() == Some(0)
    {
        return Err(Error::new(
            "error.url",
            "GitHub proxy must be an HTTPS URL without credentials, query or fragment",
        ));
    }
    let trimmed = parsed.as_str().trim_end_matches('/');
    let normalized = format!("{trimmed}/");
    // Re-parse to ensure trimming never yields a malformed URL.
    let verified = url::Url::parse(&normalized).map_err(|e| Error::new("error.url", e))?;
    if verified.scheme() != "https" || verified.host_str().is_none() {
        return Err(Error::new("error.url", raw));
    }
    Ok(normalized)
}

/// Parses a CLI `--gh-proxy` or environment (`CQS_GH_PROXY` / `GH_PROXY`) override.
///
/// Accepts `off`/`none`/`false`/`direct`/`0` to explicitly disable acceleration,
/// `default`/`on`/`true` to enable the default proxy (`https://gh-proxy.com/`), or a trusted
/// HTTPS prefix URL.
pub fn parse_override(raw: &str) -> Result<Option<String>> {
    if raw.is_empty() || raw.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err(Error::new(
            "error.url",
            "GitHub proxy override must be off, default, or a valid HTTPS URL",
        ));
    }
    match raw.to_ascii_lowercase().as_str() {
        "off" | "none" | "false" | "direct" | "0" => Ok(None),
        "default" | "on" | "true" => Ok(Some(DEFAULT_URL.to_owned())),
        _ => normalize(raw).map(Some),
    }
}

/// Resolves the active GitHub proxy from CLI flag > `CQS_GH_PROXY` > `GH_PROXY` > `Preferences`.
///
/// By default (when neither CLI, environment, nor config enables it), acceleration is disabled
/// (`Ok(None)`) so hosts outside China never send traffic through a third-party proxy implicitly.
pub fn resolve(
    flag: Option<&str>,
    cqs_env: Option<&str>,
    gh_env: Option<&str>,
    preferences: &Preferences,
) -> Result<Option<String>> {
    if let Some(raw) = flag {
        return parse_override(raw);
    }
    if let Some(raw) = cqs_env.filter(|value| !value.is_empty()) {
        return parse_override(raw);
    }
    if let Some(raw) = gh_env.filter(|value| !value.is_empty()) {
        return parse_override(raw);
    }
    from_preferences(preferences)
}

/// Resolves the active GitHub proxy strictly from saved [`Preferences`].
pub fn from_preferences(preferences: &Preferences) -> Result<Option<String>> {
    match preferences.github_proxy_enabled {
        Some(false) => Ok(None),
        Some(true) => {
            let raw = preferences
                .github_proxy
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_URL);
            normalize(raw).map(Some)
        }
        None => match preferences
            .github_proxy
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            Some(raw) => normalize(raw).map(Some),
            None => Ok(None),
        },
    }
}

/// Returns the saved proxy URL (or [`DEFAULT_URL`] when unset) for pre-populating UI forms.
pub fn configured_url(preferences: &Preferences) -> String {
    preferences
        .github_proxy
        .as_deref()
        .filter(|value| !value.is_empty())
        .and_then(|value| normalize(value).ok())
        .unwrap_or_else(|| DEFAULT_URL.to_owned())
}

/// Rewrites `url` through `proxy` when `proxy` is set and `url` targets a supported GitHub host.
/// Non-GitHub URLs are always returned unchanged.
pub fn rewrite(url: &str, proxy: Option<&str>) -> String {
    let Some(proxy) = proxy.filter(|value| !value.is_empty()) else {
        return url.to_owned();
    };
    if !is_github_url(url) || url.starts_with(proxy) {
        return url.to_owned();
    }
    format!("{proxy}{url}")
}

/// Persists GitHub acceleration proxy settings into `config.toml` and logs the active state.
pub fn configure(ctx: &Context, enabled: bool, raw_url: &str) -> Result<()> {
    let normalized = if raw_url.is_empty() && !enabled {
        let existing = Preferences::read(&ctx.paths.preferences, false)?;
        configured_url(&existing)
    } else {
        normalize(raw_url)?
    };
    let mut preferences = Preferences::read(&ctx.paths.preferences, false)?;
    preferences.github_proxy_enabled = Some(enabled);
    preferences.github_proxy = Some(normalized.clone());
    preferences.write(&ctx.paths.preferences)?;
    if enabled {
        ctx.message("notice.github_proxy_on", &normalized);
    } else {
        ctx.message("notice.github_proxy_off", &normalized);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_https_prefixes_and_normalizes_trailing_slash() {
        assert_eq!(
            normalize("https://gh-proxy.com").unwrap(),
            "https://gh-proxy.com/"
        );
        assert_eq!(
            normalize("https://gh-proxy.com/").unwrap(),
            "https://gh-proxy.com/"
        );
        assert_eq!(
            normalize("https://gh-proxy.com///").unwrap(),
            "https://gh-proxy.com/"
        );
        assert_eq!(
            normalize("https://mirror.example:8443/gh").unwrap(),
            "https://mirror.example:8443/gh/"
        );
        assert_eq!(
            normalize("https://mirror.example/gh///").unwrap(),
            "https://mirror.example/gh/"
        );
    }

    #[test]
    fn normalize_rejects_insecure_or_malformed_urls() {
        for invalid in [
            "",
            "http://gh-proxy.com/",
            "https://",
            "https:///gh-proxy.com",
            "https://user@gh-proxy.com/",
            "https://user:pass@gh-proxy.com/",
            "https://gh-proxy.com/?token=1",
            "https://gh-proxy.com/#frag",
            "https://gh-proxy.com/\\path",
            "https://gh-proxy.com/\n",
            "https://gh-proxy.com/ path",
            "https://gh-proxy.com:0/",
            "https://gh-proxy.com:65536/",
            "not-a-url",
        ] {
            let err = normalize(invalid).unwrap_err();
            assert_eq!(err.key, "error.url", "expected error.url for {invalid:?}");
        }
    }

    #[test]
    fn rewrite_targets_only_canonical_github_https_hosts() {
        let proxy = Some(DEFAULT_URL);
        for url in [
            "https://github.com/starship/starship/releases/download/v1.0.0/starship.tar.gz",
            "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest",
            "https://raw.githubusercontent.com/xykt/IPQuality/abc/ip.sh",
            "https://gist.githubusercontent.com/user/id/raw/file",
            "https://codeload.github.com/owner/repo/tar.gz/main",
            "https://objects.githubusercontent.com/github-production-release-asset/1",
            "https://release-assets.githubusercontent.com/asset/1",
        ] {
            assert_eq!(rewrite(url, proxy), format!("{DEFAULT_URL}{url}"));
        }
        // Non-GitHub, HTTP, credentialed, or suffix-spoofed hosts must stay untouched.
        for untouched in [
            "https://downloads.claude.ai/claude-code-releases/latest",
            "https://ipv4.ip.sb",
            "https://ipinfo.io/json",
            "https://get.docker.com",
            "https://linuxmirrors.cn/docker.sh",
            "https://evil-github.com/repo/file",
            "https://github.com.evil.example/repo/file",
            "http://github.com/owner/repo",
            "https://user@github.com/owner/repo",
            "https://localhost:8443/asset",
        ] {
            assert_eq!(rewrite(untouched, proxy), untouched, "{untouched}");
        }
        assert_eq!(
            rewrite("https://github.com/owner/repo", None),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn precedence_prefers_flag_then_cqs_env_then_gh_env_then_preferences() {
        let mut prefs = Preferences::default();
        assert_eq!(resolve(None, None, None, &prefs).unwrap(), None);

        prefs.github_proxy_enabled = Some(true);
        assert_eq!(
            resolve(None, None, None, &prefs).unwrap().as_deref(),
            Some(DEFAULT_URL)
        );

        prefs.github_proxy = Some("https://saved.example/".into());
        assert_eq!(
            resolve(None, None, None, &prefs).unwrap().as_deref(),
            Some("https://saved.example/")
        );

        assert_eq!(
            resolve(None, None, Some("https://gh-env.example"), &prefs)
                .unwrap()
                .as_deref(),
            Some("https://gh-env.example/")
        );
        assert_eq!(
            resolve(
                None,
                Some("https://cqs-env.example"),
                Some("https://gh-env.example"),
                &prefs
            )
            .unwrap()
            .as_deref(),
            Some("https://cqs-env.example/")
        );
        assert_eq!(
            resolve(
                Some("https://flag.example"),
                Some("https://cqs-env.example"),
                Some("https://gh-env.example"),
                &prefs
            )
            .unwrap()
            .as_deref(),
            Some("https://flag.example/")
        );

        // Explicit disable overrides saved preferences and lower-precedence env vars.
        assert_eq!(
            resolve(Some("off"), Some("https://cqs-env.example"), None, &prefs).unwrap(),
            None
        );
        assert_eq!(resolve(None, Some("none"), None, &prefs).unwrap(), None);
        assert_eq!(
            resolve(Some("default"), None, None, &Preferences::default())
                .unwrap()
                .as_deref(),
            Some(DEFAULT_URL)
        );

        // Disabled in preferences preserves saved URL without enabling it.
        prefs.github_proxy_enabled = Some(false);
        assert_eq!(resolve(None, None, None, &prefs).unwrap(), None);
        assert_eq!(configured_url(&prefs), "https://saved.example/");

        // Setting github_proxy alone in config.toml activates it.
        let url_only = Preferences {
            github_proxy: Some("https://custom.example".into()),
            ..Preferences::default()
        };
        assert_eq!(
            resolve(None, None, None, &url_only).unwrap().as_deref(),
            Some("https://custom.example/")
        );
    }
}
