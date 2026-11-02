Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: cb9254e7b4a839ef1fe220636b4ce55207805929

# A ListParticipants nextToken scopes the stage and session but NOT the filter that selects the page, so resuming it under a different filter silently drops participants

_Filed: 2026-10-29T15:57:47Z_
_Operation: ListParticipants | Class: pagination_

> _Note: this issue was identified against the code base at commit `cb9254e7b4a839ef1fe220636b4ce55207805929`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListParticipants` pages over the participants its filter members select: `selected_by_filters` (`filterByUserId`, `filterByPublished`, `filterByState`, `filterByRecordingState`) is applied BEFORE the resume position is compared, and only then is `record.sequence > after` tested (src/ops/list_participants.rs:292-299). The token the page ends with, however, carries only three scope terms — the region, the stage ARN and the session id (src/ops/list_participants.rs:318-325 -> src/state.rs:4680-4691) — and the position it carries is a store-wide join `sequence`, not an ordinal in the filtered list.

The crate's own page-token codec states the rule this breaks: the scope a token carries is "the terms of the request that select them", so that a token offered to any other scope "names no position in the records that page holds, whatever store-wide sequence it carries" (src/state.rs:4875-4883). The four filter members are exactly such selecting terms — they are what `selected_by_filters` (src/ops/list_participants.rs:354-377) uses to decide which records the page holds — and none of them is in the scope.

Consequence: a caller that pages with `filterByUserId = "alice"` (or `filterByState = "CONNECTED"`, or `filterByPublished = true`) and then offers the resulting `nextToken` with a DIFFERENT filter, or with no filter at all, is answered HTTP 200 with a SHORT or EMPTY page. Every participant the new filter admits whose join `sequence` lies below the position the first page ended at is skipped and can never be reached by continuing to page. The honest answer, per the rule the codec states and per the refusal the handler already performs for a token of another listing/region/session, is the 400 `ValidationException` worded by `crate::state::foreign_page_token_message`.

This is NOT the gap the already-fixed issue "A ListParticipants nextToken carries only a store-wide sequence, so resuming it against a different stage session serves a short or empty page" closed: that one added the two PATH-like selecting terms (`stageArn`, `sessionId`) to the scope, which the code now carries. The FILTER terms — equally "terms of the request that select them" — were not added, and the same defect for the sibling listings' filters is filed as the open ListCompositions/pagination and ListIngestConfigurations/pagination issues.

## Evidence

The rule the crate states for every page token — src/state.rs:4875-4883:
```
/// The one page-token codec, so the listings cannot encode a position two ways.
///
/// A page position is an ordinal in ONE list of records, and which records a listing
/// pages over is decided by `scope`: the terms of the request that select them. The
/// token therefore carries that scope beside the position, and [`decode_after`] reads a
/// position out of it only for the scope the resuming request itself names — so a token
/// offered to any other scope names no position in the records that page holds, whatever
/// store-wide sequence it carries.
fn encode_after(prefix: &str, scope: &[&str], after_sequence: u64) -> String {
```

What this listing's token actually scopes — src/state.rs:4680-4691 (three terms, no filter):
```
pub fn encode_participant_page_token(
    region: &str,
    stage_arn: &str,
    session_id: &str,
    after_sequence: u64,
) -> String {
    encode_after(
        PARTICIPANT_PAGE_TOKEN_PREFIX,
        &[region, stage_arn, session_id],
        after_sequence,
    )
}
```

The filter that DOES select which records the page holds, applied before the resume position — src/ops/list_participants.rs:292-299:
```
        let matching: Vec<&crate::state::ParticipantRecord> = store
            .stage_session_in(&region, &input.stage_arn, &input.session_id)
            .map(crate::state::StageSessionRecord::participants_in_join_order)
            .unwrap_or_default()
            .into_iter()
            .filter(|record| selected_by_filters(&input, record))
            .filter(|record| resume_after.is_none_or(|after| record.sequence > after))
            .collect();
```
and the selection itself — src/ops/list_participants.rs:354-377:
```
fn selected_by_filters(
    input: &ListParticipantsInput,
    record: &crate::state::ParticipantRecord,
) -> bool {
    if let Some(user_id) = input.filter_by_user_id.as_deref() {
        if record.user_id.as_deref().unwrap_or_default() != user_id {
            return false;
        }
    }
    ...
```

The handler's own claim about which tokens cannot name a position here — src/ops/list_participants.rs:235-240 — it names the other listings and the other sessions, never the other filters:
```
        // A `nextToken` this operation did not issue names no position, so it is
        // refused rather than read as "start from the beginning" — including a token
        // from this service's other listing, and one this operation issued for ANOTHER
        // stage session, each of which names a position among records these pages do not
        // hold. A position is an ordinal among the participants of one session, so the
        // token is read only for the session this request itself names.
```

The position written into the token is a store-wide join sequence, not an ordinal in the filtered list — src/ops/list_participants.rs:318-325:
```
        let next_token = (matching.len() > page_size).then(|| {
            crate::state::encode_participant_page_token(
                &region,
                &input.stage_arn,
                &input.session_id,
                matching[page_size - 1].sequence,
            )
        });
```

Test coverage: no test combines a filter with a resume. `tests/all/participants.rs:407` (`list_participants_selects_the_participants_the_filter_names`) exercises the filters with no `nextToken`; `tests/all/participants.rs:673` (`list_participants_pages_the_session_in_join_order`) pages with no filter; `tests/all/participants.rs:806` (`list_participants_refuses_a_page_token_it_did_not_issue`) and `tests/all/page_token_scope.rs:777` (`a_participants_token_naming_a_position_past_another_session_is_refused`) cover only the listing/region/session scope terms. So nothing pins the cross-filter resume either way.

## Steps to Reproduce

1. Create one stage through one region's endpoint and seed its live session the way this crate's own participant suites do (`tests/all/participants.rs::populate` -> `state::join_stage_session`), in this join order: P1 with userId "alice", P2 with userId "bob", P3 with userId "bob". P1 therefore holds the lowest store sequence.
2. Read `stage.activeSessionId` off GetStage.
3. ListParticipants {"stageArn":"<stage>","sessionId":"<session>","filterByUserId":"bob","maxResults":1} — the page reports P2 and carries a nextToken naming P2's store-wide join sequence.
4. Offer that SAME nextToken back with a different filter: ListParticipants {"stageArn":"<stage>","sessionId":"<session>","filterByUserId":"alice","nextToken":"<token>"} (or with no filter at all).
5. Observe HTTP 200 with a page that silently omits P1 — its sequence is below the position the token names, so P1 can never be reached by continuing to page. Expected, per the rule src/state.rs:4875-4883 states and the refusal src/ops/list_participants.rs:241-258 already performs for a token offered to another scope: HTTP 400 ValidationException "ListParticipants.nextToken is required to be a pagination token issued by ListParticipants".
