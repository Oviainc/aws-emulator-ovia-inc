Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 5072f4ec107ec0dab088411d8137f3fc4cd946a3

# A ListStageSessions nextToken carries only a store-wide sequence, so resuming it against a DIFFERENT stage serves a short or empty page instead of being refused

_Filed: 2026-10-29T02:01:03Z_
_Operation: ListStageSessions | Class: pagination_

> _Note: this issue was identified against the code base at commit `5072f4ec107ec0dab088411d8137f3fc4cd946a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A `ListStageSessions` page token is a marker plus one integer and nothing else: `encode_stage_session_page_token(sequence)` / `decode_stage_session_page_token(token)` wrap the shared `encode_after`/`decode_after` with `STAGE_SESSION_PAGE_TOKEN_PREFIX`. The handler accepts any token carrying that marker, whatever `stageArn` the resuming request now names, and then applies the integer as a cutoff over that stage's sessions (`record.sequence > after`).

The integer is NOT a per-stage ordinal. A session's `sequence` is drawn from the store-wide counter in `ensure_active_session` (`let sequence = state.next_sequence();`), and `stage_sessions_in_start_order` pages by exactly that field. So the sequences of two stages' sessions interleave, and a token issued while paging stage S1 names a position that can lie past some — or all — of stage S2's sessions.

Result: `ListStageSessions{stageArn: S2, nextToken: <token issued for S1>}` answers HTTP 200 with a `stageSessions` list that silently omits every session of S2 whose sequence is below the cutoff, and with no `nextToken` — telling a client S2 has had no (more) sessions while the store holds session records for it that the same call without the token reports. That is the opposite of the rule the handler's own comment states: the token is supposed to be refused when it "names a position among records these pages do not hold". The page-token prefixes in `state.rs` are kept distinct for exactly that reason — "a position in one is meaningless in the other" — but the stage dimension inside this one listing is not part of the token at all.

This is the stage-scoped counterpart of the already-filed identity-scoped holes; neither the ListParticipants nor the ListParticipantEvents issue mentions `ListStageSessions`.

## Evidence

The handler states the rule it then breaks — `src/ops/list_stage_sessions.rs:179-195`:
```
        // A `nextToken` this operation did not issue names no position, so it is
        // refused rather than read as "start from the beginning" — including a token
        // from this service's other listings, which name a position among records
        // these pages do not hold.
        let resume_after = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) => Some(
                crate::state::decode_stage_session_page_token(token).ok_or_else(|| {
```
A token issued for another STAGE is exactly "a position among records these pages do not hold", yet nothing in the decode or the handler compares it against the stage being paged. The cutoff is applied as a bare integer — `src/ops/list_stage_sessions.rs:214-218`:
```
        let remaining: Vec<&crate::state::StageSessionRecord> = store
            .stage_sessions_in_start_order(&region, &input.stage_arn)
            .into_iter()
            .filter(|record| resume_after.is_none_or(|after| record.sequence > after))
            .collect();
```
The token payload is only the marker plus that integer — `src/state.rs:4449-4459`:
```
/// [`encode_page_token`] for a `ListStageSessions` page: the start sequence the next
/// page of stage sessions resumes after, under that listing's own marker.
pub fn encode_stage_session_page_token(after_sequence: u64) -> String {
    encode_after(STAGE_SESSION_PAGE_TOKEN_PREFIX, after_sequence)
}

/// The start sequence a stage-session `nextToken` resumes after, or `None` when the
/// token was not issued by [`encode_stage_session_page_token`] — a token from another
/// listing included.
pub fn decode_stage_session_page_token(token: &str) -> Option<u64> {
    decode_after(STAGE_SESSION_PAGE_TOKEN_PREFIX, token)
}
```
and that integer is store-wide, not per stage — `src/state.rs:4026-4038` (`ensure_active_session`):
```
    let sequence = state.next_sequence();
    let session_id = stage_session_id(sequence);
    state
        .stage_sessions
        .entry(stage_arn.to_string())
        .or_default()
        .push(StageSessionRecord {
            ...
            sequence,
        });
```
and the page order is that same field — `src/state.rs:3494-3504`:
```
    pub fn stage_sessions_in_start_order(
        &self,
        region: &str,
        stage_arn: &str,
    ) -> Vec<&StageSessionRecord> {
        let mut sessions: Vec<&StageSessionRecord> = request_scoped(stage_arn, region)
            .and_then(|stage_arn| self.stage_sessions.get(stage_arn))
            .map(|sessions| sessions.iter().collect())
            .unwrap_or_default();
        sessions.sort_by_key(|record| record.sequence);
```
The crate's own reason for keeping token namespaces apart — `src/state.rs:880-882`:
```
/// The same marker for a `ListStageSessions` page, kept distinct from the others so a
/// token issued by one listing names no position in another and is refused there.
const STAGE_SESSION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-stage-session-after:";
```

Test coverage: `tests/all/stage_sessions.rs:15-16` pins only "A `nextToken` this operation did not issue is refused with `ValidationException` (400)", and the case at `tests/all/stage_sessions.rs:559-621` builds its foreign tokens from OTHER listings (`stage_token`, `participant_token`) plus junk strings. No case keeps a `ListStageSessions` token and swaps the `stageArn`, which is why the hole is open.

## Steps to Reproduce

1. Create stage S2 first (CreateStage) and drive two sessions for it through `state::join_stage_session` / `state::leave_stage_session` (the data-plane seam every test in this crate uses, since no control-plane operation opens a session). These take the two lowest session sequences in the store.
2. Create stage S1 and drive two sessions for it the same way; they take higher sequences.
3. Call `ListStageSessions{stageArn: S1, maxResults: 1}` → HTTP 200 with one session and a `nextToken`.
4. Call `ListStageSessions{stageArn: S2, nextToken: <the token from step 3>}` → HTTP 200 with `stageSessions: []` and no `nextToken`, although `ListStageSessions{stageArn: S2}` without the token reports two sessions.
5. Observe that the handler's own comment says such a token must be refused with ValidationException (400), and that the same swap against a token from another LISTING is refused, so the behaviour is inconsistent within one rule.
