Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# A ListParticipants nextToken carries only a store-wide sequence, so resuming it against a different stage session serves a short or empty page instead of being refused

_Filed: 2026-10-28T19:50:10Z_
_Operation: ListParticipants | Class: pagination_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A `ListParticipants` page token is a marker plus one integer: `encode_participant_page_token(sequence)` / `decode_participant_page_token(token)` wrap `encode_after` / `decode_after` with `PARTICIPANT_PAGE_TOKEN_PREFIX` and nothing else. The handler accepts any token carrying that marker, whatever `stageArn` / `sessionId` the resuming request now names, and then applies the integer as a global cutoff (`record.sequence > after`).

The integer is NOT a per-session ordinal: a participant's `sequence` comes from the store-wide counter `state.next_sequence()` (state.rs:2780), and `participants_in_join_order` sorts by exactly that field (state.rs:2574-2577). So sequences of two sessions interleave, and a token issued for session A can name a position that lies past some — or all — of session B's participants.

Result: `ListParticipants{stageArn: S2, sessionId: <S2 session>, nextToken: <token issued for S1's session>}` answers HTTP 200 with a `participants` list that silently omits every participant of S2 whose sequence is below the cutoff, and with no `nextToken`. A client is told the session holds no (more) participants while the store holds CONNECTED records for it. That is the opposite of the refusal the handler's own comment states the rule as.

The same unscoped codec and the same accept-any-token branch are used by the two other identity-scoped listings of this crate, `ListParticipantEvents` (`decode_participant_event_page_token`, required `stageArn`/`sessionId`/`participantId`) and `ListParticipantReplicas` (`decode_replica_page_token`, required `sourceStageArn`/`participantId`), so the same short/empty page is reachable there by swapping the identity members while keeping the token.

## Evidence

The handler states the rule it then breaks — `src/ops/list_participants.rs:235-251`:

```
        // A `nextToken` this operation did not issue names no position, so it is
        // refused rather than read as "start from the beginning" — including a token
        // from this service's other listing, which names a position among records
        // these pages do not hold.
        let resume_after = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) => Some(
                crate::state::decode_participant_page_token(token).ok_or_else(|| {
```

A token issued for another SESSION is exactly "a position among records these pages do not hold", yet nothing in the decode or the handler compares it against the session being paged. The cutoff is applied globally — `src/ops/list_participants.rs:284`:

```
            .filter(|record| resume_after.is_none_or(|after| record.sequence > after))
```

The token payload carries only the marker and the integer — `src/state.rs:4428-4433`:

```
/// The join sequence a `ListParticipants` `nextToken` resumes after, or `None` when
/// the token was not issued by [`encode_participant_page_token`] — a token from
/// another listing included.
pub fn decode_participant_page_token(token: &str) -> Option<u64> {
    decode_after(PARTICIPANT_PAGE_TOKEN_PREFIX, token)
}
```

…and that integer is store-wide, not per session — `src/state.rs:2778-2780`:

```
    // Only a participant that is NEW to the session takes a page position, so the join
    // order of a session's participants stays contiguous.
    let sequence = state.next_sequence();
```

`src/state.rs:2574-2577` confirms the page order is that same field:

```
    pub fn participants_in_join_order(&self) -> Vec<&ParticipantRecord> {
        let mut participants: Vec<&ParticipantRecord> = self.participants.iter().collect();
        participants.sort_by_key(|record| record.sequence);
```

Test coverage: `tests/all/foreign_page_token_wording.rs:3-20` pins only "a token another listing of this same service really did issue" being refused; `tests/all/participants.rs` and `tests/all/list_participant_replicas_paging.rs:16-21` pin SCOPE ("A page reports the replicas of the named participant in the named source stage and no others") and page-token refusal as two SEPARATE rules — no test combines a token with a swapped identity, which is why the hole is open.

## Steps to Reproduce

1. Create stage S2 and bring two participants into its session FIRST (CreateStage, CreateParticipantToken twice, then `state::join_stage_session` for each, the seam every test in this crate uses to put a participant in a stage). These take the two lowest participant sequences in the store.
2. Create stage S1 and bring two participants into its session, which take higher sequences.
3. Call `ListParticipants{stageArn: S1, sessionId: <S1 activeSessionId>, maxResults: 1}` → 200 with one participant and a `nextToken`.
4. Call `ListParticipants{stageArn: S2, sessionId: <S2 activeSessionId>, nextToken: <the token from step 3>}` → 200 with `participants: []` and no `nextToken`, although S2's session holds two CONNECTED participants that `ListParticipants` without the token reports.
5. Repeat with `ListParticipantEvents` (swap `participantId`/`sessionId` while keeping the token) and `ListParticipantReplicas` (swap `sourceStageArn`/`participantId`) for the same silent omission.
