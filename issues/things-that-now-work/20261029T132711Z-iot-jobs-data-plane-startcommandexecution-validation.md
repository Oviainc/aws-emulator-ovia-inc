Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9644ddf994ccf47d3ab59a1dffbb24b4a89d5fe4

# StartCommandExecution never checks `parameters` NAMES against CommandParameterName, although the crate's own rule says a map key is the handler's to apply

_Filed: 2026-10-29T13:27:11Z_
_Operation: StartCommandExecution | Class: validation_

> _Note: this issue was identified against the code base at commit `9644ddf994ccf47d3ab59a1dffbb24b4a89d5fe4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartCommandExecution$parameters` targets `CommandExecutionParameterMap`, whose KEY shape `CommandParameterName` declares `@length(1..192)` and `@pattern ^[.$a-zA-Z0-9_-]+$`. The handler does exactly two things with the map: it asks whether it is non-empty (for the static-payload refusal) and it renders it into the job document. No name is ever tested, so a parameter name that is empty, longer than 192 characters, or carries a character outside the declared pattern (a space, a slash, a newline, a non-ASCII character) is accepted with HTTP 200, an `executionId` is minted, and the name is baked verbatim into the persisted `jobDocument` the device later fetches through `DescribeJobExecution` / `StartNextPendingJobExecution`.

This is not a constraint the framework applies ahead of the handler. The shared module this crate's state re-exports states the opposite rule explicitly — a map's KEY is not walked as a constrained leaf, so "the name's own constraints are therefore the handler's to apply" — and that is precisely why the sibling `statusDetails` path (`UpdateJobExecution`, `StartNextPendingJobExecution`) applies `DetailsKey`'s length AND pattern inside `check_status_details`. The already-FIXED issue `20261028T151436Z-iot-jobs-data-plane-updatejobexecution-validation` pinned the same point with a passing test ("the proof the 128-character `@length` is not applied anywhere in the request walk"). `StartCommandExecution` is the one map-accepting path of this crate that skips the check its own crate performs for every other map.

## Evidence

The model's constraint on the map KEY — `smithy_query(query="shape", shape="CommandExecutionParameterMap")`:
```
"CommandExecutionParameterMap": {"type": "map", "length": {"min": 1}, "key": {"target": "CommandParameterName", "length": {"min": 1, "max": 192}, "pattern": "^[.$a-zA-Z0-9_-]+$"}, "value": "CommandParameterValue"}
```

The crate's own stated rule that a map key is the handler's to check — services/common/src/iot_job_executions.rs:100-106:
```
/// A map's key and value each carry their OWN declared constraints, and a name is
/// bounded by the KEY shape's, never by the ceiling published for a value. The walk
/// the framework runs ahead of a handler treats a map's key as a path segment rather
/// than as a constrained leaf, so it reports a value outside `DetailsValue`'s
/// declared length or pattern and reports NOTHING about a name: the name's own
/// constraints are therefore the handler's to apply, and the value's declared ones
/// are not re-checked here.
```
and the sibling check it justifies — services/common/src/iot_job_executions.rs:117-129:
```
    for (name, value) in details {
        let name_len = name.chars().count();
        if name_len > MAX_STATUS_DETAIL_NAME_LEN {
...
        if !is_modeled_status_detail_name(name) {
            return Err(format!(
                "the statusDetails name {name} is not 1..{MAX_STATUS_DETAIL_NAME_LEN} characters matching {STATUS_DETAIL_NAME_PATTERN}"
```
reached from both sibling handlers: services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/update_job_execution.rs:339 `if let Err(reason) = record.try_replace_status_details(input.status_details.as_ref()) {` and .../src/ops/start_next_pending_job_execution.rs:203 `if let Err(reason) = crate::state::check_status_details(input.status_details.as_ref()) {`.

The side that skips it — .../src/ops/start_command_execution.rs:476-485, the ONLY inspection `parameters` gets:
```
        let supplies_parameters = input
            .parameters
            .as_ref()
            .is_some_and(|parameters| !parameters.is_empty());
        if command.static_payload && supplies_parameters {
            return Err(invalid_request(STATIC_PAYLOAD_PARAMETERS_MESSAGE.to_string()));
        }

        let parameters = parameters_json(input.parameters.as_ref());
        let document = job_document(&input.command_arn, &parameters);
```
and the renderer that copies every name through unexamined — .../src/ops/start_command_execution.rs:319-327:
```
    let mut pairs: Vec<(&String, &CommandParameterValue)> =
        parameters.into_iter().flatten().collect();
    pairs.sort_unstable_by_key(|(name, _)| *name);
    serde_json::Value::Object(
        pairs
            .into_iter()
            .map(|(name, value)| (name.clone(), parameter_value_json(value)))
            .collect(),
    )
```

Test coverage: the crate's `parameters` tests only ever submit well-formed names — tests/all/command_execution_validation.rs:554 `parameters: r#"{ "level": { "S": "high" } }"#` and tests/all/job_execution_lifecycle.rs:342 `("two parameters", json!({"mode": {"S": "pulse"}, "power": {"I": 7}}))` — so no existing test pins an out-of-constraint name as accepted. The key-constraint rule itself is covered for `statusDetails` by `status_details_are_bounded_and_an_over_bound_map_changes_nothing` (tests/all/job_execution_lifecycle.rs:1271) and by `a_status_details_name_is_admitted_only_within_the_key_constraint` (services/common/src/iot_job_executions.rs:1110).

## Steps to Reproduce

1. Register a templated (non-static-payload, non-deprecated) command in the shared IoT command index, e.g. `arn:aws:iot:us-east-1:111122223333:command/reboot`, as `iot:CreateCommand` would.
2. POST /command-executions with body `{"targetArn": "arn:aws:iot:us-east-1:111122223333:thing/laser-1", "commandArn": "arn:aws:iot:us-east-1:111122223333:command/reboot", "parameters": {"bad name!": {"S": "x"}}}`.
3. Observe HTTP 200 and a minted `executionId`, although `CommandParameterName` declares `@pattern ^[.$a-zA-Z0-9_-]+$` — the sibling `statusDetails` path refuses the analogous name against `DetailsKey`.
4. GET /things/laser-1/jobs/{executionId} and observe the out-of-pattern name persisted inside `execution.jobDocument.parameters`.
5. Repeat with a 300-character name and with the empty name `""` (the key shape declares `@length(1..192)`) and observe both are likewise accepted and stored.
