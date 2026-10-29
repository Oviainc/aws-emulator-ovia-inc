Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: fc350f637d730e9c7a46ff0b276c9700aec9957e

# The shared ListCursor accepts a nextToken at exactly the end of a listing that its own rule says it issues — and never issues it, while ListProfileTimes refuses the same cursor

_Filed: 2026-10-29T18:23:49Z_
_Operation: GetFindingsReportAccountSummary | Class: pagination_

> _Note: this issue was identified against the code base at commit `fc350f637d730e9c7a46ff0b276c9700aec9957e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::ListCursor` is this crate's single statement of the pagination rule (`ListProfilingGroups`, `ListFindingsReports`, `GetFindingsReportAccountSummary` all page through it). Its doc states that a cursor index at exactly the end of the listing is a token the service DOES issue, and `page_of` therefore admits it (`if self.start > items.len()` — strictly greater) and answers a final empty page.

The code under that doc never issues such a token: `page_of` emits `next_token` only when `end < items.len()`, so a cursor equal to the listing length is never handed to any caller. The stated justification for admitting it ("the page before it ended at the last element") describes a token this helper cannot produce.

The crate's fourth listing, `ListProfileTimes`, spells the same rule out inline and reaches the opposite conclusion for the same input, with a reason that IS true of its code: a cursor at or past the end is one the service never issued, so it is refused with `ValidationException`. The result is that one cursor position gets two different answers from the same service depending on which listing is paged — 400 from `ListProfileTimes`, 200 with an empty page from the three `ListCursor` listings — and the two sides' stated reasons contradict each other, with the shared helper's reason being the false one. The test on the `ListCursor` side even asserts the false reason in its label.

This is the pagination rule stated twice with the copies disagreeing; a maintainer reading `ListCursor`'s doc will believe the service issues end-of-list tokens, which it does not.

## Evidence

The shared rule's claim — services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/state.rs:2309-2313:
```
/// (len 1..64, `^[\w-]+$`) for a listing of any length. Anything else — a token that
/// does not read as an index, or one past the end of the listing — is a token this
/// service never issued and is refused with [`UNISSUED_TOKEN_MESSAGE`]; an index at
/// exactly the end is one it DOES issue (the page before it ended at the last element),
/// so it answers a final empty page.
```

The code under it, which never issues that index — src/state.rs:2349-2359:
```
        if self.start > items.len() {
            return Err(UNISSUED_TOKEN_MESSAGE.to_string());
        }
...
        Ok(ListPage {
            items: items.get(self.start..end).unwrap_or_default(),
            next_token: (end < items.len()).then(|| end.to_string()),
        })
```
`next_token` is `Some` only while `end < items.len()`, so `end == items.len()` is never handed back — the index the doc says "it DOES issue".

The sibling listing, same crate, opposite answer for the same cursor — src/ops/list_profile_times.rs:267-278:
```
        // A cursor is only ever issued while entries remain after it, so a token at or
        // past the end of the list is one this service never issued: it is refused
        // rather than answered with an empty page. A request with NO token starts at 0,
        // which is the whole list however short it is — including empty.
        if input.next_token.is_some() && first >= starts.len() {
            return Err(ListProfileTimesErrorModeled::ValidationException(
                ValidationException {
                    message: "nextToken is not a token this service issued".to_string(),
```
and its own token emission, which agrees with that reason — src/ops/list_profile_times.rs:297: `next_token: (last < starts.len()).then(|| last.to_string()),`.

The handler that reaches the shared helper — src/ops/get_findings_report_account_summary.rs:198-204:
```
        let page = cursor
            .page_of(&summaries, input.max_results)
```

Test coverage, asserting both sides (and the false reason) — tests/all/findings_report_account_summary.rs:590-595:
```
        (
            "a nextToken at exactly the end of the summary is one this service issued",
            "?maxResults=1&nextToken=5",
            200,
            "",
        ),
```
versus tests/all/profile_times.rs:754-759:
```
        (
            // The group holds two five-minute windows, so a cursor of 2 points exactly
            // at the end — a position this service never issues a token for.
            "a nextToken exactly at the end of the list",
            format!("?{range}&period=PT5M&nextToken=2"),
        ),
```
(the latter row is in the table asserted to answer 400 `ValidationException`, tests/all/profile_times.rs:770-785).

## Steps to Reproduce

1. Read src/state.rs:2309-2313 — the shared pagination rule says a cursor index at exactly the end of the listing is one the service DOES issue, so it answers a final empty page.
2. Read src/state.rs:2355-2358 — `next_token` is produced only when `end < items.len()`, so an index equal to the listing length is never issued to any caller; the doc's justification describes a token the helper cannot produce.
3. Read src/ops/list_profile_times.rs:267-278 and :297 — the same rule written inline concludes the opposite (a cursor at the end is refused), and its reason matches its code.
4. Observe the divergence on the wire: with a stored listing of N elements, GET /internal/findingsReports?maxResults=1&nextToken=N answers 200 with an empty page (tests/all/findings_report_account_summary.rs:590-595) while GET /profilingGroups/<g>/profileTimes?...&nextToken=N with N profile times answers 400 ValidationException (tests/all/profile_times.rs:754-759 in the 400 table at :770-785).
5. Decide the rule once in `crate::state` and make both the helper's doc and its behaviour agree with the token it actually issues, so the two listings answer one cursor position one way.
