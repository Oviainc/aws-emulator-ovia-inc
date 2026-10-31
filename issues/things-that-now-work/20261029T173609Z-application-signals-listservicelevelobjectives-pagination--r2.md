Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 910698ab7b22be6d08fbb15c4abd76821047ddd2

# The SLO listing honours ANY cursor payload inside a tag it issued, so an edited token re-serves the first page or reports the listing exhausted

_Filed: 2026-10-29T17:36:09Z_
_Operation: ListServiceLevelObjectives | Class: pagination_

> _Note: this issue was identified against the code base at commit `910698ab7b22be6d08fbb15c4abd76821047ddd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListServiceLevelObjectives` pages the stored objectives with a cursor that is a store KEY, resolved by `ComAmazonawsApplicationsignalsApplicationsignalsState::page`. That function takes the decoded payload and uses it DIRECTLY as the range's exclusive lower bound, with no check that the payload is a boundary this service's own paging could have handed out:

```rust
let lower = match cursor {
    Some(cursor) => Bound::Excluded(cursor),
    None => Bound::Unbounded,
};
```

Every OTHER read of this crate validates the payload inside the tag, and the crate states that as a rule: the offset pager refuses a position it could not have minted (`issued_offset`), and the chronological status cursor refuses any field not spelled the way its encoder spells it (`StatusEventCursor::decode`). A `NextToken` of this service is hex of `tag + payload` (`encode_cursor`), i.e. opaque but NOT authenticated, so a caller holding one genuinely issued token can hex-decode it, replace the payload, re-encode, and have the SLO listing honour a boundary the service never issued:

* payload `""` (the bare tag) -> `Bound::Excluded("")` admits every stored name, so the FIRST page is served again under a cursor, together with a fresh `NextToken`;
* a payload sorting after every stored name -> an empty `SloSummaries` and no `NextToken`, i.e. the listing is reported exhausted from a cursor the service never gave out;
* a payload naming a stored objective that was not a page boundary -> the objectives sorting before it are silently skipped.

This is the payload half of the cursor rule only; the already-fixed `ListServiceLevelObjectives` pagination issue bound the SELECTION into the token's TAG (`slo_selection_token_tag`), which is a different half and does not constrain what the payload inside that tag may say. The same shape exists in `page_instrumentation_configurations` (state.rs:4687-4690), the crate's other key cursor, which is mentioned only as context here.

## Evidence

The unvalidated key cursor — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:508-521:
```rust
    pub fn page<F>(
        &self,
        token_tag: &str,
        cursor: Option<&str>,
        page_size: usize,
        matches: F,
    ) -> (Vec<ServiceLevelObjectiveSummary>, Option<String>)
    where
        F: Fn(&ServiceLevelObjective) -> bool,
    {
        let lower = match cursor {
            Some(cursor) => Bound::Excluded(cursor),
            None => Bound::Unbounded,
        };
```
and its doc states only the TAG half of the rule (state.rs:500-502): "The next page's token is issued under `token_tag`, the tag of the very selection `matches` picks out ([`slo_selection_token_tag`]), so the key it carries is resolved only against the ordered set it was taken from."

The rule the crate holds every other cursor to — state.rs:1565-1568 (`page_of` doc): "The token is opaque but not authenticated, so the POSITION inside the tag is a value a caller may have written: it is honoured only when it is an offset this paging can have minted over the projection being read ([`issued_offset`]), and refused with that same ONE wording otherwise." Enforced at state.rs:1545-1550:
```rust
fn issued_offset(cursor: &str, len: usize) -> Option<usize> {
    let offset = canonical_number::<usize>(cursor)?;
    (MIN_PAGE_BOUND as usize..len)
        .contains(&offset)
        .then_some(offset)
}
```
and at state.rs:4257-4263 (`StatusEventCursor::decode` doc): "A `NextToken` is opaque but not authenticated, so every field it carries is a value a caller may have written: ... A payload carrying anything else is not a cursor this service issued, and is refused exactly like a token that is not hex."

What honouring such a payload does, in the crate's own words (state.rs:1542-1544): "honouring it would re-serve the first page under a cursor, or report the projection as exhausted to a caller whose cursor is not one it was given."

The token is caller-editable hex — state.rs:1405-1413:
```rust
pub fn encode_cursor(tag: &str, cursor: &str) -> String {
    ::hex::encode(format!("{tag}{cursor}"))
}
...
pub fn decode_cursor(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(tag).map(ToString::to_string)
}
```
and the handler feeds the decoded payload straight to `page` — src/ops/list_service_level_objectives.rs:203-229.

Test coverage: tests/all/issued_page_position.rs:3-11 states the rule as a property of the FAILURE — "The POSITION a page cursor carries is honoured only when it is one this service issued. ... a read resolves it only when it is a boundary this service's own paging could have handed out over the set being read: spelled exactly as the service spells it, and naming a position that paging can reach. Anything else is refused with the modelled `ValidationException` (HTTP 400)" — but its rows cover only "all eight whose cursor is an offset" plus the status read; the key-cursor read is never mentioned there, and tests/all/unissued_next_token.rs:17-19 covers `ListServiceLevelObjectives` only for the TAG half ("it is empty, it is not hex, ... and it is a token this service really DID issue, for another read"). So nothing pins the payload half for this read.

## Steps to Reproduce

1. Create three SLOs whose names sort as `pos-a`, `pos-b`, `pos-c` (POST /slo, as tests/all/slo_lifecycle.rs::paging_terminates_and_visits_every_slo_exactly_once seeds them).
2. POST /slos?MaxResults=1 -> 200 with `SloSummaries` = [pos-a] and a `NextToken` T.
3. Hex-decode T: it reads `slos/1/<selection hash>/pos-a`. Re-encode just `slos/1/<selection hash>/` (empty payload) as hex -> T'.
4. POST /slos?MaxResults=1&NextToken=T' -> 200 with `SloSummaries` = [pos-a] again plus a fresh `NextToken`: the first page is re-served under a cursor. Expected, by the rule state.rs:1565-1568 and tests/all/issued_page_position.rs:3-11 state for every other read, is HTTP 400 `ValidationException` with the message `NextToken is not a token this service issued`.
5. Re-encode `slos/1/<selection hash>/zzzz` (a payload sorting after every stored name) as hex and resend -> 200 with an empty `SloSummaries` and no `NextToken`: the listing is reported exhausted from a boundary the service never issued.
6. For contrast, perform the same payload edit on a `ListServices` token: the offset pager answers HTTP 400 `NextToken is not a token this service issued` (tests/all/issued_page_position.rs).
