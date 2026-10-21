// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Resource-listener surfaces: how a generated emulator safely opens extra ports.
//!
//! A generated service (e.g. RDS `CreateDBInstance`, API Gateway
//! `CreateDeployment`) sometimes needs a listener a real client connects to —
//! a Postgres port for `psql`, an HTTPS port for `execute-api`. Agents never
//! bind a socket themselves: they hand a handler to [`Listeners::tcp`] or
//! [`Listeners::http`], and the core owns every transport-safety property.
//!
//! The security model rests on where a browser can reach:
//! a web page cannot open a raw socket, so a loopback TCP port is unreachable
//! from the browser; but it CAN send HTTP(S) requests to any localhost port, so
//! the HTTP path must reject cross-site requests itself — CORS only hides the
//! response, it does not stop the request arriving.
//!
//! This module is the pure, protocol-agnostic heart of that enforcement. The
//! decisions here are deliberately free of I/O so they are exhaustively unit
//! tested; the live listeners in `listeners.rs` call into them.

/// Per-connection and per-request caps the core applies to resource
/// listeners. Each cap is scoped to the transport where it is meaningful:
/// `MAX_CONNECTIONS` bounds concurrent `tcp()` connections (a native server
/// could otherwise accept unboundedly); `MAX_BYTES_PER_CONN` bounds a single
/// `http()` request body; `IDLE_TIMEOUT_SECS` drops a `tcp()` connection that
/// makes no read/write progress for that long.
pub mod caps {
    /// Max concurrent connections a single `tcp()` listener accepts.
    pub const MAX_CONNECTIONS: usize = 256;
    /// Max concurrent in-flight `http()` requests. Deliberately much smaller than
    /// [`MAX_CONNECTIONS`] because each in-flight request may buffer a body up to
    /// [`MAX_BYTES_PER_CONN`]; this bounds peak HTTP body memory at
    /// `MAX_HTTP_INFLIGHT * MAX_BYTES_PER_CONN` rather than at the connection count.
    pub const MAX_HTTP_INFLIGHT: usize = 16;
    /// Max bytes read from one `http()` request body before it is rejected 413.
    pub const MAX_BYTES_PER_CONN: u64 = 64 * 1024 * 1024;
    /// Hard deadline for one `http()` request (body read + handler), kept well
    /// under [`IDLE_TIMEOUT_SECS`] so a slow-drip client cannot hold one of the
    /// few [`MAX_HTTP_INFLIGHT`] in-flight permits for minutes and starve others.
    pub const HTTP_REQUEST_DEADLINE_SECS: u64 = 30;
    /// Idle timeout (seconds) before a silent `tcp()` connection is dropped.
    pub const IDLE_TIMEOUT_SECS: u64 = 300;
}

/// Which cross-origin browser requests the operator has opted into. Resolved
/// once at boot from `--remote-origin` and never read from the environment in
/// request-path code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteOrigins {
    /// Default: only same-origin (and non-browser) requests are served.
    SameOriginOnly,
    /// Exactly these origins are additionally allowed. Never applies to the
    /// control-plane or `/__admin` hosts (the caller enforces that).
    Exact(Vec<String>),
}

impl RemoteOrigins {
    fn allows(&self, origin: &str) -> bool {
        match self {
            RemoteOrigins::SameOriginOnly => false,
            RemoteOrigins::Exact(list) => list.iter().any(|o| o == origin),
        }
    }
}

/// The verdict of the pre-handler browser check applied to every HTTP request
/// on a resource-listener (or control-plane) port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserVerdict {
    /// Serve the request.
    Allow,
    /// Refuse with 403 before any handler runs; the string is the reason.
    Forbid(&'static str),
}

