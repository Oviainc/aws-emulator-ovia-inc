Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: d204f113841562be393bb0d029d4ee5d3cf48852

# The report URL's host resolution never falls back to the URI authority when a Host header is present but not a bare authority

_Filed: 2026-10-28T23:14:38Z_
_Operation: GetSuiteRunReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d204f113841562be393bb0d029d4ee5d3cf48852`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetSuiteRunReport` builds the announced `qualificationReportDownloadUrl` around `crate::state::report_url_host`, whose own documentation states a three-step chain: the request's `Host` header, ELSE the authority its URI carries, ELSE the loopback host. The code applies the "only a bare authority is honoured" filter AFTER the `or_else`, so the chain is not what the doc says: as soon as a `Host` header is present and readable as UTF-8, the URI-authority source is skipped forever, and if that header value then fails `is_bare_authority` (it carries a path, query, fragment, userinfo, backslash or whitespace) the function drops straight to `127.0.0.1` instead of consulting the authority the request URI carries.

The result is an announced URL that does not resolve back to the endpoint the caller reached — exactly the property the function exists to guarantee ("a client addressing the emulator by socket is handed that socket"). Note the inconsistency inside the same expression: a `Host` header whose bytes are not UTF-8 DOES fall through to the URI authority (`to_str().ok()` yields `None`, so `or_else` runs), while a readable-but-non-bare `Host` does not. Two kinds of unusable `Host` header are treated differently, and only one of them follows the documented chain.

## Evidence

The documented rule — services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/state.rs:1331-1338:
```
/// The endpoint the request reached this emulator on, as a report URL names it: the
/// request's own `Host`, else the authority its URI carries, else [`LOOPBACK_HOST`].
///
/// Read from the request rather than built from the model's endpoint template, because
/// the URL has to resolve for whoever was handed it: a client addressing the emulator by
/// socket is handed that socket, and a literal AWS endpoint here would send a following
/// client to the real service. The value is the CALLER's, and it is interpolated into a
/// URL this service announces, so only a bare authority is honoured.
```
The code under it — src/state.rs:1344-1356, where `.filter(..)` sits AFTER `.or_else(..)`, so the second source is unreachable whenever the first produced any UTF-8 value:
```
pub fn report_url_host(parts: &::http::request::Parts) -> String {
    parts
        .headers
        .get(::http::header::HOST)
        // The URI's AUTHORITY, not its host: a request that carries its endpoint on the
        // URI carries the port there too, and a URL announced without the port would not
        // resolve back to the socket the caller reached.
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.authority().map(::http::uri::Authority::as_str))
        .filter(|candidate| is_bare_authority(candidate))
        .unwrap_or(LOOPBACK_HOST)
        .to_string()
}
```
The filter that rejects the value — src/state.rs:1362-1366:
```
fn is_bare_authority(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['/', '?', '#', '@', '\\', ' ', '\t'])
        && value.parse::<::http::uri::Authority>().is_ok()
}
```
The single consumer — src/ops/get_suite_run_report.rs:158-160:
```
        // The endpoint the caller reached this emulator on, because that is the endpoint
        // the URL announced below has to resolve back to.
        let host = crate::state::report_url_host(ctx.parts());
```
Test coverage: tests/all/suite_run_report.rs:40 pins ONE host form only (`const REQUEST_HOST: &str = "api.iotdeviceadvisor.us-east-1.amazonaws.com";`, asserted at :146-154), and no case in that file sends a `Host` header that fails `is_bare_authority` or a request whose authority lives only on the URI, so the fallback order is unpinned rather than deliberately chosen.

## Steps to Reproduce

1. Start the emulator and create a suite definition with `intendedForQualification: true` (`POST /suiteDefinitions`).
2. Seed a successful run of that definition in the store (status `PASS`), as tests/all/suite_run_report.rs:77-106 `seed_run` does, so that `GetSuiteRunReport` can answer 200.
3. Issue `GET /suiteDefinitions/{id}/suiteRuns/{runId}/report` with an absolute-form request URI carrying the authority `api.iotdeviceadvisor.us-east-1.amazonaws.com` AND a `Host` header that is not a bare authority (e.g. `Host: api.iotdeviceadvisor.us-east-1.amazonaws.com/x`).
4. Observed: `qualificationReportDownloadUrl` is `https://127.0.0.1/qualificationReports/{id}/{runId}` — the URI authority the request carried was never consulted.
5. Expected by the documented chain at src/state.rs:1331-1332 ("the request's own `Host`, else the authority its URI carries, else LOOPBACK_HOST"): the URI authority is used, since the `Host` value was not honoured.
6. Contrast with the same request carrying a `Host` header of invalid UTF-8 bytes: there `to_str().ok()` returns `None`, the `or_else` branch runs, and the URI authority IS used — the two unusable-header cases take different paths.
