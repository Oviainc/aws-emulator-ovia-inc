Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f75cbb9a461527dc7539f53b0560573b007d83b1

# The attributes merged onto a replica participant are never measured, so GetParticipant reports an attributes map past the 1 KB maximum this crate enforces on every other attributes map

_Filed: 2026-10-29T03:23:29Z_
_Operation: StartParticipantReplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f75cbb9a461527dc7539f53b0560573b007d83b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartParticipantReplication` checks the 1 KB `attributes` bound on the REQUEST's map only, then merges that map over the source participant's own attributes and persists the result as the replica participant's attributes in the destination session. The merged map is never measured, so two individually legal maps (e.g. 600 bytes on the token plus 600 bytes on the replication request) produce a stored map of ~1200 bytes, which `GetParticipant` / `ListParticipants` then report as the replica's `Participant$attributes` — a field the model documents with the very same 1 KB maximum, and the field this crate's own `MAX_ATTRIBUTES_BYTES` says it bounds "on every shape of this service that carries one".

The crate implements that documented rule in one place (`check_attributes_size`) and routes all four operations that take an `attributes` map through it, but the one map this service COMPOSES and stores escapes the check: the only value measured is the request's half of it.

## Evidence

The model states the bound on the field that reports the merged value — `smithy_query(query="doc", shape="Participant", member="attributes")`:
> "Application-provided attributes to encode into the token and attach to a stage. Map keys and values can contain UTF-8 encoded text. **The maximum length of this field is 1 KB total.**"

and on the request member, together with the merge it mandates — `smithy_query(query="doc", shape="StartParticipantReplicationRequest", member="attributes")`:
> "... The maximum length of this field is 1 KB total. ... These attributes are merged with any attributes set for this participant when creating the token. If there is overlap in keys, the values in these attributes are replaced."

The handler measures only the request's map — `src/ops/start_participant_replication.rs:205-212`:
```
        let requested_attributes = input.attributes.unwrap_or_default();
        crate::state::check_attributes_size(Self::NAME, "attributes", &requested_attributes)
            .map_err(|message| {
                crate::state::validation_failed(
                    StartParticipantReplicationErrorModeled::ValidationException,
                    message,
                )
            })?;
```
and then stores the unmeasured merge — `src/ops/start_participant_replication.rs:331-349`:
```
        // "These attributes are merged with any attributes set for this participant when
        // creating the token. If there is overlap in keys, the values in these attributes
        // are replaced" — so the source participant's own attributes first, then the
        // request's over them.
        let mut attributes = participant.attributes.clone();
        attributes.extend(requested_attributes);
        ...
            attributes,
```
That merged map is what the destination session's record carries — `src/state.rs:3104-3117` (`record_replicated_participant`):
```
    let ends = [
        (
            &replica.source_stage_arn,
            &replica.source_session_id,
            &identity.attributes,
            false,
        ),
        (
            &replica.destination_stage_arn,
            &replica.destination_session_id,
            &replica.attributes,
            true,
        ),
    ];
```
and it is reported verbatim — `src/state.rs:2263`:
```
            attributes: (!self.attributes.is_empty()).then(|| self.attributes.clone()),
```
The crate states the rule it skips here — `src/state.rs:399-412`:
```
/// Total size an `attributes` map may reach, on every shape of this service that
/// carries one.
...
pub const MAX_ATTRIBUTES_BYTES: usize = 1024;
```
and — `src/state.rs:5219-5227`:
```
/// Refuse a request whose `attributes` map is larger than the documented
/// [`MAX_ATTRIBUTES_BYTES`] total. ...
/// ... it lives here once and every operation taking an
/// `attributes` map maps the sentence onto its own error.
```
Test coverage: `tests/all/attributes_size_limit.rs:3-12` drives the bound "through EVERY operation of this service that takes an `attributes` map" — but only per request map; `tests/all/attributes_size_limit.rs:381-410` bounds `StartParticipantReplication`'s own member and no case reads the MERGED map back off the replica, so nothing in the crate justifies or pins the overflow.

## Steps to Reproduce

1. CreateStage → S (source) and D (destination).
2. CreateParticipantToken{stageArn: S, attributes: <a map whose keys+values total ~600 bytes>} → publisher P (HTTP 200: under the 1 KB bound).
3. StartParticipantReplication{sourceStageArn: S, destinationStageArn: D, participantId: P, attributes: <a different map totalling ~600 bytes>} → HTTP 200 (each map is under the 1 KB bound on its own).
4. GetStage{arn: D} → read `stage.activeSessionId` as SID.
5. GetParticipant{stageArn: D, sessionId: SID, participantId: P} → `participant.attributes` is the union of both maps, ~1200 bytes, although `Participant$attributes` documents "The maximum length of this field is 1 KB total" and the crate's `MAX_ATTRIBUTES_BYTES` claims to bound that map "on every shape of this service that carries one".
6. Send the same two maps to `CreateParticipantToken` (or `CreateIngestConfiguration`) as ONE map and observe the 400 `ValidationException` the same total is refused with there.
