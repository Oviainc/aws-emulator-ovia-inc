Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: f75cbb9a461527dc7539f53b0560573b007d83b1

# A ListParticipantReplicas nextToken carries only a store-wide sequence, so resuming it against a DIFFERENT participant or source stage serves a short or empty page instead of being refused

_Filed: 2026-10-29T03:23:29Z_
_Operation: ListParticipantReplicas | Class: pagination_

> _Note: this issue was identified against the code base at commit `f75cbb9a461527dc7539f53b0560573b007d83b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A `ListParticipantReplicas` page token is a marker plus one integer and nothing else: `encode_replica_page_token(sequence)` / `decode_replica_page_token(token)` wrap the shared `encode_after`/`decode_after` with `REPLICA_PAGE_TOKEN_PREFIX`. The handler accepts any token carrying that marker, whatever `sourceStageArn` and `participantId` the resuming request now names, and then applies the bare integer as a cutoff over that pair's replicas (`replica.sequence > after`).

The integer is NOT a per-participant (or per-stage) ordinal. A replica's `sequence` is drawn from the store-wide counter in `StartParticipantReplication` (`let sequence = store.next_sequence();`), and `participant_replicas_in` returns the records in the order they were pushed, paged by exactly that field. So the sequences of two publishers' replicas — and of two stages' replicas — interleave, and a token issued while paging publisher P1 names a position that can lie past some, or all, of publisher P2's replicas.

Result: `ListParticipantReplicas{sourceStageArn: S, participantId: P2, nextToken: <token issued for P1>}` answers HTTP 200 with `replicas: []` and no `nextToken` — telling a client P2 has no (more) replicas while the store holds replica records the same call without the token reports. That is the opposite of the rule the handler's own comment states: such a token is supposed to be refused because it "names a position" this listing did not issue. The page-token prefixes in `state.rs` are kept distinct for exactly that reason, but the participant and source-stage dimensions inside this one listing are not part of the token at all.

This is the replica-scoped counterpart of the already-filed ListParticipants / ListParticipantEvents / ListStageSessions holes; none of those issues mentions `ListParticipantReplicas`.

## Evidence

The handler states the rule it then breaks — `src/ops/list_participant_replicas.rs:163-178`:
```
        // A `nextToken` this listing did not issue names no position, so it is refused
        // rather than read as "start from the beginning" — a token from `ListStages`
        // included; see `crate::state::decode_replica_page_token`.
        let resume_after = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) => {
                Some(crate::state::decode_replica_page_token(token).ok_or_else(|| {
```
A token issued for another PARTICIPANT or another SOURCE STAGE is exactly such a token, yet nothing in the decode or the handler compares it against the pair being paged. The cutoff is applied as a bare integer — `src/ops/list_participant_replicas.rs:198-205`:
```
        let remaining: Vec<&crate::state::ParticipantReplicaRecord> = store
            .participant_replicas_in(&region, &input.source_stage_arn)
            .iter()
            .filter(|replica| {
                replica.participant_id == input.participant_id
                    && resume_after.is_none_or(|after| replica.sequence > after)
            })
            .collect();
```
and the token payload is only the marker plus that integer — `src/state.rs:4462-4473`:
```
/// [`encode_page_token`] for a `ListParticipantReplicas` page: the start sequence the
/// next page of replicas resumes after, under that listing's own marker.
pub fn encode_replica_page_token(after_sequence: u64) -> String {
    encode_after(REPLICA_PAGE_TOKEN_PREFIX, after_sequence)
}

/// The start sequence a replica `nextToken` resumes after, or `None` when the token was
/// not issued by [`encode_replica_page_token`] — a token from another listing
/// included.
pub fn decode_replica_page_token(token: &str) -> Option<u64> {
    decode_after(REPLICA_PAGE_TOKEN_PREFIX, token)
}
```
That integer is store-wide, not per participant — `src/ops/start_participant_replication.rs:338-352`:
```
        let sequence = store.next_sequence();
        let record = crate::state::ParticipantReplicaRecord {
            source_stage_arn: input.source_stage_arn.clone(),
            ...
            sequence,
        };
```
and the page order is that same field over a slice keyed only by the SOURCE STAGE — `src/state.rs:3522-3532`:
```
    /// The replications sourced from the stage `stage_arn` in `region`'s view, in the
    /// order they were started. Empty for a stage that view holds no replication for.
    pub fn participant_replicas_in(
        &self,
        region: &str,
        stage_arn: &str,
    ) -> &[ParticipantReplicaRecord] {
        request_scoped(stage_arn, region)
            .and_then(|stage_arn| self.participant_replicas.get(stage_arn))
            .map_or(&[], Vec::as_slice)
    }
```
The crate's own reason for keeping token namespaces apart — `src/state.rs:870-873`:
```
/// The same marker for a `ListParticipantReplicas` page, kept distinct from
/// [`PAGE_TOKEN_PREFIX`] so a token issued by one listing names no position in the
/// other and is refused there.
const REPLICA_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-replica-after:";
```
Test coverage: `tests/all/list_participant_replicas_paging.rs:558-609` builds its "refused" tokens only from ANOTHER listing (`ListStages`) and from junk strings (`"not-a-token"`, `"aGVsbG8tNw=="`, a 1024-char string). No case keeps a `ListParticipantReplicas` token and swaps the `participantId` or the `sourceStageArn`, which is why the hole is open; the SCOPE tests at `tests/all/list_participant_replicas_paging.rs:538-551` only check that a page without a token reports the pair asked for.

## Steps to Reproduce

1. CreateStage → S (source), and four more stages D1..D4 (destinations).
2. CreateParticipantToken{stageArn: S} twice → publishers P2 then P1 (default capabilities include PUBLISH).
3. StartParticipantReplication{sourceStageArn: S, destinationStageArn: D1, participantId: P2} and again with D2 → P2's two replicas take the two LOWEST replica sequences.
4. StartParticipantReplication{sourceStageArn: S, destinationStageArn: D3, participantId: P1} and again with D4 → P1's replicas take HIGHER sequences.
5. ListParticipantReplicas{sourceStageArn: S, participantId: P1, maxResults: 1} → HTTP 200, one replica plus a `nextToken`.
6. ListParticipantReplicas{sourceStageArn: S, participantId: P2, nextToken: <the token from step 5>} → HTTP 200 with `replicas: []` and no `nextToken`, although the same call without the token reports P2's two replicas.
7. Observe that the handler's own comment says a token that names no position in this listing must be refused with ValidationException (400), and that the same swap of a token from another LISTING is refused — so the behaviour is inconsistent within one rule.
