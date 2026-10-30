Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 060392db00c5b3260ef85f15c64a4c64c2352242

# An empty ProgramName is stored as a multiplex program's identity, creating a program no single-program operation can name

_Filed: 2026-10-29T05:05:48Z_
_Operation: CreateMultiplexProgram | Class: validation_

> _Note: this issue was identified against the code base at commit `060392db00c5b3260ef85f15c64a4c64c2352242`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateMultiplexProgram` stores the request's `ProgramName` verbatim as the key of the program inside its multiplex, with no check that the caller supplied a non-empty name. `ProgramName` is `@required` but targets bare `__string`, so the request boundary admits `""` (the member is present), and `insert_multiplex_program` checks only the settings ranges, the multiplex's existence and state, the name-already-in-use rule, the `ProgramNumber` uniqueness rule, the per-multiplex cap and the PID-block allocation — never the name itself.

The result is a stored program whose identity is the empty string. The record's own doc says that key "identifies the program within its multiplex", and the only three operations that address one program (`DescribeMultiplexProgram`, `UpdateMultiplexProgram`, `DeleteMultiplexProgram`) carry `ProgramName` as an `@httpLabel` path segment of `/prod/multiplexes/{MultiplexId}/programs/{ProgramName}` — an empty segment does not form that path, so no request names the program the call just created. It is reported by `ListMultiplexPrograms` with `programName: ""`, it holds one of the multiplex's program slots and one of its allocated PID blocks, and a retry under the same empty name is answered 409 `ProgramNameInUse`, so the slot stays taken for the multiplex's life (only deleting the whole multiplex clears it). A request naming no program is thus answered 201 as if it had worked.

This crate already states the rule for exactly this situation — a required, unbounded, caller-chosen string that IS a resource's identity — in two sibling places, and both refuse an empty value: the schedule slice refuses an empty `ActionName`, and `ClaimDevice` refuses an empty `Id` even though the model makes that member optional. `CreateMultiplexProgram` models the `BadRequestException` (400) such a refusal would use.

## Evidence

The model admits the value: `smithy_query(query="operation", operation="CreateMultiplexProgram")` gives `"ProgramName": {"target": "__string", "jsonName": "programName", "clientOptional": true, "required": true}` — required, with no `@length`/`@pattern` — and `smithy_query(query="doc", shape="CreateMultiplexProgramRequest", member="ProgramName")` is only "Name of multiplex program."

The name is the identity and the path label: `smithy_query(query="operation", operation="DescribeMultiplexProgram,DeleteMultiplexProgram")` gives for both `"ProgramName": {"target": "__string", "httpLabel": true, "required": true}` with `"uri": "/prod/multiplexes/{MultiplexId}/programs/{ProgramName}"`, and the stored record says the same — services/service-com-amazonaws-medialive-medialive/src/state/multiplexes.rs:772-775:
```
pub struct MultiplexProgramRecord {
    /// The `ProgramName` the request created the program under, which is also
    /// the key it is stored at: it identifies the program within its multiplex.
    pub program_name: String,
```

The write that never checks it — src/state/multiplexes.rs:1495-1511 (the complete list of refusals before the insert):
```
    ) -> Result<(), MultiplexProgramRefusal> {
        if let Some(refusal) = program_settings_refusal(&multiplex_program_settings) {
            return Err(MultiplexProgramRefusal::Settings(refusal));
        }

        let multiplex = self
            .multiplexes
            .get_mut(multiplex_id)
            .ok_or(MultiplexProgramRefusal::MultiplexNotFound)?;
        if !multiplex.accepts_programs() {
            return Err(MultiplexProgramRefusal::MultiplexGone(
                multiplex.state.clone(),
            ));
        }
        if multiplex.programs.contains_key(&program_name) {
            return Err(MultiplexProgramRefusal::ProgramNameInUse);
        }
```
and the handler that hands the name straight to it — src/ops/create_multiplex_program.rs:198-204:
```
        state
            .insert_multiplex_program(
                &input.multiplex_id,
                input.program_name.clone(),
                input.multiplex_program_settings,
            )
```

The sibling rule this crate applies to a required, unbounded, caller-chosen identity — src/state/schedules.rs:536-545:
```
        for (index, action) in creates.iter().enumerate() {
            let path = format!("creates/scheduleActions/{index}");
            let name = action.action_name.as_str();
            if name.is_empty() {
                problems.push(problem(
                    &format!("{path}/actionName"),
                    "actionName must not be empty",
                ));
                continue;
            }
```
with the reason stated at src/state/schedules.rs:380-386: "Keyed by action name, because the model makes the name the schedule's identity: \"must be unique within the schedule. This name provides the main reference to an action once it is added to the schedule.\"" — and the second copy of the same rule, on a member the model does not even mark required, src/ops/claim_device.rs:147-156:
```
        // The device id is what the claim is ABOUT, so a request without one
        // names no device to claim: the operation's own modelled 400, never a
        // device registered under an empty id.
        let id = input.id.unwrap_or_default();
        if id.is_empty() {
            return Err(ClaimDeviceErrorModeled::BadRequestException(BadRequestException {
                message: Some("Id is required: it is the id of the device to claim".to_string()),
```

Nothing pins the current behaviour: tests/all/multiplex_programs.rs always creates programs under non-empty names (`"programName": program_name` at :103, `"news"` at :1024) and no test in tests/all sends an empty `programName` (grep for `"programName": ""` across tests/all finds no match).

## Steps to Reproduce

1. POST /prod/multiplexes with a name, two availabilityZones, valid multiplexSettings and a requestId; poll GET /prod/multiplexes/{id} until state is IDLE. Call the id M.
2. POST /prod/multiplexes/M/programs with body {"programName": "", "requestId": "t1", "multiplexProgramSettings": {"programNumber": 1, "preferredChannelPipeline": "CURRENTLY_PREFERRED"}} — observed: 201 with multiplexProgram.programName "" and a freshly allocated packetIdentifiersMap. Expected: the 400 BadRequestException this operation models, as src/state/schedules.rs:539 answers for an empty actionName.
3. GET /prod/multiplexes/M/programs — the program is listed with "programName": "", so a client can see it exists.
4. Try to read, update or delete it: all three operations put ProgramName in the path (/prod/multiplexes/M/programs/{ProgramName}), and an empty segment does not form that path, so no request names this program.
5. POST /prod/multiplexes/M/programs again with {"programName": ""} and a new requestId — 409 ConflictException (ProgramNameInUse), confirming the unaddressable program keeps its program slot and its PID block until the whole multiplex is deleted.
