Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 3bf50673ef9b9fde8f395d64d3e48e3a4a38bebc

# The cross-account source-role refusal drops the list position the crate's own member-path rule requires

_Filed: 2026-10-28T23:03:55Z_
_Operation: UpdateInvestigationGroup | Class: error-response_

> _Note: this issue was identified against the code base at commit `3bf50673ef9b9fde8f395d64d3e48e3a4a38bebc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crossAccountConfigurations` is a LIST the model allows up to 25 entries in, and the write path refuses a request whose entry names the investigation account itself. That refusal is worded by `state::cross_account_configurations_rejection`, which states its clause at the bare member path `crossAccountConfigurations.sourceRoleArn` — with no element position — so a caller that sent 25 configurations is told that "a" source role is wrong without being told WHICH entry the service refused.

The crate states the opposite rule twice, in the module that owns the wording. First, a handler-enforced refusal is supposed to be indistinguishable from a boundary refusal (`validation_message`: "Worded through the same builders as a boundary refusal, so the two cannot drift"). Second, the path rule explicitly keeps a list element's position: "A LIST element keeps its position (`<member>.<n>.member`), which is a fact about the request's shape rather than anything it sent" — i.e. the position is NOT caller-sent text and so is not withheld for the no-echo reason the rest of that section is about.

And the boundary refusal of the SAME member does carry the position today: the crate's own test pins `crossAccountConfigurations.2.member.sourceRoleArn` for an entry whose `sourceRoleArn` is below its modelled length minimum. So two refusals of the same list member, in the same request, use two different path grammars: the modelled rule names the entry, the service's own rule does not. That is the drift `validation_message`'s doc says cannot happen.

Both writes apply the chain (`CreateInvestigationGroup` and `UpdateInvestigationGroup` both call `state::write_configuration_rejection`); this is filed against `UpdateInvestigationGroup` because the (CreateInvestigationGroup, error-response) pair is already taken by the fixed map-key-echo issue. Fixing it changes the expected message in `tests/all/error_message_rules.rs:461-467`, which pins the position-less clause today.

## Evidence

The clause the handler-enforced rule states, with no element position — `services/service-com-amazonaws-aiops-aiops/src/state.rs:1174-1181`:
```
        .then(|| {
            validation_message(
                "crossAccountConfigurations.sourceRoleArn",
                &format!(
                    "Member must name an account other than the investigation account {own_account}"
                ),
            )
        })
```
The rule that says a handler-enforced refusal cannot differ from a boundary refusal — `src/state.rs:564-570`:
```
/// The whole `ValidationException` message for ONE member that failed ONE
/// requirement, for a rule a handler enforces itself (one the model does not express,
/// so the boundary walk passed it through). Worded through the same builders as a
/// boundary refusal, so the two cannot drift.
pub fn validation_message(member_path: &str, requirement: &str) -> String {
```
The path rule it drifts from — `src/state.rs:578-581`:
```
// the path a clause states stops at the map MEMBER: a map entry is reported at the
// member that holds it, with the requirement its value shape states, and never at a
// key. A LIST element keeps its position (`<member>.<n>.member`), which is a fact about
// the request's shape rather than anything it sent.
```
The other side: the boundary refusal of the same member DOES carry the position — `services/service-com-amazonaws-aiops-aiops/tests/all/map_member_refusal_paths.rs:320-337`:
```
    cases.push(Case {
        name: "a list element keeps its position".to_string(),
...
            "crossAccountConfigurations": [
                { "sourceRoleArn": "arn:aws:iam::000000000001:role/Source" },
                { "sourceRoleArn": "arn:aws:iam" },
            ],
...
        expected: member_message(
            "crossAccountConfigurations.2.member.sourceRoleArn",
            "Member must have length greater than or equal to 20",
        ),
    });
```
The call site on this operation — `src/ops/update_investigation_group.rs:202-207`:
```
        if let Some(why) = crate::state::write_configuration_rejection(
            input.encryption_configuration.as_ref(),
            input.cross_account_configurations.as_deref(),
```
which chains into the rule at `src/state.rs:1210-1212`:
```
    encryption_configuration_rejection(encryption_configuration)
        .or_else(|| cross_account_configurations_rejection(cross_account_configurations))
```
Test coverage of the current (position-less) wording — `tests/all/error_message_rules.rs:461-467`:
```
            envelope(&[clause(
                "crossAccountConfigurations.sourceRoleArn",
                &format!(
                    "Member must name an account other than the investigation account {}",
                    engine::EMULATOR_ACCOUNT_ID
                ),
            )]),
```
The model allows the many entries that make the position matter — `smithy_query(query="operation",operation="UpdateInvestigationGroup")`: `"crossAccountConfigurations": {"target": "CrossAccountConfigurations", "length": {"min": 0, "max": 25}, "type": "list"}`.

## Steps to Reproduce

1. Create an investigation group in us-west-2 and note its ARN
2. PATCH /investigationGroups/<url-encoded ARN> with {"crossAccountConfigurations":[{"sourceRoleArn":"arn:aws:iam::000000000001:role/Source"},{"sourceRoleArn":"arn:aws:iam::<EMULATOR_ACCOUNT_ID>:role/Own"}]}
3. Observe the 400 ValidationException message: "1 validation error detected: Value at 'crossAccountConfigurations.sourceRoleArn' failed to satisfy constraint: Member must name an account other than the investigation account <account>" — the entry that was refused is not named
4. Send the same request with the second entry's sourceRoleArn shortened to "arn:aws:iam" (below the modelled 20-character minimum): the boundary refusal names the entry as 'crossAccountConfigurations.2.member.sourceRoleArn' (pinned by tests/all/map_member_refusal_paths.rs:334)
5. Compare the two member paths with src/state.rs:578-581 ("A LIST element keeps its position") and src/state.rs:564-567 ("Worded through the same builders as a boundary refusal, so the two cannot drift")
