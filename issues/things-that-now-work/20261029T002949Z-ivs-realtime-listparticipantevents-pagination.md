Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# A ListParticipantEvents nextToken carries neither the session nor the participant it was issued for, so another participant's token silently drops events from the page

_Filed: 2026-10-29T00:29:48Z_
_Operation: ListParticipantEvents | Class: pagination_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListParticipantEvents` reports the occurrences of ONE participant during ONE stage session: the handler filters the session's events by `record.participant_id == input.participant_id` and then by the position a `nextToken` names. The token itself records only that position (the global store sequence of the last event reported), marked with a per-LISTING prefix, so it is accepted for any `(stageArn, sessionId, participantId)` triple.

Because a session's events for all of its participants are interleaved in one strictly increasing sequence drawn from the store's single counter, a token issued while paging participant A's events is a valid cut point for participant B of the SAME session: the page served for B silently omits every event of B that occurred before A's cut — commonly an empty `events` list with no `nextToken`, which a client reads as "this participant has no (more) events". Nothing is refused, and the dropped events are unrecoverable through pagination.

This is the condition the crate itself says a token must be refused for: the codec's comment grounds the refusal in "a token issued by one listing names no position in the other" because the pages "page over different records", and the handler restates it — but the only thing checked is the listing marker, never the session and participant the page is a page OF.

## Evidence

The token is a bare position under a listing-wide marker — `src/state.rs:4435-4446`:

    /// [`encode_page_token`] for a `ListParticipantEvents` page: the occurrence position of
    /// the last event it reported, which the next page resumes after, under that listing's
    /// own marker.
    pub fn encode_participant_event_page_token(after_sequence: u64) -> String {
        encode_after(PARTICIPANT_EVENT_PAGE_TOKEN_PREFIX, after_sequence)
    }
    ...
    pub fn decode_participant_event_page_token(token: &str) -> Option<u64> {
        decode_after(PARTICIPANT_EVENT_PAGE_TOKEN_PREFIX, token)
    }

and the marker's stated purpose — `src/state.rs:884-886`:

    /// The same marker for a `ListParticipantEvents` page, kept distinct from the others so
    /// a token issued by one listing names no position in another and is refused there.

The handler accepts any such token and applies it across the participant filter — `src/ops/list_participant_events.rs:189-205` and `:226-233`:

    // A `nextToken` this operation did not issue names no position, so it is
    // refused rather than read as "start from the beginning" — including a token
    // from this service's other listings, which name a position among records these
    // pages do not hold.
    let resume_after = match input.next_token.as_deref() { ... }
    ...
        .filter(|record| record.participant_id == input.participant_id)
        .filter(|record| resume_after.is_none_or(|after| record.sequence > after))

Events of all participants of a session share the one global sequence — `src/state.rs:2440` in the single event writer:

    let sequence = state.next_sequence();

over the counter documented at `src/state.rs:3311-3313` as the "Monotonic counter behind every id this service mints"; and `src/state.rs:2361-2364` records that this value is "Occurrence order within the session, and the stable key `ListParticipantEvents` pages over".

The page's declared scope is the triple — `src/ops/list_participant_events.rs:7-10`:

    //! The occurrences of ONE participant during ONE stage session: exactly the events the
    //! store holds for the `(stageArn, sessionId)` pair the request names
    //! ([`crate::state::StageSessionRecord`]) whose own `participantId` is the one the
    //! request names, in the order they occurred.

Test coverage: `tests/all/participant_events.rs:635-700` (`list_participant_events_refuses_a_next_token_it_did_not_issue`) pins only tokens from OTHER listings (`let participant_token = participants["nextToken"]`, taken from `ListParticipants`). No test offers a `ListParticipantEvents` token issued for one participant to another participant of the same session, so the behaviour above is unpinned.

## Steps to Reproduce

1. Create a stage with `CreateStage` and issue two participant tokens with `CreateParticipantToken`.
2. Bring both participants into the stage's session through the crate's data-plane seam the way the existing tests do (`state::join_stage_session`, as in `tests/all/participant_events.rs:150`), so each accumulates `JOINED`/`PUBLISH_STARTED` events interleaved in the session's occurrence order.
3. Call `ListParticipantEvents` for participant A with `maxResults: 1` and keep the returned `nextToken`.
4. Call `ListParticipantEvents` for participant B of the same `(stageArn, sessionId)` passing that `nextToken`.
5. Observe HTTP 200 with a page that omits every event of participant B whose occurrence position is below the cut A's token names (an empty `events` list and no `nextToken` once A's cut is past all of B's events), instead of the 400 `ValidationException` the handler's stated rule gives a token that "names a position among records these pages do not hold".
