Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 9a3acde976cfb747ba29bb78f67d8041a784763f

# A GetAssetPropertyAggregates nextToken carries only a window start, so a token minted for another property or another aggregate selection silently truncates the page instead of being refused

_Filed: 2026-10-29T03:06:29Z_
_Operation: GetAssetPropertyAggregates | Class: pagination_

> _Note: this issue was identified against the code base at commit `9a3acde976cfb747ba29bb78f67d8041a784763f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This operation's `nextToken` is nothing but the window-start second of the first aggregate of the next page, hex-encoded. On the way back in, the only check is that some aggregate of THIS enumeration starts at that second; nothing binds the token to the request that produced it — not the stream (assetId/propertyId or propertyAlias), not the `resolution`, not `aggregateTypes`, not `qualities`, not `timeOrdering`, not the date range.

Because window starts are aligned to the resolution grid by construction, a token minted while paging property A at resolution 1h is found in property B's 1h enumeration whenever B has an aggregate for that same hour. The handler then serves B's aggregates FROM that offset: every window of B before the resume point is dropped from this page and from every later page of that enumeration, and the caller is told 200. The same happens for a token replayed against a different `aggregateTypes`/`qualities`/ordering of the same property, where the page content is a different enumeration altogether.

The crate already states the opposite rule for its sibling batch reader, which deliberately puts a fingerprint of the request beside the position precisely so a token cannot be honoured against another request. This handler's own comment says a token "whose resume point is not in this enumeration is refused rather than answered with an empty page" — the refusal it intends is exactly the one that fails here, because a foreign token's resume point usually IS in this enumeration.

## Evidence

The token and the whole of its validation:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/get_asset_property_aggregates.rs:380-397 —
    "// `nextToken` is this enumeration's resume point: the window start of the first\n        // aggregate of the next page, hex-encoded so the token matches the modelled\n        // `^[A-Za-z0-9+/=]+$`. A token whose resume point is not in this enumeration is\n        // refused rather than answered with an empty page.\n        let first = match input.next_token.as_deref() {\n            Some(token) => {\n                let resume = decode_token(token)?;\n                aggregated\n                    .iter()\n                    .position(|value| value.timestamp.secs() == resume)\n                    .ok_or_else(invalid_token)?\n            }\n            None => 0,\n        };"
  and the codec, src/ops/get_asset_property_aggregates.rs:425-436 — "fn encode_token(window_start: i64) -> String { ::hex::encode(window_start.to_string().as_bytes()) }" / "fn decode_token(token: &str) -> Result<i64, ...> { ::hex::decode(token) ... .and_then(|text| text.parse::<i64>().ok()) ... }" — nothing but an integer, so the token carries no identity of the request.

The rule this crate states for the same problem one operation over:
  src/ops/batch_get_asset_property_value.rs:386-389 — "/// The token carries the FINGERPRINT of the entry list beside the position, so a\n/// token is only honoured against the request that produced it: replayed against a\n/// different list it names no page, rather than silently re-paging that other list\n/// and reporting its leading entries as already processed."
  and its decoder, src/ops/batch_get_asset_property_value.rs:396-401 — "let (at, fingerprint) = ::std::str::from_utf8(&decoded).ok()?.split_once('|')?;\n    (fingerprint == entries_fingerprint(entries)).then_some(())?;"

Test coverage: tests/all/single_property_aggregates.rs:765-771 is the only `nextToken` refusal pinned for this operation, and it forges a token whose resume point is in no window at all — "(\n            \"a nextToken this operation never issued\",\n            // Hex of \"7\": a well-formed token whose resume point is in no window.\n            with(vec![(\"nextToken\", \"37\".to_string())]),\n            400,\n            \"InvalidRequestException\",\n        )". A token that IS a window start of another request's enumeration is pinned nowhere, while the sibling's binding is pinned (tests/all/batch_get_latest_values.rs).

## Steps to Reproduce

1. Create an asset with two DOUBLE measurement properties A and B.
2. BatchPutAssetPropertyValue several readings for A and several for B whose timestamps fall in the same hours (e.g. the same TQV timestamps for both), so both properties have aggregates in at least two 1h windows.
3. GET /properties/aggregates?assetId=<asset>&propertyId=<A>&resolution=1h&aggregateTypes=AVERAGE&startDate=<t0>&endDate=<t0+1d>&maxResults=1 -> 200 with a nextToken (hex of the next window's start second).
4. GET /properties/aggregates?assetId=<asset>&propertyId=<B>&resolution=1h&aggregateTypes=AVERAGE&startDate=<t0>&endDate=<t0+1d>&nextToken=<that token>.
5. Observed: 200. B's aggregates are served starting at the window the token names; B's earlier window(s) are absent from this page and unreachable from this enumeration, with no indication that the token belonged to another request.
6. Expected, per the rule the crate states for its sibling reader (src/ops/batch_get_asset_property_value.rs:386-389) and per this handler's own intent at src/ops/get_asset_property_aggregates.rs:382-383: a token replayed against a different stream/resolution/aggregate selection names no page of this enumeration and is refused 400 InvalidRequestException "The nextToken is not a token this operation issued."
7. The same replay across differing `aggregateTypes`, `qualities` or `timeOrdering` on one property is accepted the same way.
