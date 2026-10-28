Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# A UUID-shaped nextToken this service never issued silently truncates (or empties) the listing instead of being refused

_Filed: 2026-10-28T21:53:03Z_
_Operation: ListSuiteDefinitions | Class: pagination_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both listings in this crate validate `nextToken` only against the SHAPE of a UUID, never against the ids they actually handed out, and then use the token as an inclusive lower bound on the collection's key order. The accepted-token set is therefore much wider than the issued-token set: any hand-written lowercase-hex `8-4-4-4-12` string (e.g. `ffffffff-ffff-ffff-ffff-ffffffffffff`) is accepted, and the listing answers HTTP 200 with the records whose id sorts at or above that invented value — silently omitting every record below it, and an EMPTY list with no `nextToken` when the invented value sorts above every id. A client cannot distinguish that from "the collection is empty / this is the last page", so a forged, stale, hand-edited or cross-listing token (a suite-definition id handed to `/suiteRuns`, or a suite-run id handed to `/suiteDefinitions` — both are UUIDs this service mints) silently loses records instead of producing the refusal the crate says it produces.

The crate states the opposite rule in `state.rs`: the shape test exists precisely so that a listing can "refuse a garbled or hand-written token with the modelled client-input error", and each handler's own comment asserts the token "is the suite-run id / suite-definition id the next page resumes AT". Nothing in either handler checks that claim against `store.definitions` / `store.runs`, although the handler holds the collection at that point. `ListSuiteRuns` has the identical defect at the same lines of its own copy of the rule (the rule's single helper is shared, the applying branch is duplicated).

## Evidence

The crate's stated rule — services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/state.rs:728-734:
```
/// True when `token` could be a pagination token one of this service's listings issued.
///
/// Every listing here resumes AT a resource id, so the token it hands out is one of the
/// ids this service mints — a hyphenated lowercase v4 UUID (see [`new_uuid`]). A token of
/// any other shape is one this service cannot have issued, which is what lets a listing
/// refuse a garbled or hand-written token with the modelled client-input error instead of
/// silently answering from the start of the map.
```
What the code actually accepts — state.rs:739-748 (shape only; no lookup of the id in any collection):
```
pub fn is_page_token_shape(token: &str) -> bool {
    fn is_lower_hex(byte: u8) -> bool {
        byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
    }
    let groups: Vec<&str> = token.split('-').collect();
    groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| group.bytes().all(is_lower_hex))
}
```
The handler that trusts it — src/ops/list_suite_definitions.rs:145-152 ("A token is a position this service handed out, never arbitrary caller text: it is the suite-definition id the next page resumes AT, so one that cannot be an id this service mints is refused") and :176-179, where the unverified value becomes the page's lower bound:
```
        let mut remaining = match resume_at {
            Some(token) => store.definitions.range(token..).peekable(),
            None => store.definitions.range(String::new()..).peekable(),
        };
```
The same two sides in the sibling listing — src/ops/list_suite_runs.rs:174-181 ("it is the suite-run id the next page resumes AT, so one that cannot be an id this service mints is refused") and :225-232:
```
        let mut remaining = matching
            .iter()
            .filter(|run| {
                resume_at
                    .as_deref()
                    .is_none_or(|from| run.id.as_str() >= from)
            })
```
Test coverage: tests/all/suite_runs_listing_and_update.rs:430-486 `a_next_token_this_service_never_issued_is_refused_by_every_listing` claims "A `nextToken` this service cannot have issued is refused with the modelled `ValidationException`, by EVERY listing", but its `malformed` table (:453-462) contains only non-hex, UPPER-case (`...5555555555AA`), truncated, over-grouped and unhyphenated values — every entry fails the shape test. No case exercises a correctly shaped lowercase-hex UUID that no listing issued, nor a token one listing issued passed to the other, so the behaviour above is untested rather than pinned.

## Steps to Reproduce

1. Start the emulator and create three suite definitions with CreateSuiteDefinition (`POST /suiteDefinitions`).
2. Call `GET /suiteDefinitions` and confirm all three are listed.
3. Call `GET /suiteDefinitions?nextToken=ffffffff-ffff-ffff-ffff-ffffffffffff` — a token no listing ever issued, but one that satisfies `is_page_token_shape`. Observed: HTTP 200 with an empty `suiteDefinitionInformationList` and no `nextToken`, indistinguishable from an empty account; expected by the crate's own rule (state.rs:728-734): the modelled `ValidationException`.
4. Call `GET /suiteDefinitions?nextToken=<the middle definition's id>` without having been handed that token: the page starts mid-collection and the definitions sorting below it are silently absent.
5. Repeat with `GET /suiteRuns?nextToken=<a suiteDefinitionId from step 1>` after seeding runs: the definition id is accepted as a run cursor (same UUID shape) and every run whose id sorts below it is silently dropped from the listing.
