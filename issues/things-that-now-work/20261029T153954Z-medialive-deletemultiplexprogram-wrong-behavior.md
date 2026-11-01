Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: de2d8767cb82ce68bb0f28dff69738907970fd37

# Deleting a multiplex program does not forget its idempotency token, so the spent RequestId latches onto a later program that reuses the name

_Filed: 2026-10-29T15:39:54Z_
_Operation: DeleteMultiplexProgram | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `de2d8767cb82ce68bb0f28dff69738907970fd37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other resource of this crate forgets a create's `RequestId` when the resource it minted goes away — the state module states the rule twice ("A token never outlives the channel it minted", "A token never outlives the resource it minted") and nine delete/settle paths call `IdempotencyTokens::forget`. `remove_multiplex_program` is the one delete that removes the resource and leaves the token behind.

Because the multiplex-program token map records a program NAME (a program has no minted id), the surviving entry does not simply go stale: a later `CreateMultiplexProgram` in the same multiplex that reuses the deleted program's name makes the old token resolve again — to a program a DIFFERENT request created, with settings the replaying request never asked for. `CreateMultiplexProgram` then answers 201 with that other program and stores nothing, where the same request under any unspent `RequestId` would get the 409 `ConflictException` "already has a program named …".

The handler comment asserts the opposite of what the code does, which is the inconsistency: it claims the token can neither "resolve to anything once the program it created is gone", while `multiplex_program_for_token` resolves the stored NAME against whatever program currently holds it.

## Evidence

The delete removes the program and never forgets the token — services/service-com-amazonaws-medialive-medialive/src/state/multiplexes.rs:1688-1692:
```
        let Some(multiplex) = self.multiplexes.get_mut(multiplex_id) else {
            return Err(MultiplexProgramDeleteRefusal::MultiplexNotFound);
        };
        multiplex.programs.remove(program_name);
        Ok(program)
```

The crate's own rule, applied by the sibling delete — src/state.rs:3948-3951:
```
        // A token never outlives the resource it minted, so a retried create
        // cannot resurrect a deleted signal map.
        self.signal_map_tokens.forget(&id);
```
and src/state.rs:2468-2474:
```
        // A token never outlives the channel it minted: once that channel has
        // reached the terminal `DELETED` state its idempotency window is over, so
        // a retried create mints a new channel rather than answering with a
        // deleted one.
        for id in just_deleted {
            self.channel_tokens.forget(&id);
        }
```
(the same `forget` call is made by src/state/channel_placement_groups.rs:412, src/state/clusters.rs:478, src/state/alarm_templates.rs:595, src/state/alarm_template_groups.rs:253, src/state/event_bridge_rule_templates.rs:498, src/state.rs:2487 and src/state.rs:2504.)

The token resolves by NAME, so a reused name revives it — src/state/multiplexes.rs:1723-1731:
```
        let program_name = self
            .multiplex_program_tokens
            .created(&multiplex_program_token_key(multiplex_id, request_id))?;
        let program = self
            .multiplexes
            .get(multiplex_id)?
            .programs
            .get(program_name)?;
        Some(program.program_name.as_str())
```

The comment the code contradicts — src/ops/create_multiplex_program.rs:205-213:
```
        // Bounded, evicting write: a caller-chosen token can neither accumulate
        // past the cap nor resolve to anything once the program it created is
        // gone (`multiplex_program_for_token` answers only for a program that
        // exists).
        state.record_multiplex_program_token(
```
and the replay path it feeds, src/ops/create_multiplex_program.rs:181-191, which returns 201 with `multiplex_program_description(multiplex_id, program_name)`.

Test coverage: the multiplex-program suites cover idempotency only while the program still exists (tests/all/multiplex_programs.rs:583 `a_retried_create_returns_the_program_the_first_one_created`, :694 `a_retried_create_is_idempotent_only_for_the_multiplex_it_was_addressed_to`); tests/all/multiplex_program_deletion.rs contains no `requestId` at all, so nothing pins the post-delete behaviour. The parallel rule IS pinned for the resource that forgets its token: tests/all/channel_placement_group_idempotency_after_delete.rs.

## Steps to Reproduce

1. CreateMultiplex and let it settle to IDLE; call the resulting multiplex M.
2. POST /prod/multiplexes/M/programs with {"programName":"A","requestId":"T1","multiplexProgramSettings":{"programNumber":1,...}} → 201.
3. DELETE /prod/multiplexes/M/programs/A → 200 (the program leaves the store; the token entry for key "M/T1" -> "A" stays).
4. POST /prod/multiplexes/M/programs with {"programName":"A","requestId":"T2","multiplexProgramSettings":{"programNumber":2,...}} → 201, creating a different program that reuses the name "A".
5. Retry step 2's request verbatim (requestId "T1", programNumber 1). Expected: the same answer any unspent token gets — 409 ConflictException "Multiplex M already has a program named A". Actual: 201 replaying the program created in step 4, reporting programNumber 2, with nothing written.
