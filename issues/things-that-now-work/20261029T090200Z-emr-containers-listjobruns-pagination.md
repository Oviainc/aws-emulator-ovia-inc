Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# decode_list_cursor accepts cursor spellings encode_list_cursor never writes ('+'-signed and leading-zero decimals), so a token this service never issued is served as a page position instead of refused

_Filed: 2026-10-29T09:02:00Z_
_Operation: ListJobRuns | Class: pagination_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `@paginated` listing of this crate (ListJobRuns, ListJobTemplates, ListManagedEndpoints, ListSecurityConfigurations, ListVirtualClusters) resumes through `crate::state::page_of`, which decodes the client's `nextToken` with `crate::state::decode_list_cursor`. That decoder documents itself as refusing anything that is "not a cursor this `scope` issued", and `page_of` repeats the rule. But it parses the two numeric components with `str::parse`, which accepts spellings the one encoder (`encode_list_cursor`, a plain `format!("{scope}:{created_at_millis}:{sequence}")` over `i64`/`u64` Display) can never produce: a leading `+` sign and leading zeros.

So `?nextToken=<scope>:+0:+0` or `?nextToken=<scope>:0000:0000` is NOT refused — it is read as a position in the newest-first order and answered 200. Because the cursor scan is `position(|record| record.list_position() <= cursor)`, a fabricated position older than every stored record makes `start == ordered.len()`, so the listing reports an EMPTY page with no `nextToken`: a token the service never minted silently hides every record of the result set instead of earning the modelled `ValidationException` that every other forged token earns.

The crate's own tests pin the refusal for the other forged shapes (`notatoken`, `1-1`, `12345`, `1:2`, `1:2:3`, `a:b`, a cursor from another listing's scope) but never for a scope-correct token with a non-canonical numeric spelling, which is why the divergence from the decoder's documented rule is invisible today.

## Evidence

The one encoder — only canonical decimals can ever be produced (`services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/state.rs:412-414`):
```rust
pub fn encode_list_cursor(scope: &str, created_at_millis: i64, sequence: u64) -> String {
    format!("{scope}:{created_at_millis}:{sequence}")
}
```
The decoder's own documented rule (`src/state.rs:416-419`):
```rust
/// The place in a newest-first order a `nextToken` names.
///
/// `None` — refused as an invalid request — when the token is not a cursor this `scope`
/// issued, so a token cannot be carried from one listing to another's.
```
…and the code under it, which accepts `"+5"` and `"0005"` for both components (`src/state.rs:420-428`):
```rust
pub fn decode_list_cursor(scope: &str, token: &str) -> Option<(i64, u64)> {
    let rest = token.strip_prefix(scope)?.strip_prefix(':')?;
    let mut parts = rest.split(':');
    let (Some(created_at_millis), Some(sequence), None) = (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    Some((created_at_millis.parse().ok()?, sequence.parse().ok()?))
}
```
`page_of` restates the rule it then fails to hold (`src/state.rs:526-528`): "A token no [`decode_list_cursor`] of this `scope` accepts is refused, so a cursor one listing minted is never read as a position in another's order", and the scan that turns an accepted fabrication into a silent empty page (`src/state.rs:548-561`):
```rust
            let cursor = decode_list_cursor(scope, token).ok_or(PageRefusal::UnknownNextToken)?;
            ordered
                .iter()
                .position(|record| record.list_position() <= cursor)
                .unwrap_or(ordered.len())
```
The caller that maps a refusal to the modelled error, so the path is live for this operation (`src/ops/list_job_runs.rs:201-215`):
```rust
        let page = crate::state::page_of(
            &input.virtual_cluster_id,
            ...
        )
        .map_err(|refusal| {
            ListJobRunsError::from(ListJobRunsErrorModeled::ValidationException(
```
Test coverage that pins the refusal for every OTHER forged spelling but not this one — `tests/all/virtual_cluster_job_run_loop.rs:999-1001`:
```rust
    emr.list_jobs(&cluster, "?nextToken=notatoken")
        .await
        .refused("a bogus nextToken", 400, "ValidationException");
```
and `tests/all/virtual_cluster_job_run_loop.rs:1061-1063`:
```rust
    emr.list_jobs(&cluster, "?nextToken=1-1")
        .await
        .refused("a fabricated numeric token", 400, "ValidationException");
```
plus the shared forged-token table in `tests/all/paginated_page_bounds.rs:348-369` (`not-a-cursor`, `12345`, `1:2`, `a:b`, a foreign cursor) — no row uses a `+`-signed or leading-zero component.

## Steps to Reproduce

1. Create a virtual cluster (`POST /virtualclusters`) and note its id `VC`.
2. Start two job runs on it (`POST /virtualclusters/VC/jobruns`).
3. `GET /virtualclusters/VC/jobruns` — both runs are reported.
4. `GET /virtualclusters/VC/jobruns?nextToken=1-1` — refused 400 ValidationException "nextToken is not a valid pagination token" (the documented answer for a token this service never issued; pinned by tests/all/virtual_cluster_job_run_loop.rs:1061).
5. `GET /virtualclusters/VC/jobruns?nextToken=VC:+0:+0` — a spelling `encode_list_cursor` can never produce, yet answered 200 with `jobRuns: []` and no `nextToken`, silently hiding both runs instead of refusing the token.
6. Same with `?nextToken=VC:0000:0000`, and the identical behaviour on `GET /jobtemplates?nextToken=jobtemplates:+0:+0`, `GET /securityconfigurations?nextToken=securityconfigurations:+0:+0`, `GET /virtualclusters?nextToken=virtualclusters:+0:+0` and `GET /virtualclusters/VC/endpoints?nextToken=endpoints:VC:+0:+0`.
