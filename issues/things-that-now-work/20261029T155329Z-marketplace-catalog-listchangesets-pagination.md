Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: cb9254e7b4a839ef1fe220636b4ce55207805929

# decode_page_token honours NextToken hex spellings encode_page_token never mints (uppercase/mixed-case, a leading '+'), so a client-fabricated token is served as a cursor

_Filed: 2026-10-29T15:53:29Z_
_Operation: ListChangeSets | Class: pagination_

> _Note: this issue was identified against the code base at commit `cb9254e7b4a839ef1fe220636b4ce55207805929`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this crate resolves its `NextToken` through the one shared decision `crate::state::page_offset` -> `crate::state::decode_page_token`. The crate states that decision as: a token this service issued resumes where it points, and ANYTHING else is the single `invalid_page_token` fault. The code does not implement that rule.

`encode_page_token` mints exactly one spelling per offset: `format!("{prefix}-{offset:x}")` — lowercase hex, no sign. `decode_page_token` accepts the offset with `usize::from_str_radix(rest, 16)`, which additionally honours uppercase and mixed-case hex digits and an explicit leading `+`. So `CS-F`, `CS-1A`, `CS-+1` and `ENT-A` are tokens this service never issued, yet they are decoded as offsets 15, 26, 1 and 10 and served as a cursor: the call answers 200 with the page starting at that offset (silently skipping the earlier items) instead of the modelled `ValidationException` the crate says a token naming no page gets.

The same looseness applies to all four paginated handlers that call the helper — `ListChangeSets` (prefix `CS`), `ListEntities` (`ENT`), `ListAssessments` (`ASMT`) and `DescribeAssessment` (`CTRL`) — because the rule is implemented once; `ListChangeSets` is named here as the handler whose repro needs no setup at all.

## Evidence

The minting side writes ONE spelling (lowercase hex, unsigned) — services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:2970-2974:
```
/// The page `offset` as a `NextToken`: `^[\w+=.:@\-\/]+$`, with a prefix naming
/// the list it pages, so a token one list returned is not accepted by another.
pub fn encode_page_token(prefix: &str, offset: usize) -> String {
    format!("{prefix}-{offset:x}")
}
```
The decoding side promises to accept only those, and then accepts more — state.rs:2976-2981:
```
/// The offset a `NextToken` this service issued for `prefix` resumes at, or
/// `None` for a token it never issued.
pub fn decode_page_token(prefix: &str, token: &str) -> Option<usize> {
    let rest = token.strip_prefix(prefix)?.strip_prefix('-')?;
    usize::from_str_radix(rest, 16).ok()
}
```
`usize::from_str_radix("F", 16)` == `Ok(15)` and `usize::from_str_radix("+1", 16)` == `Ok(1)`, while `format!("{15:x}")` is `"f"` — so those spellings are tokens the service never issued.

The rule the crate states for exactly this case — state.rs:1494-1508:
```
/// The offset a `NextToken` asks the list paging with `prefix` to resume at, or
/// the modelled `ValidationException` that refuses it.
...
/// anything else — a token outside the modelled `NextToken` syntax, a well-formed
/// token naming no page, or a token another list of this service issued — names
/// no page of this list and is the one [`invalid_page_token`] fault.
```
And the handler that calls it repeats the same rule — src/ops/list_change_sets.rs:240-253:
```
        // The token is the offset the next page resumes at; one this service
        // never issued names no page, and neither does one outside the modelled
        // `NextToken` syntax (`skip_validation` on `ListChangeSetsInput`), so
        // both are the one page-token fault — the decision
        // `crate::state::page_offset` makes for every paginated operation.
        let offset = match crate::state::page_offset(
            crate::state::CHANGE_SET_PAGE_PREFIX,
            input.next_token.as_deref(),
        ) {
```
(the identical comment and call appear in src/ops/list_entities.rs:338-349, src/ops/list_assessments.rs:187-200 and src/ops/describe_assessment.rs:216-230).

Test coverage leaves the gap unpinned: tests/all/page_token_rule.rs:250-291 enumerates "the ways a `NextToken` can name no page" as `not a valid token !!`, no prefix, prefix without separator, wrong separator, `{prefix}-zz`, empty offset, `{prefix}-0x10`, 40 `f`s and another list's token — no uppercase/mixed-case hex row and no signed row — and tests/all/page_token_rule.rs:480-517 ("the_page_token_rule_judges_every_list_by_its_own_prefix") asserts only over that list plus the tokens `encode_page_token` itself mints (offsets 15 and 16 are asserted as the lowercase `f` / `10` spellings).

## Steps to Reproduce

1. Start the emulator with this service mounted (as tests/all/page_token_rule.rs does).
2. POST /ListChangeSets with body {"Catalog": "AWSMarketplace", "NextToken": "CS-F"}.
3. Observe 200 with a page resolved at offset 15 (an empty page on a small store), instead of the modelled 422 ValidationException with Message "[Requested page token is invalid.]" that `CS-zz` or `CS-0x10` gets.
4. Repeat with "CS-+1": it is decoded as offset 1, so the first change set is silently skipped, although `encode_page_token` can never mint a token carrying a '+'.
5. Same for POST /ListEntities with NextToken "ENT-A", POST /ListAssessments with "ASMT-A" and POST /DescribeAssessment with "CTRL-A".
