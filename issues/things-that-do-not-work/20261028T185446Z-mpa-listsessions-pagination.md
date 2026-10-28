Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 293c15726352cf981422bfe1d7cee5f6a5389f12

# A NextToken ListSessions issued is refused as "not a token this service issued" once the session it names stops satisfying the request's Filters

_Filed: 2026-10-28T18:54:46Z_
_Operation: ListSessions | Class: pagination_

> _Note: this issue was identified against the code base at commit `293c15726352cf981422bfe1d7cee5f6a5389f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListSessions` compiles `Filters` into predicates, applies them to the team's sessions, and only THEN cuts the page with the position-token rule `crate::state::page_after_seq` / `page_after_position`. That rule decides whether the token was ever handed out by looking for the token's position among the records it is paging over — which here are the FILTERED sessions. So the provenance test is made against a set that the request's own filter narrows and that the service's own writes keep changing.

The consequence: a token this very listing handed out is refused with HTTP 400 `ValidationException` and the message `NextToken is not a token this service issued.` as soon as the session that token names stops satisfying the filter. Nothing about the token is wrong, the session still exists, and `ListSessions` without the filter still reports it at that position — but the client that paged exactly as the handler's own doc tells it to cannot reach page 2, and is told its token was never issued.

This is reachable with no timing race at all through `CancelSession` (a `PENDING` session becomes `CANCELLED`, so a `SessionStatus EQ PENDING` filter stops matching it), and also by the clock alone, which moves every session `CREATING` -> `PENDING` after `SESSION_ACTIVATION_DELAY` and `PENDING` -> `FAILED` at its expiration. The handler's doc promises the opposite outcome for exactly this case: "a client paging with any `MaxResults` sees each session exactly once".

The unfiltered listings of this crate are not in the same position: `page_after_position` documents its refusal for "one of a record that has since gone", and a session only goes when its team goes (which makes `ListSessions` answer 404 anyway). Here the record has NOT gone — it is still a session of the team, at the position the token names.

## Evidence

The handler filters first and hands the FILTERED sequence to the position-token rule — `src/ops/list_sessions.rs:257-273`:
```rust
        let matching = state
            .team_sessions_in_initiation_order(&input.approval_team_arn)
            .into_iter()
            .filter(|session| {
                predicates
                    .iter()
                    .all(|predicate| predicate.matches(session))
            });
        let page = crate::state::page_after_seq(
            crate::state::Listing::Sessions {
                approval_team_arn: &input.approval_team_arn,
            },
            matching,
            after,
            input.max_results,
        )
        .map_err(ListSessionsErrorModeled::ValidationException)?;
```

The provenance test inside that rule — the token's position must be present in the sequence being paged, or the whole request is refused — `src/state.rs:604-614`:
```rust
    let mut handed_out = after.is_none();
    let mut records: Vec<(u64, &'a T)> = Vec::new();
    for (position, record) in ordered {
        match after {
            Some(after) if position <= after => handed_out |= position == after,
            _ => records.push((position, record)),
        }
    }
    if !handed_out {
        return Err(foreign_next_token());
    }
```

What the refusal it raises CLAIMS — `src/state.rs:3531-3545`:
```rust
/// The one message this service refuses a `NextToken` it did not issue with.
...
pub const FOREIGN_NEXT_TOKEN_MESSAGE: &str = "NextToken is not a token this service issued.";

/// The `ValidationException` (400) a `NextToken` no page of this service handed out is
/// refused with: [`FOREIGN_NEXT_TOKEN_MESSAGE`].
pub fn foreign_next_token() -> ValidationException {
```
and `src/state.rs:194-195` (module doc): "[`foreign_next_token`] / [`FOREIGN_NEXT_TOKEN_MESSAGE`] — the one message every paginated listing refuses a `NextToken` it did not issue with." The token in the scenario below WAS issued, by this same listing, one call earlier.

The token the listing hands out is the position of the last record it reported — `src/state.rs:571-574`:
```rust
    let mut reported: Vec<U> = ordered.into_iter().take(page_size + 1).collect();
    let more_remain = reported.len() > page_size;
    reported.truncate(page_size);
    let next_token = reported.last().filter(|_| more_remain).map(token_of);
```

The promise this breaks — `src/ops/list_sessions.rs:16-22`:
```text
//! `NextToken` is this listing's own: it names this listing — this operation, for THIS
//! team — and the initiation-order position of the last session the previous page
//! returned, so the next page is every matching session ordered after it; a client
//! paging with any `MaxResults` sees each session exactly once. A token this listing did
//! not hand out — one it cannot read, one another listing (or another team's sessions)
//! issued, or one naming a position this listing reports no session at — earns
//! `ValidationException` (400) rather than an empty page.
```

The writes that move a session out of a `SessionStatus` filter between two pages are this service's own: `src/ops/cancel_session.rs:201-206` (`SessionStatus::Creating | SessionStatus::Pending => { session.cancel(now); }`) and the clock in `src/state.rs:2665-2695` (`CREATING` -> `PENDING` on `activation_due`, then `PENDING` -> `FAILED`/`EXPIRED` on `expiration_time`).

Test coverage: the token-provenance test pages an UNFILTERED listing — `tests/all/session_listing.rs:838-854` ("The token the service itself issued is accepted, and resumes after the first page", request bodies `json!({ "MaxResults": 1 })` then `json!({ "MaxResults": 1, "NextToken": token })`, no `Filters`), and the filtered tests never page: `tests/all/session_listing.rs:959-975` (`listing_sessions_changes_nothing_a_later_caller_sees`) sends the same `MaxResults: 2` + `SessionStatus EQ PENDING` request twice but never feeds a token back. So no test pins paging a FILTERED listing across a status change.

## Steps to Reproduce

1. Start the emulator and point an MPA client at it; create an approval team and let it reach ACTIVE.
2. Call StartApprovalTeamBaseline twice on that team, so it holds two sessions (seq 1 and seq 2); let both reach PENDING (past SESSION_ACTIVATION_DELAY).
3. POST /approval-teams/{ApprovalTeamArn}/sessions/?List with body {"MaxResults": 1, "Filters": [{"FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING"}]}. Observe HTTP 200 with the first session and a NextToken (the token names position 1 — src/state.rs:571-574).
4. Call CancelSession on the session the first page reported (src/ops/cancel_session.rs:205 moves it to CANCELLED).
5. Repeat the ListSessions call of step 3 with the SAME filter plus "NextToken": <the token from step 3>.
6. Observe HTTP 400 ValidationException with Message "NextToken is not a token this service issued." — the filtered sequence no longer contains position 1, so `handed_out` stays false (src/state.rs:604-614) — instead of the second page holding the remaining PENDING session. Repeat without Filters and the same token answers 200 with that session, proving the token was this listing's own.
