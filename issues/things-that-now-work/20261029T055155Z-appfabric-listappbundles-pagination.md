Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0ab5854f8833828180776b987700bd810a961abd

# decode_page_token validates only a token's prefix and hex form, so a never-issued position (0, or past the counter) is silently read as a page position instead of refused

_Filed: 2026-10-29T05:51:55Z_
_Operation: ListAppBundles | Class: pagination_

> _Note: this issue was identified against the code base at commit `0ab5854f8833828180776b987700bd810a961abd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `List*` handler in this crate decodes its `nextToken` through `crate::state::decode_page_token`, whose own doc says it answers "a `ValidationException` naming the member when the token is not one this listing issued", and the crate's pagination test states the same rule: "a `nextToken` the service did not issue is refused as a `ValidationException` (HTTP 400), never silently treated as the first page."

The implementation checks only two things: the `v1-<listing>-` prefix, and that the remainder parses as hex. It never checks that the decoded number is a position this listing ever handed out. Two concrete consequences:

* `nextToken=v1-appbundles-0000000000000000` decodes to `after = Some(0)`. `page()` then keeps every record (`sequence_of(record) > 0`), and because `next_sequence()` pre-increments, 0 is never any record's sequence — so a token the service never minted is answered HTTP 200 with the FIRST page, which is exactly the outcome the rule forbids ("never silently treated as the first page").
* `nextToken=v1-appbundles-ffffffffffffffff` decodes to a position past the store's counter, so the handler answers HTTP 200 with an empty list and no `nextToken` — silently ending a walk at a position the service never issued, instead of the modelled 400.

The gap is in the shared helper, so all four paginated operations behave this way (`ListAppBundles` with `APP_BUNDLE_LISTING`, and `ListIngestions` / `ListIngestionDestinations` / `ListAppAuthorizations` with their per-parent listing names); `ListAppBundles` is the handler whose own test states the rule.

## Evidence

The rule, from the helper's own doc — services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs:1520-1529:

    /// The position a `nextToken` names in `listing`, or a `ValidationException`
    /// naming the member when the token is not one this listing issued.
    pub fn decode_page_token(listing: &str, token: &str) -> Result<u64, ValidationException> {
        token
            .strip_prefix(&format!("v1-{listing}-"))
            .and_then(|sequence| u64::from_str_radix(sequence, 16).ok())
            .ok_or_else(|| {
                field_validation_failed("nextToken", "nextToken is not a valid pagination token")
            })
    }

— the only checks are the prefix and `from_str_radix`, so ANY 16-hex value is accepted as a position.

The same rule, stated by the crate's own test — tests/all/app_bundle_ingestion_lifecycle.rs:630-631:

    /// RULE: a `nextToken` the service did not issue is refused as a
    /// `ValidationException` (HTTP 400), never silently treated as the first page.

Its table (tests/all/app_bundle_ingestion_lifecycle.rs:638-650) covers a bad prefix, a bad listing and non-hex digits (`"v1-appbundles-zzzzzzzzzzzzzzzz"`, `"v1-ingestions-0000000000000001"`), but no row with THIS listing's prefix and a well-formed position the listing never issued — the case the code lets through.

What the accepted position then does — src/state.rs:1487-1491:

        let remaining: Vec<&R> = records
            .iter()
            .copied()
            .filter(|record| after.is_none_or(|after| sequence_of(record) > after))
            .collect();

with `after = Some(0)` every record passes, because sequence 0 is never assigned — src/state.rs:822-825:

        pub fn next_sequence(&mut self) -> u64 {
            self.next_sequence += 1;
            self.next_sequence
        }

Caller side (unchanged by this finding) — src/ops/list_app_bundles.rs:166-174 passes `crate::state::APP_BUNDLE_LISTING` and `input.next_token` straight into `crate::state::page`, mapping only its `ValidationException` out.

Test coverage: `a_token_this_service_did_not_issue_is_refused` and `a_token_from_another_listing_is_refused` (tests/all/app_bundle_ingestion_lifecycle.rs:632, 672) pin the refusal for malformed and cross-listing tokens; neither pins the well-formed-but-never-minted position, so nothing in the crate justifies accepting it.

## Steps to Reproduce

1. Boot the emulator and call `CreateAppBundle` once (POST /appbundles with {}) so the store holds one app bundle with sequence >= 1.
2. Call `GET /appbundles?nextToken=v1-appbundles-0000000000000000` — a token this listing never issued (sequence 0 is never assigned). Observed: HTTP 200 with the full first page, i.e. the forged token is read as a position. Expected per the helper's doc and the crate's own test rule: HTTP 400 ValidationException naming `nextToken`.
3. Call `GET /appbundles?nextToken=v1-appbundles-ffffffffffffffff`. Observed: HTTP 200 with an empty `appBundleSummaryList` and no `nextToken`, silently ending the walk. Expected: HTTP 400 ValidationException.
4. Repeat against the other listings through the same helper, e.g. `GET /appbundles/{bundleUuid}/ingestions?nextToken=v1-ingestions-<bundleUuid>-0000000000000000`, and observe the same 200.