/// Decide whether an HTTP request may be served, from the browser-set headers.
///
/// Rules:
/// - No `Origin` AND no `Sec-Fetch-Site` → a non-browser client (CLI, SDK): allow.
/// - `Sec-Fetch-Site: same-origin` or `none` → allow.
/// - `Sec-Fetch-Site: same-site` or `cross-site` → treat as cross-site.
/// - An `Origin` present is allowed only when it is in `remote_origins`.
/// - `Origin: null` (sandboxed iframe, `file://`, some redirects) is cross-site.
///
/// `allow_remote` is false for the control-plane and `/__admin` hosts, so a
/// configured remote origin can never reach them.
pub fn classify_browser_request(
    origin: Option<&str>,
    sec_fetch_site: Option<&str>,
    remote_origins: &RemoteOrigins,
    allow_remote: bool,
) -> BrowserVerdict {
    // Non-browser clients send neither header: nothing to spoof, so serve them.
    if origin.is_none() && sec_fetch_site.is_none() {
        return BrowserVerdict::Allow;
    }

    // Sec-Fetch-Site is the browser's own, unforgeable classification. Trust it
    // first when present.
    match sec_fetch_site {
        Some("same-origin") | Some("none") => return BrowserVerdict::Allow,
        Some("same-site") | Some("cross-site") => {
            // Fall through to the Origin allowlist below: an operator may have
            // opted this exact origin in.
        }
        Some(_) => return BrowserVerdict::Forbid("unrecognized Sec-Fetch-Site"),
        None => {
            // No Sec-Fetch-* (older browsers). A browser also sends Origin on a
            // same-origin unsafe-method request, but with no self-origin to
            // compare here we fail closed and decide on Origin alone below.
        }
    }

    match origin {
        // `null` is never same-origin; refuse it.
        Some("null") => BrowserVerdict::Forbid("null origin"),
        Some(o) if allow_remote && remote_origins.allows(o) => BrowserVerdict::Allow,
        Some(_) => BrowserVerdict::Forbid("cross-site origin"),
        // Sec-Fetch-Site said same-site/cross-site but no Origin header: refuse.
        None => BrowserVerdict::Forbid("cross-site request"),
    }
}

/// True when the request is a WebSocket upgrade (or HTTP/2 extended CONNECT),
/// which the core refuses with 501 at GA.
pub fn is_websocket_upgrade(
    connection: Option<&str>,
    upgrade: Option<&str>,
    http2_protocol: Option<&str>,
) -> bool {
    let header_upgrade = connection
        .map(|c| c.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false)
        && upgrade
            .map(|u| u.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false);
    let extended_connect = http2_protocol
        .map(|p| p.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    header_upgrade || extended_connect
}

/// True when the first bytes of a raw TCP connection look like an HTTP request
/// line or the HTTP/2 preface. On a `tcp()` listener the TLS-only listener
/// already rejects a browser's cross-protocol HTTP POST at the handshake, so
/// this is not on the live serve path; it is exported for a test that asserts
/// a plaintext HTTP request is refused.
pub fn looks_like_http_preface(first_bytes: &[u8]) -> bool {
    const METHODS: &[&[u8]] = &[
        b"GET ", b"POST ", b"PUT ", b"HEAD ", b"DELETE ", b"OPTIONS ", b"PATCH ", b"TRACE ",
        b"CONNECT ",
    ];
    const H2_PREFACE: &[u8] = b"PRI * HTTP/2";
    if first_bytes.starts_with(H2_PREFACE) {
        return true;
    }
    METHODS.iter().any(|m| first_bytes.starts_with(m))
}

/// Recognize a loopback host — `127.0.0.0/8`, `::1`, and the literal name
/// `localhost`. Used by the HTTP `Host` allowlist: a loopback bind accepts only
/// a loopback Host (the DNS-rebinding guard), and a routable bind additionally
/// accepts its own address (see `host_allowed`).
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    let h = h.split('%').next().unwrap_or(h); // strip IPv6 zone id
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(v4) = h.parse::<std::net::Ipv4Addr>() {
        return v4.octets()[0] == 127;
    }
    if let Ok(v6) = h.parse::<std::net::Ipv6Addr>() {
        return v6.is_loopback();
    }
    false
}

/// Error returned by [`Listeners`] calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceError {
    /// Binding or accepting on the loopback socket failed.
    Bind(String),
    /// Generating or configuring the listener's TLS material failed.
    Tls(String),
    /// `EMULATOR_LISTENER_PORT_RANGE` was set to something that is not a port or
    /// a `LOW-HIGH` range.
    InvalidPortRange(String),
    /// A port range was configured but every port in it was already in use.
    PortRangeExhausted(String),
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SurfaceError::Bind(e) => write!(f, "listener bind failed: {e}"),
            SurfaceError::Tls(e) => write!(f, "listener TLS setup failed: {e}"),
            SurfaceError::InvalidPortRange(e) => write!(f, "invalid listener port range: {e}"),
            SurfaceError::PortRangeExhausted(e) => write!(f, "listener port range exhausted: {e}"),
        }
    }
}

