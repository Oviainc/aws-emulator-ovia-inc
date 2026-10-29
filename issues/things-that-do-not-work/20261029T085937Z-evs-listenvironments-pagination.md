Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# decode_page_token validates only the token envelope, so a cursor this service never issued is honoured as a page position instead of refused

_Filed: 2026-10-29T08:59:37Z_
_Operation: ListEnvironments | Class: pagination_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this crate turns `nextToken` into a start cursor through `crate::state::decode_page_token`, which checks three things: that the token is hex, that the decoded text splits on `:` into a parsable i64 and a remainder, and that the i64 is within 24 hours. The remainder is then used verbatim as the page cursor with no check that it is a key this listing could ever have emitted.

Two consequences:

1. A token this service never issued — any `hex("<recent epoch ms>:<arbitrary text>")` — decodes successfully and is honoured. `ListEnvironments` passes it to `environments.range((Bound::Included(cursor), Unbounded))`, so a cursor sorting above every stored id answers HTTP 200 with an empty `environmentSummaries` and no `nextToken`, instead of the refusal the crate's own message text and its own test comment say such a token gets. The same holds for `ListEnvironmentConnectors`, `ListEnvironmentHosts` and `ListVmEntitlements`.

2. A token that WAS issued is bound to nothing, so replaying one across listings or selections silently truncates or empties a page. A `ListEnvironments` token (cursor `env-…`) fed to `ListEnvironmentHosts` hides every host whose `hostName` sorts below `env-…`; a `ListVmEntitlements` token minted for environment X (cursor `<connectorId>/<vmId>`) fed to environment Y hides every entitlement of Y whose connector id sorts below X's, answering 200 with an empty `entitlements` list a caller cannot tell from "this connector has no entitlement".

Only `ListEnvironmentVlans` does any domain check on the decoded cursor (it must parse as an i32); the other four accept anything.

## Evidence

The decoder, services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:3087-3098 — the only checks are hex, the `:` split, the i64 parse and the TTL:

    pub fn decode_page_token(token: &str) -> Result<String, PageTokenError> {
        let decoded = ::hex::decode(token)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or(PageTokenError::Malformed)?;
        let (issued_at_ms, cursor) = decoded.split_once(':').ok_or(PageTokenError::Malformed)?;
        let issued_at_ms: i64 = issued_at_ms.parse().map_err(|_| PageTokenError::Malformed)?;
        if now_ms().saturating_sub(issued_at_ms) > PAGE_TOKEN_TTL.as_millis() as i64 {
            return Err(PageTokenError::Expired);
        }
        Ok(cursor.to_string())
    }

The rule the crate states for that refusal, state.rs:3105-3112:

    PageTokenError::Malformed => "is not a pagination token this service issued",

The crate's own test states the same rule more broadly than the code enforces it — tests/all/environment_lifecycle.rs:1137-1140:

    // A token this service did not issue, and one it issued more than 24 hours ago,
    // are both refused rather than silently restarting the scan.
    let stale = ::hex::encode("0:env-0000000000");
    for (case, token) in [("malformed", "not-a-token".to_string()), ("expired", stale)] {

— the only "did not issue" case exercised is the NON-HEX string `"not-a-token"`; a hex-encoded forged cursor is never tried. tests/all/entitlements.rs:540 does the same: `request["nextToken"] = json!("not-a-token-this-service-issued");`.

The unchecked use of the cursor, src/ops/list_environments.rs:167-174 and 190-196:

    Some(token) => Some(crate::state::decode_page_token(token).map_err(|error| { ... })?),
    ...
            .range::<str, _>((
                match start_after.as_deref() {
                    Some(cursor) => std::ops::Bound::Included(cursor),

and src/ops/list_vm_entitlements.rs:248-256, where the cursor carries a `connectorId` the request can contradict:

        let mut matching = environment
            .entitlements
            .range::<str, _>((
                match start_at.as_deref() {
                    Some(cursor) => std::ops::Bound::Included(cursor),

The one sibling that does check the cursor's domain, src/ops/list_environment_vlans.rs:183-187:

                Some(cursor.parse::<i32>().map_err(|_| {
                    ListEnvironmentVlansErrorModeled::ValidationException(
                        crate::state::invalid_next_token(crate::state::PageTokenError::Malformed),
                    )
                })?)

## Steps to Reproduce

1. Create three environments so ListEnvironments has something to page over.
2. Build a token this service never issued: hex-encode the ASCII string "<current epoch millis>:env-zzzzzzzzzz" (for example `printf '%s' "1793263846445:env-zzzzzzzzzz" | xxd -p`).
3. Call ListEnvironments with that nextToken. It answers HTTP 200 with an empty environmentSummaries and no nextToken, instead of the HTTP 400 ValidationException state.rs:3109 words as "is not a pagination token this service issued".
4. Now take a REAL token: call ListEnvironments with maxResults=1 and keep the returned nextToken (cursor `env-…`).
5. Feed that token to ListEnvironmentHosts for a settled environment whose hosts are named e.g. "esx-0".."esx-3": every host sorting below the `env-…` cursor is skipped, so the page is truncated or empty and no error is reported.
6. Equivalently, mint a ListVmEntitlements nextToken against environment X and replay it against environment Y: when Y's connector id sorts below X's, Y's entitlements are all hidden behind an empty 200.
