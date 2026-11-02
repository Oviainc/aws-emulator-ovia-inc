Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: e4f45eba1b7e0829bfedb4a437f2a9c9eaec6d3e

# The nextToken is a bare entry COUNT scoped only to the feature, so continuing a walk with different filters (or after an update) silently skips entries

_Filed: 2026-11-01T16:30:40Z_
_Operation: GetBillingPreferences | Class: pagination_

> _Note: this issue was identified against the code base at commit `e4f45eba1b7e0829bfedb4a437f2a9c9eaec6d3e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This read's cursor is a position (a count of entries already delivered) into the sequence `preference_entries(feature)` AFTER the request's `filters` have been applied. The token encodes only `feature:consumed`, and `decode_page_token` validates only the feature — the `filters` member that selects the very sequence the count indexes into is never bound to the token. Two consequences, both of which hand a caller the wrong page with HTTP 200:

1. A token minted for a filtered walk, replayed with different (or omitted) `filters`, is applied as an offset into a DIFFERENT, longer sequence: the entries at the front of the new sequence are never reported and the page carries entries the original filter excluded. The handler already knows a cursor must be scoped to what minted it — it refuses a token "issued for another feature" — but stops at the feature.

2. For a NON-history feature the entries are the stored keys in key order, i.e. unique values that can name a position, which is exactly the situation in which this crate's two other paginated reads resume AFTER A KEY and state the reason ("so a view inserted or removed between pages cannot make the walk skip or repeat another one"). GetBillingPreferences uses a count instead, and its stated justification for doing so covers only the history-feature case ("a history feature reports one entry per recorded change, so one key can appear many times"). So an UpdateBillingPreferences that adds a key sorting before the cursor between two pages makes the next page skip an entry the caller never sees.

## Evidence

The token carries only feature+count — services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs:157-167:
```
/// The opaque `nextToken` for a page of `feature` that ended after `consumed`
/// entries.
///
/// The cursor is a COUNT, not a key: a history feature reports one entry per
/// recorded change, so one key can appear many times and a key alone cannot name
/// a position. ...
fn encode_page_token(feature: &str, consumed: usize) -> String {
    hex::encode(format!("{feature}:{consumed}"))
}
```
and the only scoping check is the feature — get_billing_preferences.rs:192-197:
```
    if stored_feature != feature {
        return Err(invalid_field(
            "nextToken",
            "the pagination token was issued for another feature",
        ));
    }
```
while the count is applied to the FILTERED sequence — get_billing_preferences.rs:314-325:
```
        let matched: Vec<crate::state::PreferenceEntry> = state
            .preference_entries(feature)
            .into_iter()
            .filter(|entry| matches_filters(entry, &input))
            .collect();
        ...
        let page: Vec<crate::state::PreferenceEntry> =
            matched.iter().skip(consumed).take(limit).cloned().collect();
```
The rule this crate states for its other two paginated reads — src/ops/list_billing_views.rs:335-341:
```
        // The page resumes strictly after the last ARN the previous page
        // returned, so a view inserted or removed between pages cannot make the
        // walk skip or repeat another one.
        let resume_after = match input.next_token.as_deref() {
```
and src/ops/get_credit_allocation_history.rs:347-349: "The page resumes strictly after the entry the previous page ended on, so an allocation written or removed between pages cannot make the walk skip or repeat another one."

For a non-history feature the entries ARE unique keys in key order, so a key cursor is available — src/state.rs:787-790 (`preference_entries` doc): "For a history feature that is the recorded changes to the feature behind it, oldest first; for every other feature it is the stored keys in key order." (state.rs:806-816 walks `billing_preferences`, a `BTreeMap<(String, String), String>` keyed by (feature, key), so each key appears once.)

Existing coverage never varies the request between pages: tests/all/billing_preferences.rs:560-614 (`pagination_walks_every_entry_exactly_once`) re-sends the SAME body plus the token (`request = json!({ "features": [...], "maxResults": page_size })` at line 581, no `filters`, no interleaved update), and tests/all/billing_preferences.rs:695 only pins that a token this service never issued is refused. No test combines a token with a changed `filters` or with a write between pages.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it.
2. Call UpdateBillingPreferences(feature="BILLING_ALERTS", billingPreferencesPerKey=[{key:"key/0",value:"ENABLED"},{key:"key/1",value:"ENABLED"},{key:"key/2",value:"ENABLED"}]).
3. Call GetBillingPreferences(features=["BILLING_ALERTS"], filters=[{name:"PREFERENCE_KEY", value:["key/1","key/2"]}], maxResults=1) — observe `key/1` and a `nextToken` (hex of "BILLING_ALERTS:1").
4. Replay that token with the `filters` member omitted: GetBillingPreferences(features=["BILLING_ALERTS"], maxResults=1, nextToken=<token>). Observe 200 with `key/1` again and NO report of `key/0`: the count is applied as an offset into the unfiltered 3-entry sequence, so the first entry of the new sequence is skipped instead of the token being refused as it is for another feature (get_billing_preferences.rs:192-197).
5. For symptom 2, restart, set two keys `key/1`,`key/2`, read page 1 with maxResults=1 (returns `key/1`, token "…:1"), then call UpdateBillingPreferences adding `key/0`, then request page 2 with the token: the walk resumes at offset 1 of [key/0,key/1,key/2] and reports `key/1` again while `key/0` is never reported — the skip/repeat the crate's other two reads state their key cursors prevent.