impl std::error::Error for SurfaceError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn same_origin_only() -> RemoteOrigins {
        RemoteOrigins::SameOriginOnly
    }

    #[test]
    fn non_browser_client_is_allowed() {
        // No Origin, no Sec-Fetch-*: a CLI/SDK call.
        assert_eq!(
            classify_browser_request(None, None, &same_origin_only(), true),
            BrowserVerdict::Allow
        );
    }

    #[test]
    fn same_origin_and_none_pass() {
        for sfs in ["same-origin", "none"] {
            assert_eq!(
                classify_browser_request(Some("https://x.localhost"), Some(sfs), &same_origin_only(), true),
                BrowserVerdict::Allow,
                "sec-fetch-site={sfs}"
            );
        }
    }

    #[test]
    fn same_site_and_cross_site_are_refused_by_default() {
        for sfs in ["same-site", "cross-site"] {
            assert!(
                matches!(
                    classify_browser_request(Some("https://evil.example"), Some(sfs), &same_origin_only(), true),
                    BrowserVerdict::Forbid(_)
                ),
                "sec-fetch-site={sfs} must be refused"
            );
        }
    }

    #[test]
    fn null_origin_is_cross_site() {
        assert!(matches!(
            classify_browser_request(Some("null"), Some("cross-site"), &same_origin_only(), true),
            BrowserVerdict::Forbid(_)
        ));
        // Even if an operator allowlist exists, `null` never matches it.
        let allow = RemoteOrigins::Exact(vec!["null".to_string()]);
        assert!(matches!(
            classify_browser_request(Some("null"), Some("cross-site"), &allow, true),
            BrowserVerdict::Forbid(_)
        ));
    }

    #[test]
    fn exact_remote_origin_allowed_only_when_opted_in_and_permitted() {
        let allow = RemoteOrigins::Exact(vec!["http://localhost:5173".to_string()]);
        // Opted in AND host permits remote origins -> allowed.
        assert_eq!(
            classify_browser_request(Some("http://localhost:5173"), Some("cross-site"), &allow, true),
            BrowserVerdict::Allow
        );
        // Same request on a control-plane/admin host (allow_remote=false) -> refused.
        assert!(matches!(
            classify_browser_request(Some("http://localhost:5173"), Some("cross-site"), &allow, false),
            BrowserVerdict::Forbid(_)
        ));
        // A different origin not in the list -> refused.
        assert!(matches!(
            classify_browser_request(Some("http://evil.example"), Some("cross-site"), &allow, true),
            BrowserVerdict::Forbid(_)
        ));
    }

    #[test]
    fn older_browser_without_sec_fetch_decides_on_origin() {
        // No Sec-Fetch-* but a cross origin present: refuse.
        assert!(matches!(
            classify_browser_request(Some("http://evil.example"), None, &same_origin_only(), true),
            BrowserVerdict::Forbid(_)
        ));
    }

    #[test]
    fn websocket_upgrade_detected() {
        assert!(is_websocket_upgrade(Some("Upgrade"), Some("websocket"), None));
        assert!(is_websocket_upgrade(Some("keep-alive, Upgrade"), Some("WebSocket"), None));
        assert!(is_websocket_upgrade(None, None, Some("websocket"))); // h2 extended CONNECT
        assert!(!is_websocket_upgrade(Some("keep-alive"), None, None));
        assert!(!is_websocket_upgrade(None, Some("h2c"), None));
    }

    #[test]
    fn http_preface_detected_but_not_binary_protocols() {
        assert!(looks_like_http_preface(b"GET / HTTP/1.1\r\n"));
        assert!(looks_like_http_preface(b"POST /x HTTP/1.1"));
        assert!(looks_like_http_preface(b"PRI * HTTP/2.0\r\n"));
        // Postgres SSLRequest: 8 bytes, length 0x00000008 then 0x04d2162f.
        assert!(!looks_like_http_preface(&[0x00, 0x00, 0x00, 0x08, 0x04, 0xd2, 0x16, 0x2f]));
        // Redis inline PING and a RESP array both start non-HTTP.
        assert!(!looks_like_http_preface(b"*1\r\n$4\r\nPING\r\n"));
    }

    #[test]
    fn loopback_hosts_recognized() {
        for h in ["127.0.0.1", "127.5.6.7", "::1", "localhost", "[::1]", "::1%lo0"] {
            assert!(is_loopback_host(h), "{h} should be loopback");
        }
        for h in ["0.0.0.0", "10.0.0.1", "::", "169.254.169.254", "example.com"] {
            assert!(!is_loopback_host(h), "{h} must not be loopback");
        }
    }
}

