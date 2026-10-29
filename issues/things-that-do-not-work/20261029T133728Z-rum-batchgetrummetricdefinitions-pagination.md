Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: e9dbd7f28045b442d217f817f50d4dbc748f9e32

# decode_pagination_token honours uppercase/mixed-case hex spellings encode_pagination_token never writes, so a NextToken this service never issued is served as a page

_Filed: 2026-10-29T13:37:28Z_
_Operation: BatchGetRumMetricDefinitions | Class: pagination_

> _Note: this issue was identified against the code base at commit `e9dbd7f28045b442d217f817f50d4dbc748f9e32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this service mints its `NextToken` through `crate::state::encode_pagination_token`, which hex-encodes the marker plus the resume key with `hex::encode` — a function that only ever emits LOWERCASE hex digits. The matching reader, `decode_pagination_token`, accepts the token through `hex::decode`, which accepts upper-, lower- and mixed-case hex digits alike. So `nextToken=4261746368...` and `nextToken=4261746368...` re-spelled with any subset of its `a`-`f` digits uppercased decode to the same bytes, pass the marker and arity checks, and are served as a resume point — although no page of this service ever handed that value out.

That contradicts the decoder's own contract ("`None` for any value that operation never issued") and the codec's own premise that a client "must pass back what it was handed rather than construct one": a client that upper-cases, or an intermediary that normalises case, gets a 200 page where this service's one token rule says it must get the modelled `ValidationException`. The defect is in the shared codec, so all four paginated readers (`BatchGetRumMetricDefinitions`, `ListRumMetricsDestinations`, `ListAppMonitors`, `GetAppMonitorData`) inherit it.

## Evidence

The one minting site writes lowercase hex only — `services/service-com-amazonaws-rum-rum/src/state.rs:2955-2958`:
```
pub fn encode_pagination_token(marker: &str, resume_after: &[&str]) -> String {
    let key = resume_after.join(&PAGINATION_TOKEN_SEPARATOR.to_string());
    hex::encode(format!("{marker}{key}"))
}
```
with the premise, at `state.rs:2950-2953`: "Hex keeps the token printable and opaque to a client — `NextToken` is an unconstrained `String`, so a client must pass back what it was handed rather than construct one".

The reader's contract, `state.rs:2960-2961`:
```
/// The `components` of the key a token issued by [`encode_pagination_token`] under
/// `marker` resumes after, or `None` for any value that operation never issued.
```
and the code that breaks it, `state.rs:2973` — `hex::decode` is case-insensitive by definition (the `hex` crate's decoder accepts `0-9`, `a-f` AND `A-F`), so a spelling the encoder never writes round-trips:
```
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let resume_after = decoded.strip_prefix(marker)?;
```
The only gate after that is the marker, the arity and a non-empty first component (`state.rs:2978-2984`), none of which is case-sensitive about the HEX layer.

Test coverage: `services/service-com-amazonaws-rum-rum/tests/all/batch_get_metric_definitions_reads.rs:584-603` claims to enumerate the failure space — "One row per way a value can fail to be a token THIS listing issued for THIS target. The forged ones are built with the `hex` codec the service's token codec uses" — with rows for `"not-a-token"`, `hex::encode("whatever/1/CheckoutsCompleted0")`, `hex::encode("ListAppMonitors/1/token-rules")`, `hex::encode("BatchGetRumMetricDefinitions/1/")` and a token issued for another monitor, all asserted at `:611-622` to answer `VALIDATION_STATUS`/`VALIDATION_CODE`. There is no row for a case-altered spelling of an issued token, which is why the gap is unpinned. The sibling suite states the same rule: `tests/all/metrics_destinations.rs:681` `"expected HTTP 400 for a token this service never issued"`.

## Steps to Reproduce

1. `CreateAppMonitor` with `Name=token-rules`, then `PutRumMetricsDestination` with `Destination=CloudWatch`, then `BatchCreateRumMetricDefinitions` with 3 definitions for that destination.
2. `GET /rummetrics/token-rules/metrics?destination=CloudWatch&maxResults=1` and copy the returned `NextToken` (a lowercase hex string).
3. Upper-case the token's hex digits (e.g. `token.to_ascii_uppercase()`), a spelling `encode_pagination_token` can never produce.
4. `GET /rummetrics/token-rules/metrics?destination=CloudWatch&maxResults=1&nextToken=<UPPERCASED>`.
5. Observed: HTTP 200 with the second page, i.e. the uppercased value is accepted as a resume point. Expected per `decode_pagination_token`'s doc and the suites quoted above: HTTP 400 `ValidationException` ("NextToken {token} is not a pagination token this service issued for app monitor …", batch_get_rum_metric_definitions.rs:205-215), because the service never issued that value.
