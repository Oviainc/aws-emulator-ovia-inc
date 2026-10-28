Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: db5b8cf4b40010fa5fa0137914026d79724270f1

# An input whose input security group was deleted can no longer be updated at all: the merged-record resolution refuses a reference the request never touched

_Filed: 2026-10-28T20:10:26Z_
_Operation: UpdateInput | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `db5b8cf4b40010fa5fa0137914026d79724270f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateInput` validates the MERGED record (the stored input plus the members this request named) with `ComAmazonawsMedialiveMedialiveState::input_configuration_error`, which refuses any `inputSecurityGroups` entry that `live_input_security_group` does not resolve — and `live_input_security_group` filters out a group whose delete was accepted.

`DeleteInputSecurityGroup` is a soft delete that DELIBERATELY deletes a group an input still uses (its handler says so in a comment, and a test row pins it). So the lawful sequence `CreateInputSecurityGroup` -> `CreateInput` (RTMP_PUSH, naming that group) -> `DeleteInputSecurityGroup` leaves an input whose stored configuration the crate's own write path refuses to store again: every later `UpdateInput` on that input — including one that only carries `{"name": "renamed"}` — is answered 400 `BadRequestException` with `unknown_reference_message("inputSecurityGroups", ...)`, for a member the request did not supply.

The input cannot even be repaired by clearing the member: a push input is held to `needs_input_whitelist()`, so `{"inputSecurityGroups": []}` is refused too (`required_for_input_type_message`). The only way out is attaching a different, still-live group — i.e. the delete silently turns a rename into an error.

The crate is inconsistent about this invariant: the sibling delete for the OTHER reference `input_configuration_error` resolves the same way — `DeleteSdiSource` — refuses to delete a source an input holds, exactly so the stored input keeps naming resolvable resources. `DeleteInputSecurityGroup` does not, and `input_configuration_error`'s own doc says the resolution exists because "an id nothing minted would leave the input claiming a resource no read of the account can show" — which is precisely the state the group delete produces.

## Evidence

The validator, applied to EVERY stored security-group reference regardless of what the request changed — `services/service-com-amazonaws-medialive-medialive/src/state.rs:3312-3320`:
```
        for group in &input.security_groups {
            if self.live_input_security_group(group).is_none() {
                return Some(unknown_reference_message(
                    "inputSecurityGroups",
                    "input security group",
                    group,
                ));
            }
        }
```
and its stated rationale — `src/state.rs:3283-3289`:
```
    /// * every id the configuration names must resolve to a resource this
    ///   account holds — an input security group, an SdiSource, a claimed input
    ///   device. ... so an id nothing minted would leave the input claiming a resource no
    ///   read of the account can show.
```

The resolution excludes a deleted group — `src/state.rs:3596-3600`:
```
    pub fn live_input_security_group(&self, id: &str) -> Option<&InputSecurityGroupRecord> {
        self.input_security_groups
            .get(id)
            .filter(|group| !group.is_deleted())
    }
```

It is run on the MERGED record, so an untouched member refuses the request — `src/ops/update_input.rs:289-300`:
```
        // The same rule `CreateInput` applies, to the merged record: an update
        // may no more leave an input naming a resource this account does not
        // hold, ... than a create may create one.
        if let Some(message) = state.input_configuration_error(&record) {
            return Err(UpdateInputErrorModeled::BadRequestException(BadRequestException {
                message: Some(message),
            })
            .into());
        }
```
(the merge above it, `src/ops/update_input.rs:245-248`, clones the STORED record: `let mut record = state.live_input(&input.input_id).cloned()`)

The delete that creates the unwritable state — `src/ops/delete_input_security_group.rs:193-201`:
```
        // DELIBERATELY unlike `BatchDelete`'s per-group delete
        // (`ops::batch_delete::delete_input_security_group`), which refuses a
        // group a resource still uses ... so a group something still uses is deleted here.
        crate::state::delete_input_security_group(group);
```

The sibling delete for the other reference the same validator resolves, which DOES preserve the invariant — `src/ops/delete_sdi_source.rs:173-184`:
```
        // "The SdiSource must not be part of any SdiSourceMapping and must not be
        // attached to any input": a source an input is using is a conflict, not a
        // deletion.
        if state.sdi_source_is_in_use(&input.sdi_source_id) {
            return Err(DeleteSdiSourceErrorModeled::ConflictException(ConflictException { ... }
```

Why clearing the member is not a way out — `src/state.rs:3339-3344`:
```
        if input.needs_input_whitelist() && input.security_groups.is_empty() {
            return Some(required_for_input_type_message(
                "inputSecurityGroups",
                input_type,
            ));
        }
```

Test coverage: `tests/all/input_security_group.rs:1284-1289` pins that deleting a group an input uses SUCCEEDS (table row `("an input uses it", ..., Some("input"), "IN_USE")` under `a_deleted_group_reaches_the_terminal_deleted_state`), so the delete side is fixed by test. `tests/all/input_update.rs:129-136` round-trips `inputSecurityGroups` replacement and `input_configuration_validation.rs:639-726` covers the merged-record `Vpc`/`inputSecurityGroups` rules — no test covers an update after the referenced group was deleted, so nothing pins the refusal reported here.

## Steps to Reproduce

1. POST /prod/inputSecurityGroups with {"whitelistRules":[{"cidr":"10.0.0.0/16"}]} -> note securityGroup.id as <group>
2. POST /prod/inputs with {"name":"ingest","type":"RTMP_PUSH","destinations":[{"streamName":"live"}],"inputSecurityGroups":["<group>"]} -> 201, note input id <input>
3. DELETE /prod/inputSecurityGroups/<group> -> 200 (the crate deliberately deletes a group an input uses)
4. PUT /prod/inputs/<input> with {"name":"renamed"} — the request names no security group, so expected 200 with the rename stored; the emulator answers 400 BadRequestException whose message is state::unknown_reference_message("inputSecurityGroups", "input security group", "<group>") and writes nothing
5. PUT /prod/inputs/<input> with {"name":"renamed","inputSecurityGroups":[]} — also 400, now from state::required_for_input_type_message("inputSecurityGroups", "RTMP_PUSH"): the input cannot be updated again until a different live group is attached
6. For comparison: DELETE /prod/sdiSources/<id> for a source an input holds is refused 409 by src/ops/delete_sdi_source.rs:176-184, so the SdiSource reference can never reach this state