/// Environment variable a customer sets to control which loopback ports the
/// resource listeners (`tcp()`/`http()`) bind. Unset (or `0`) means an
/// OS-assigned ephemeral port per listener. A value of `LOW-HIGH` (or a single
/// port `N`, meaning `N-N`) confines every listener to that range, so a customer
/// can pre-open a firewall or map container ports deterministically.
///
/// Named in the `EMULATOR_*` family (alongside `EMULATOR_AUTHENTICATION`,
/// `EMULATOR_TELEMETRY`), the customer-facing emulator configuration.
pub const LISTENER_PORT_RANGE_ENV: &str = "EMULATOR_LISTENER_PORT_RANGE";

/// How the core chooses a loopback port for a resource listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortRange {
    /// OS-assigned ephemeral port (bind `:0`) — the default.
    Ephemeral,
    /// Bind the first free port in the inclusive range `[low, high]`.
    Range(u16, u16),
}

/// Pure parser for [`LISTENER_PORT_RANGE_ENV`], split out so it is unit-testable
/// without touching process-global environment variables.
///
/// `None`, empty, or `"0"` yield [`PortRange::Ephemeral`] — an OS-assigned
/// ephemeral port. `"LOW-HIGH"` yields [`PortRange::Range`] with
/// `1 <= LOW <= HIGH <= 65535`; a single `"N"` yields `Range(N, N)`. Anything
/// else is refused with [`SurfaceError::InvalidPortRange`] rather than silently
/// defaulting — the customer asked for specific ports, so a bad value must be
/// visible, not quietly turned back into an ephemeral port they cannot predict.
pub fn parse_listener_port_range(raw: Option<&str>) -> Result<PortRange, SurfaceError> {
    let raw = match raw {
        Some(v) => v.trim(),
        None => return Ok(PortRange::Ephemeral),
    };
    if raw.is_empty() || raw == "0" {
        return Ok(PortRange::Ephemeral);
    }
    // A port is 1..=65535 written as plain digits. Reject any sign or other
    // non-digit character up front, so `u16::from_str`'s acceptance of a leading
    // `+` cannot sneak `"+5000"` through a contract that promises digits only.
    let port = |s: &str| {
        let s = s.trim();
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse::<u16>().ok().filter(|p| *p != 0)
    };
    let invalid = || {
        SurfaceError::InvalidPortRange(format!(
            "{LISTENER_PORT_RANGE_ENV}={raw:?} must be a port 1-65535 or a LOW-HIGH range \
             with 1 <= LOW <= HIGH <= 65535"
        ))
    };
    match raw.split_once('-') {
        Some((low, high)) => match (port(low), port(high)) {
            (Some(low), Some(high)) if low <= high => Ok(PortRange::Range(low, high)),
            _ => Err(invalid()),
        },
        None => port(raw).map(|p| PortRange::Range(p, p)).ok_or_else(invalid),
    }
}

#[cfg(test)]
mod port_range_tests {
    use super::*;

    #[test]
    fn unset_empty_and_zero_mean_ephemeral() {
        for raw in [None, Some(""), Some("  "), Some("0"), Some(" 0 ")] {
            assert_eq!(parse_listener_port_range(raw).unwrap(), PortRange::Ephemeral, "{raw:?}");
        }
    }

    #[test]
    fn single_port_is_a_one_port_range() {
        assert_eq!(parse_listener_port_range(Some("42000")).unwrap(), PortRange::Range(42000, 42000));
        assert_eq!(parse_listener_port_range(Some(" 8080 ")).unwrap(), PortRange::Range(8080, 8080));
    }

    #[test]
    fn low_high_range_parses() {
        assert_eq!(parse_listener_port_range(Some("42000-42099")).unwrap(), PortRange::Range(42000, 42099));
        assert_eq!(parse_listener_port_range(Some("1-65535")).unwrap(), PortRange::Range(1, 65535));
    }

    #[test]
    fn invalid_values_are_refused_not_defaulted() {
        for raw in ["99-10", "0-100", "100-0", "-100", "100-", "+5000", "+5-+10", "5-+10", "nonsense", "70000", "10-70000", "1-2-3", "12.0"] {
            assert!(
                matches!(parse_listener_port_range(Some(raw)), Err(SurfaceError::InvalidPortRange(_))),
                "expected refusal for {raw:?}"
            );
        }
    }
}
