Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 4ae98698bb1e1b3108e7a354625f282bc1be6414

# decode_page_token accepts cursor offsets encode_page_token never writes (leading-zero/'+' spellings, and positions past a sequence that only grows), so a token this listing never issued is served a page

_Filed: 2026-10-29T07:13:18Z_
_Operation: ListAccounts | Class: pagination_

> _Note: this issue was identified against the code base at commit `4ae98698bb1e1b3108e7a354625f282bc1be6414`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`decode_page_token` is documented as the inverse of `encode_page_token` — "`None` when the token was not produced by this service" — and every paginated handler leans on exactly that: a token that does not decode is refused with "Member must be a token returned by a previous <Operation> call", and a token that decodes is accepted on nothing but the fingerprint comparison.

But the offset half of the cursor is parsed with a bare `offset.parse().ok()?`, which accepts spellings `encode_page_token` can never have produced, because it renders the offset through `usize`'s `Display`:

* a non-canonical decimal — `"01:<digest>"`, `"007:<digest>"`, `"+1:<digest>"` (Rust's `usize::from_str` accepts a leading `+` and leading zeros). Each decodes to a cursor and resumes the walk, although the hex of such a string was never minted by this service.
* a position past the end of the sequence. For `ListAccounts` this is provably un-issued rather than a stale cursor over a shrunk sequence: the enrollment store is only ever written through `enrollments.entry(..).or_default()` and `enrollments.get_mut(..)` (state.rs:3258, 3309, update_enrollment_configuration.rs:253) and nothing removes an entry, so `accounts_in_order` only grows and `serve_page` only ever mints `offset + page.len()` while a further item exists. Such a token is answered HTTP 200 with `"accounts": []` and no `nextToken` — which a client reads as "the account list ended here" — instead of the refusal the handler gives every other token it did not issue.

So the handler's single stated refusal rule ("A token this listing never issued ... one refusal, worded once") is enforced only for the subset of un-issued tokens that fail to decode; a token whose offset is spelled in a way this service never writes is served. The defect is in the shared helper, so the same input behaves the same way on all seven listings that call it; `ListAccounts` is named because it is the first paginated operation on the checklist that calls it and the one whose sequence provably never shrinks.

## Evidence

The only offset spelling the service ever mints — `src/state.rs:3602-3608`:
```
/// Encode a cursor as the opaque `nextToken` every paginated operation of this
/// service describes (`^[A-Za-z0-9+/=]+$`). Hex of `<offset>:<fingerprint>` satisfies that
/// pattern and carries the cursor itself, so a page token needs no server-side
/// entry and a read never writes.
pub fn encode_page_token(cursor: &PageCursor) -> String {
    hex::encode(format!("{}:{}", cursor.offset, cursor.filter_fingerprint))
}
```
(`cursor.offset` is a `usize` — `src/state.rs:3598` — so `Display` renders exactly one, canonical, decimal form.)

The decoder's own stated rule, and the parse that breaks it — `src/state.rs:3610-3621`:
```
/// Decode a `nextToken` produced by [`encode_page_token`]. `None` when the token
/// was not produced by this service. What a decoded cursor may resume is the
/// caller's own check: it compares the cursor's fingerprint against the one it
/// would have issued for this request.
pub fn decode_page_token(token: &str) -> Option<PageCursor> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (offset, fingerprint) = decoded.split_once(':')?;
    Some(PageCursor {
        offset: offset.parse().ok()?,
        filter_fingerprint: fingerprint.to_string(),
    })
}
```
`offset.parse()` is the ONLY thing standing between a hand-built offset and a served page, and it accepts `"01"`, `"007"` and `"+1"` — none of which `format!("{}", usize)` can emit.

The handler that takes the decoded cursor at its word — `src/ops/list_accounts.rs:179-188`:
```
        let offset = match input.next_token.as_deref() {
            None => 0,
            Some(token) => match crate::state::decode_page_token(token) {
                // A token this listing never issued, whether it decodes to no
                // cursor at all or to one taken for another walk: one refusal,
                // worded once.
                Some(cursor) if cursor.filter_fingerprint == query_fingerprint => cursor.offset,
                _ => return Err(foreign_token()),
            },
        };
```
and the refusal it skips — `src/ops/list_accounts.rs:228-238`:
```
/// The modelled 400 (`InvalidParameterValueException`), for a `nextToken` this
/// operation did not issue. The model's `NextToken` pattern cannot express "a
/// token of mine", so the check is in the handler rather than at the boundary.
fn foreign_token() -> ListAccountsError {
    ListAccountsErrorModeled::InvalidParameterValueException(InvalidParameterValueException {
        message: Some(crate::state::constraint_report(
            "nextToken",
            "Member must be a token returned by a previous ListAccounts call",
        )),
```

That an un-parseable offset IS refused — and that no case covers an offset spelled a way the service never writes — `tests/all/page_token_refusals.rs:194-210`:
```
const UNISSUED_TOKENS: &[(&str, &str, bool)] = &[
    ("plain-words", "notatokenweissued", true),
...
    // Hex of "abc:def": a separator, but no offset in front of it.
    ("no-offset", "6162633a646566", true),
```
asserted refused for every paginated operation at `tests/all/page_token_refusals.rs:216-240`. The table has no `"01:<digest>"`, `"+1:<digest>"` or out-of-range-offset row, and `tests/all/page_token_refusals.rs:249-266` only varies the FINGERPRINT (`"nofingerprintofanyqueryofthisservice"`), so the offset half of the cursor is unpinned by the suite.

The sequence `ListAccounts` pages never shrinks, so a past-the-end offset cannot be a stale issued cursor — `src/state.rs:3070-3078` builds the page from `self.enrollments`, and the only writers are `src/state.rs:3258` (`self.enrollments.entry(account_id.to_string()).or_default()`), `src/state.rs:3309` (`self.enrollments.get_mut(account_id)`) and `src/ops/update_enrollment_configuration.rs:253` (`guard.enrollments.entry(account_id.clone()).or_default()`); no path removes an entry.

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"} so the caller is past the enrollment gate.
2. Call AssociateAccounts with {"accountIds": ["111111111111", "222222222222"]} so the enrollment store holds three accounts (the caller's plus the two members).
3. Call ListAccounts with {"maxResults": 1} — 200, one account and a nextToken T. Hex-decode T: it reads "1:<digest>" (src/state.rs:3607).
4. Re-encode the SAME cursor with the offset spelled non-canonically — hex of "01:<digest>" (or "+1:<digest>") — and call ListAccounts with {"maxResults": 1, "nextToken": <that hex>}. Observe 200 and the second page: `offset.parse()` (src/state.rs:3618) accepted a spelling format!("{}", usize) can never emit, so a token this service never minted resumed the walk. Expected the refusal hex("abc:def") gets — 400 InvalidParameterValueException, "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must be a token returned by a previous ListAccounts call" (tests/all/page_token_refusals.rs:202-203, 216-240).
5. Re-encode the cursor with an offset past the end — hex of "99:<digest>" — and call ListAccounts with {"nextToken": <that hex>}. Observe 200 with "accounts": [] and no nextToken, i.e. an authoritative-looking end of the account list, although no ListAccounts page ever minted offset 99 and the enrollment store only grows (src/state.rs:3258, 3309; src/ops/update_enrollment_configuration.rs:253 are its only writers and none removes an entry).
6. Repeat step 4 against any other paginated listing (ListAutomationRules, ListRecommendedActions, ListAutomationEventSteps, …) with that listing's own first-page token: the same non-canonical offset is accepted, because the defect is in the shared crate::state::decode_page_token.
