Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# A nextToken carries only a timestamp, so a token minted for another property (or another quality filter / ordering) silently truncates the page instead of being refused

_Filed: 2026-10-29T04:00:28Z_
_Operation: GetAssetPropertyValueHistory | Class: pagination_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This operation's `nextToken` is nothing but the `(seconds, nanos)` key of the first sample of the next page, hex-encoded. On the way back in the only check is that SOME sample of the enumeration being served carries that exact timestamp; nothing binds the token to the request that minted it — not the stream (assetId/propertyId or propertyAlias), not `qualities`, not `timeOrdering`, not the date range.

Because BatchPutAssetPropertyValue lets a caller write explicit timestamps (and a batch typically writes the SAME timestamps to several properties), a token minted while paging property A is found in property B's enumeration whenever B holds a sample at that instant. The handler then serves B from that offset: every sample of B before the resume point is dropped from this page and from every later page of the enumeration, and the caller is answered 200. The same happens for a token replayed against the same property with `timeOrdering` flipped or with a different `qualities` filter, where the enumeration is a different list altogether and the found position means something else.

The handler's own comment states the refusal it intends — "A token whose resume point is not in this enumeration is refused rather than answered with an empty page, which would report absence as success" — and the crate states the binding rule explicitly for its sibling batch reader, which puts a fingerprint of the request beside the position precisely so a token cannot be honoured against another request. This is the same defect already filed for GetAssetPropertyAggregates (a different operation, different token codec); the aggregates issue's own text does not cover this operation.

## Evidence

The token and the whole of its validation — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/get_asset_property_value_history.rs:271-291:
```
        // `nextToken` is this enumeration's resume point: the timestamp of the first
        // sample of the next page, hex-encoded so the token matches the modelled
        // `^[A-Za-z0-9+/=]+$`. A token whose resume point is not in this enumeration is
        // refused rather than answered with an empty page, which would report absence
        // as success.
        let start = match input.next_token.as_deref() {
            Some(token) => {
                let resume =
                    crate::state::decode_timestamp_token(token).ok_or_else(invalid_token)?;
                selected
                    .iter()
                    .position(|sample| crate::state::timestamp_key(&sample.timestamp) == resume)
                    .ok_or_else(invalid_token)?
            }
            None => 0,
        };
```
and the codec, which carries nothing but the instant — src/state.rs:6589-6610: "The opaque pagination token that resumes at the [`timestamp_key`] `resume`: the hex of `seconds.nanos`." … `::hex::encode(format!("{seconds}{TIMESTAMP_TOKEN_SEPARATOR}{nanos}").as_bytes())`.

The selection the position is taken in, which the request's own members decide — src/ops/get_asset_property_value_history.rs:212-218 (`HistoryQuery::new(input.start_date, input.end_date, input.qualities, input.time_ordering == DESCENDING)`) and :268-269 (`crate::state::history_values(&store, &stream, &query)`).

The rule this crate states for the same problem one operation over — src/ops/batch_get_asset_property_value.rs:386-389:
```
/// The token carries the FINGERPRINT of the entry list beside the position, so a
/// token is only honoured against the request that produced it: replayed against a
/// different list it names no page, rather than silently re-paging that other list
/// and reporting its leading entries as already processed.
```

Test coverage: the only refusals pinned for this operation forge structurally invalid or out-of-range tokens — tests/all/property_value_history.rs:399-417: `("not hex", "zzzz"), ("hex of nothing timestamp-shaped", "6162"), ("a timestamp no sample carries", "392e30")`. A token that IS a timestamp of ANOTHER request's enumeration is pinned nowhere, while the sibling's binding is pinned (tests/all/batch_get_latest_values.rs:645-675).

## Steps to Reproduce

1. Create an asset with two DOUBLE measurement properties A and B.
2. BatchPutAssetPropertyValue several readings to A and to B at the SAME timestamps (one batch with two entries, identical TQV timestamps).
3. GET /properties/history?assetId=<asset>&propertyId=<A>&maxResults=1 -> 200 with a nextToken (hex of "seconds.nanos" of A's second sample).
4. GET /properties/history?assetId=<asset>&propertyId=<B>&nextToken=<that token> -> 200. Observed: B's history is served starting at that instant; B's earlier sample(s) are absent from this page and unreachable from this enumeration, with no indication the token belonged to another request.
5. Variant with one property: mint a token with timeOrdering=ASCENDING, then replay it with timeOrdering=DESCENDING (or with a different `qualities` filter) -> 200 with a slice of a different enumeration.
6. Expected, per the rule the crate states for its sibling reader (src/ops/batch_get_asset_property_value.rs:386-389) and per this handler's own intent at src/ops/get_asset_property_value_history.rs:273-275: a token replayed against a different stream/quality/ordering names no page of this enumeration and is refused 400 InvalidRequestException "The nextToken is not a token this operation issued."
7. Note: GetCaptureData decodes the same codec (src/ops/get_capture_data.rs:265) and is exposed the same way for two capture sources of one workspace whose segments start at the same instant.
