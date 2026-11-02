Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 1f07a4a6c58d968bd98c0da95eee61e4f4add38f

# A nextToken is accepted against a DIFFERENT entries list, so entries this request never processed are reported as "completely processed by previous paginated requests" and their data is dropped

_Filed: 2026-10-29T00:45:57Z_
_Operation: BatchGetAssetPropertyAggregates | Class: pagination_

> _Note: this issue was identified against the code base at commit `1f07a4a6c58d968bd98c0da95eee61e4f4add38f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The paginated batch readers that page ACROSS entries decode `nextToken` into `crate::state::BatchCursor`, which carries only two positional numbers — the index into `entries` and how many data points of that entry earlier pages carried. The handler's only sanity check is `cursor.entry >= input.entries.len()`, so a token minted for request A is honoured against a completely different request B whose entry list happens to be at least that long: entries 0..cursor.entry of B are reported in `skippedEntries` with the completion status they resolve to — the handler's own comment calls them entries "that were not processed by this batch request, because these entries had been completely processed by previous paginated requests" — although B never processed them, and their aggregates are silently missing from this page and from every later page of B.

The third batch reader of this crate, BatchGetAssetPropertyValue, does exactly the check that is missing: its page token carries a fingerprint of the entry list beside the position, documented as "so a token is only honoured against the request that produced it: replayed against a different list it names no page, rather than silently re-paging that other list and reporting its leading entries as already processed" — the very failure mode the two BatchCursor-based readers allow, and a test pins the refusal for it. The same defect is in BatchGetAssetPropertyValueHistory, which reads the same cursor (BatchGetAssetPropertyAggregates is named here because `BatchCursor`'s doc names it as the first caller).

## Evidence

The loose decode, which binds a token to nothing but a position:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/batch_get_asset_property_aggregates.rs:204-210 —
    "        let cursor = match input.next_token.as_deref() {
            Some(token) => crate::state::BatchCursor::decode(token).ok_or_else(invalid_token)?,
            None => crate::state::BatchCursor::START,
        };
        if cursor.entry >= input.entries.len() {
            return Err(invalid_token());
        }"
  and what the cursor holds, src/state.rs:6626-6639 — "Where a page of a BATCH read resumes: the entry still being answered and how many of that entry's data points earlier pages already carried. ... pub struct BatchCursor { /// Index into the request's `entries` of the first entry not yet finished. pub entry: usize, /// How many of THAT entry's data points previous pages already reported."

What the handler then claims about the entries before the cursor:
  src/ops/batch_get_asset_property_aggregates.rs:229-243 — "            if at < cursor.entry {
                // \"entries that were not processed by this batch request, because
                // these entries had been completely processed by previous paginated
                // requests.\"
                skipped_entries.push(BatchGetAssetPropertyAggregatesSkippedEntry { ... });
                continue;
            }"

The sibling batch reader of this same crate that refuses such a token:
  src/ops/batch_get_asset_property_value.rs:174-188 — "A `nextToken` resumes THIS request's entry list at the entry the previous page stopped before, so it only means something against the SAME list: a token this service never issued, one replayed against a different entry list, or one past the end of the list it is replayed against names no page."
  src/ops/batch_get_asset_property_value.rs:386-401 — "The token carries the FINGERPRINT of the entry list beside the position, so a token is only honoured against the request that produced it: replayed against a different list it names no page, rather than silently re-paging that other list and reporting its leading entries as already processed." ... "fn decode_page_token(token: &str, entries: &[BatchGetAssetPropertyValueEntry]) -> Option<usize> { ... (fingerprint == entries_fingerprint(entries)).then_some(())?; at.parse().ok() }"

Test coverage: the sibling's refusal is pinned — tests/all/batch_get_latest_values.rs:645-675 ("A page token belongs to the request that produced it: replayed against a different entry list it names no page, instead of re-paging that other list" ... expects 400 for the token replayed against `reordered`). Nothing pins the loose behaviour here: tests/all/batch_property_value_history.rs:708-717 says "A token this operation never issued for these entries is refused rather than answered with an empty page" but every row it tries is malformed or out of range ("zzzz", hex of "ab", "9:0", "0:99", "a:b"); tests/all/property_aggregates.rs:973-987 likewise only forges structurally invalid tokens.

## Steps to Reproduce

1. Create an asset with two DOUBLE properties and write several readings to each, so each property feeds more than one aggregate window.
2. POST /properties/batch/aggregates with entries [A, B] (A and B naming different properties) and maxResults=1 -> 200 with a nextToken whose cursor is {entry:0 or 1, value:n}.
3. POST /properties/batch/aggregates again with a DIFFERENT entries list [C, D] (other properties, other entryIds) carrying that same nextToken.
4. Observed: 200. Entry C is reported in skippedEntries as having been "completely processed by previous paginated requests", and its aggregates never appear in any page of this enumeration; only D is answered.
5. Expected, per the sibling BatchGetAssetPropertyValue (src/ops/batch_get_asset_property_value.rs:386-389) and the refusal pinned in tests/all/batch_get_latest_values.rs:661-675: a token replayed against a different entry list names no page and is refused with InvalidRequestException.
6. The same sequence against POST /properties/batch/history (BatchGetAssetPropertyValueHistory) behaves identically, since it decodes the same BatchCursor (src/ops/batch_get_asset_property_value_history.rs:202-208).
