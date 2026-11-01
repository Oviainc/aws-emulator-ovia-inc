Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 3cf6370e40ed3f55db7810d0fd4f4a5278e686bc

# A nextToken this service never issued (any hex that decodes to text, including another read's token) is honoured as a cut point instead of refused

_Filed: 2026-11-01T23:16:57Z_
_Operation: GetCreditAllocationHistory | Class: pagination_

> _Note: this issue was identified against the code base at commit `3cf6370e40ed3f55db7810d0fd4f4a5278e686bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetCreditAllocationHistory` resumes a page through `crate::state::decode_page_token`, whose documented contract is to answer `None` "when the token is not one `encode_page_token` produced". The implementation cannot keep that promise: it hex-decodes and UTF-8-decodes the token and returns whatever string falls out, with no check that the result is a key this service ever minted. `resume_key` only raises the modelled `ValidationException` ("nextToken is not a token this service issued") when that returns `None`, i.e. only for a token that is not even valid hex.

So any hex string that decodes to text is accepted as the exclusive upper bound of the key range the walk seeks, and the request answers 200 with a WRONG page rather than refusing:

* `nextToken = hex("0") = "30"` decodes to "0", which sorts below every `credit_allocations` key (a key starts with the `YYYY-MM` month), so `lower < upper` is false and the handler answers an empty `creditAllocationHistoryList`, `partialResults: false`, no `nextToken` — a caller cannot tell that from "this account has no allocation history".
* A token minted by a DIFFERENT operation is accepted too: `ListBillingViews` mints `state::encode_page_token(<view arn>)` with the same codec, and hex of `arn:aws:billing::…` decodes to a string above every month key, so the `Some(after) if after < end_upper` guard falls through and the handler re-serves the FIRST page — the caller silently receives the same entries twice. The reverse also holds: a `GetCreditAllocationHistory` token fed to `ListBillingViews` skips nothing and re-serves its first page.

The crate's other two paginated reads validate the decoded token's shape and refuse exactly this class of input, and the crate states that as the rule.

## Evidence

The contract the codec states, and the body that cannot honour it — services/service-com-amazonaws-billing-awsbilling/src/state.rs:1152-1162:
```
/// The value a `nextToken` resumes after, or `None` when the token is not one
/// [`encode_page_token`] produced.
///
/// The caller renders the `None` as its own modeled `ValidationException`, from
/// [`PAGE_TOKEN_REFUSAL`] and [`PAGE_TOKEN_FIELD_REFUSAL`] so the refusal reads
/// the same whichever read answered it.
pub fn decode_page_token(token: &str) -> Option<String> {
    ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}
```
(`encode_page_token`, state.rs:1148-1150, is `::hex::encode(last.as_bytes())` over a key the store is keyed by — so "not one encode_page_token produced" is a set `decode_page_token` never tests membership of.)

The handler, which refuses only on `None` — services/service-com-amazonaws-billing-awsbilling/src/ops/get_credit_allocation_history.rs:199-211:
```
fn resume_key(token: &str) -> Result<String, GetCreditAllocationHistoryError> {
    crate::state::decode_page_token(token).ok_or_else(|| {
        GetCreditAllocationHistoryErrorModeled::ValidationException(ValidationException {
            message: crate::state::PAGE_TOKEN_REFUSAL.to_string(),
```
and the arbitrary string being used as the range bound — get_credit_allocation_history.rs:373-387:
```
        let upper = match resume_after {
            Some(after) if after < end_upper => after,
            _ => end_upper,
        };
        let mut page: Vec<(&String, &crate::state::CreditAllocation)> = Vec::new();
        if lower < upper {
            for (key, allocation) in state.credit_allocations.range(lower..upper).rev() {
```

The rule the crate states for a token, in the sibling read's own test — services/service-com-amazonaws-billing-awsbilling/tests/all/billing_preferences.rs:712-718:
```
    // A token is accepted only for the request that issued it; anything else is
    // a bad request, not an empty page.
    let rows: Vec<(&str, &str, &str, u16)> = vec![
        ("the token this read issued", "RI_SHARING", &issued, 200),
        ("that token on another feature", "CREDIT_SHARING", &issued, 400),
        ("text that is not a token", "RI_SHARING", "not-a-token", 400),
        ("hex that decodes to nonsense", "RI_SHARING", "6e6f6e73656e7365", 400),
```
Sibling 1 that enforces it — services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs:171-197 (`split_once(':')` plus `if stored_feature != feature { return Err(invalid_field("nextToken", "the pagination token was issued for another feature")) }`).
Sibling 2 that enforces it — services/service-com-amazonaws-billing-awsbilling/src/ops/list_source_views_for_billing_view.rs:157-172:
```
fn decode_page_token(token: &str) -> Result<usize, ListSourceViewsForBillingViewError> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| text.parse::<usize>().ok())
        .ok_or_else(|| { … ValidationException { message: crate::state::PAGE_TOKEN_REFUSAL …
```
and its test pinning the hex-that-decodes-to-text case — services/service-com-amazonaws-billing-awsbilling/tests/all/source_views.rs:292-295:
```
            "a nextToken that decodes to text, not a position",
            json!({ "arn": arn, "nextToken": hex::encode("abc") }),
```

Test coverage of the two readers that share `state::decode_page_token` stops at non-hex input, so the gap is untested: services/service-com-amazonaws-billing-awsbilling/tests/all/credit_allocation_history.rs:770-779 (`"a nextToken this service never issued"` → `"not-a-token-this-service-issued"`) and tests/all/billing_views.rs:918-921 (`"nextToken this service never issued"` → `json!({ "nextToken": "zzzz" })`) — both fail `hex::decode`, so both take the refusal path that does work.

## Steps to Reproduce

1. Boot the emulator with a shared engine and seed a few credit allocations in one billing month through `state::record_credit_allocation`, as tests/all/credit_allocation_history.rs does.
2. Call GetCreditAllocationHistory(accountId=<account>, startDate=2025-01-01, endDate=2026-01-01, maxResults=1) and confirm a 200 with a `nextToken`.
3. Repeat the same call with nextToken="30" (hex of the single character "0"), a token this service never minted: observe 200 with an empty `creditAllocationHistoryList` and no `nextToken`, instead of the 400 ValidationException "nextToken is not a token this service issued".
4. Repeat with the `nextToken` a ListBillingViews page returned (hex of a billing-view ARN): observe 200 serving the FIRST page again, so a paging client sees the same entries twice.
5. Contrast the siblings on the same emulator: ListSourceViewsForBillingView with nextToken=hex("abc") answers 400 (pinned by tests/all/source_views.rs:292-295) and GetBillingPreferences with nextToken="6e6f6e73656e7365" answers 400 (tests/all/billing_preferences.rs:718).
