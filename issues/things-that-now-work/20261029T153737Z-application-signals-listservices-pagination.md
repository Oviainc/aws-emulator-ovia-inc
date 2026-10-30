Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08

# decode_cursor honours uppercase/mixed-case hex NextToken spellings encode_cursor never mints, so a token this service never issued resumes a page

_Filed: 2026-10-29T15:37:37Z_
_Operation: ListServices | Class: pagination_

> _Note: this issue was identified against the code base at commit `95be350e2e64bcff4ad89a5a8e5d1ae15d5b4a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated read of this service mints its `NextToken` through `crate::state::encode_cursor`, which hex-encodes the tag plus the cursor with `hex::encode` — a function that only ever emits LOWERCASE hex digits. The matching reader, `crate::state::decode_cursor`, takes the token through `hex::decode`, which accepts upper-, lower- and mixed-case hex digits alike. So an issued token and the same token with any subset of its `a`-`f` digits upper-cased decode to the same bytes, pass the tag-prefix check, parse as the same offset, and are served as a resume point — although no page of this service ever handed that value out.

That breaks `decode_cursor`'s own stated contract ("`None` when it is not a token this service issued under that tag") and the service's one token rule, which `state.rs` words as "The refusal every read of this service reports for a `NextToken` it did not issue". A client that upper-cases the token, or an intermediary that normalises its case, gets a 200 page where the crate's own rule says it must get the modelled `ValidationException`.

The defect is in the shared codec, so every read that resolves a cursor inherits it: `ListServices`, `ListServiceOperations`, `ListServiceDependencies`, `ListServiceDependents`, `ListServiceStates`, `ListEntityEvents`, `ListAuditFindings`, `ListServiceLevelObjectiveExclusionWindows` (all through `page_of`), plus `ListServiceLevelObjectives`, `ListInstrumentationConfigurations` and `GetInstrumentationConfigurationStatus`, which call `decode_cursor` directly. `ListServices` is named here because it is the read the crate's own token suite uses as the issuer of a real token.

## Evidence

The ONE minting site emits lowercase hex only — `services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:1403-1407`:
```
/// A `NextToken` carrying `cursor` under `tag`: hex, so the caller reads it as the opaque
/// value the model documents rather than as a store key it could edit.
pub fn encode_cursor(tag: &str, cursor: &str) -> String {
    ::hex::encode(format!("{tag}{cursor}"))
}
```

The reader states the rule it breaks, `state.rs:1409-1414`:
```
/// The cursor `token` carries under `tag`, or `None` when it is not a token this service
/// issued under that tag.
pub fn decode_cursor(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(tag).map(ToString::to_string)
}
```
`hex::decode` is case-insensitive by definition (the `hex` crate's decoder accepts `0-9`, `a-f` AND `A-F`; `hex` is a direct dependency, `Cargo.lock:3177`), so a spelling `encode_cursor` can never write round-trips through it. Nothing after that line is case-sensitive about the HEX layer: the only further gate is the tag prefix and the offset parse, `state.rs:1540-1545`:
```
    let offset = match token {
        None => 0,
        Some(token) => decode_cursor(tag, token)
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .ok_or_else(|| UNISSUED_NEXT_TOKEN.to_string())?,
    };
```

The rule the crate says this must obey, `state.rs:1416-1428`:
```
/// The refusal every read of this service reports for a `NextToken` it did not issue.
///
/// ONE wording for ONE failure, whichever read resolved the cursor — the token is opaque to
/// the caller ([`encode_cursor`]), so "this service did not issue it" is the whole
/// requirement a client can act on
...
pub const UNISSUED_NEXT_TOKEN: &str = "NextToken is not a token this service issued";
```

The affected handler site, `src/ops/list_services.rs:220-226`:
```
        let (service_summaries, next_token) = crate::state::page_of(
            observed,
            &token_tag,
            input.next_token.as_deref(),
            page_size,
        )
        .map_err(refused)?;
```

Test coverage: the crate's token suite claims to enumerate the failure space but has no case-altered row. `tests/all/unissued_next_token.rs:23-26`: "The columns differ along WHY the token is not one this service issued: it is empty, it is not hex, it is hex of bytes that are not text, it is well-formed but carries no marker of this service, it is long, it looks like a credential a log must not keep — and it is a token this service really DID issue, for another read." The rows themselves, `tests/all/unissued_next_token.rs:261-288`, are `""`, `"not-a-token"`, `"abc"`, `"ff"`, `hex::encode("forged/1/0")`, `hex::encode("services/1/first")`, `hex::encode("x".repeat(512))`, `"AKIAIOSFODNN7EXAMPLE"` and a token issued for another read — with `:260` noting "the rest are forged with `hex::encode`, the one hex encoder in this tree (the same call `state::encode_cursor` makes)". No row re-spells an issued token in another case, which is why the gap is unpinned.

## Steps to Reproduce

1. Seed more than one discovered service, as `tests/all/unissued_next_token.rs:87-102` does: `POST /slo` three times with `Name=cursor-alpha-latency` / `cursor-beta-latency` / `cursor-gamma-latency`, each with an `SliConfig` whose `SliMetricConfig.KeyAttributes` names `{"Type":"Service","Name":"cursor-<n>","Environment":"prod"}`.
2. `GET /services?StartTime=<now-3600>&EndTime=<now+3600>&MaxResults=1` and copy the returned `NextToken` (a lowercase hex string, e.g. `7365727669636573...`).
3. Upper-case the token's hex digits (`token.to_ascii_uppercase()`) — a spelling `state::encode_cursor` can never produce, since `hex::encode` only writes `0-9a-f`.
4. `GET /services?StartTime=<now-3600>&EndTime=<now+3600>&MaxResults=1&NextToken=<UPPERCASED>`.
5. Observed: HTTP 200 with the second page — the uppercased value is honoured as a cursor. Expected per `decode_cursor`'s own doc (`state.rs:1409-1410`) and `UNISSUED_NEXT_TOKEN` (`state.rs:1416-1428`): HTTP 400 `ValidationException` with message `NextToken is not a token this service issued`, because this service never issued that value.
6. The same substitution resumes a page on every other cursor-resolving read too (`/service-operations`, `/service-dependencies`, `/service-dependents`, `/service/states`, `/events`, `/auditFindings`, `/slo/{Id}/exclusion-windows`, `/slos`, `/list-instrumentation-configurations`, `/get-instrumentation-configuration-status`), since all of them go through the same codec.
