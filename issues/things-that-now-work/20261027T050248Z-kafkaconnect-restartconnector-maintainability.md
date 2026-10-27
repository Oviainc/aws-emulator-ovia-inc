Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 53db56bf3158c7205a85c81c618ec0b3c4193a64

# De-duplicate the connector-operation minting rule shared by RestartConnector and UpdateConnector

_Filed: 2026-10-27T05:02:48Z_
_Operation: RestartConnector | Class: maintainability_

> _Note: this issue was identified against the code base at commit `53db56bf3158c7205a85c81c618ec0b3c4193a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Starting a connector operation is ONE rule in this crate, but it is written twice, verbatim (comments included), in the two handlers that start one — `src/ops/restart_connector.rs` and `src/ops/update_connector.rs`. Two copies are involved:

1. The per-connector operation meter and its refusal message: `state.connector_operation_quota_reached(&connector_arn)` followed by the same `TooManyRequestsException` message string rendered from `crate::state::RESOURCE_QUOTA`.
2. The operation's ARN computation: `state.mint_resource_id("connector-operation", &connector_name)` followed by `crate::state::connector_operation_arn(&connector_arn, &id).unwrap_or_else(|| ::engine::emulator_arn(ARN_SERVICE, &region, &format!("connector-operation/{connector_name}/{id}")))` — the same 10-line computation, including the identical 4-line explanatory comment about the fallback.

The crate already keeps the ONE-rule shape for the other rule these two handlers share: the capacity rule lives in `crate::state::capacity_refusal` and the "not RUNNING" reason in `crate::state::connector_is_not_running()`, both reached from both handlers, and `tests/all/capacity_rule_is_one_rule.rs` exists to pin that the capacity rule is one rule. The two copies above are the remaining hand-copied pair, so a change to the id kind string, the ARN resource prefix, the fallback, or the quota wording can land in one handler and not the other — and the two paths would then mint operation ARNs or word the 429 differently for the same store.

The fix is mechanical and response-preserving: move the mint+derive computation into one `crate::state` helper (e.g. a method on the state struct taking `&connector_arn`, `&connector_name`, `&region` and returning the minted ARN) and the quota message into one `crate::state` function, then have both handlers call them and keep constructing their own modelled error variant (the error enum is per-operation, so only the message text is shared). Every response body and status stays byte-identical, and every existing test keeps passing.

## Evidence

Copy 1 — services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/restart_connector.rs:207-232:
```
        // The operations stored against one connector are metered, so a
        // connector restarted without end does not grow this store without end.
        if state.connector_operation_quota_reached(&connector_arn) {
            return Err(RestartConnectorErrorModeled::TooManyRequestsException(
                TooManyRequestsException {
                    message: Some(format!(
                        "Limit exceeded. Resource limit reached: this connector already holds {} operations.",
                        crate::state::RESOURCE_QUOTA
                    )),
...
        let id = state.mint_resource_id("connector-operation", &connector_name);
        // The operation's ARN is derived from the connector's, so it names the
        // same partition, region and account the connector itself lives in. The
        // fallback covers only a connector ARN of a shape this service did not
        // mint, which keeps the handler total without a panic.
        let arn = crate::state::connector_operation_arn(&connector_arn, &id).unwrap_or_else(|| {
            ::engine::emulator_arn(
                crate::state::ARN_SERVICE,
                &region,
                &format!("connector-operation/{connector_name}/{id}"),
            )
        });
```

Copy 2 — services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/update_connector.rs:279-304: character-for-character the same two blocks, differing only in the error enum name (`UpdateConnectorErrorModeled`) and one word of the first comment ("updated" vs "restarted"):
```
        if state.connector_operation_quota_reached(&connector_arn) {
            return Err(UpdateConnectorErrorModeled::TooManyRequestsException(
                TooManyRequestsException {
                    message: Some(format!(
                        "Limit exceeded. Resource limit reached: this connector already holds {} operations.",
                        crate::state::RESOURCE_QUOTA
                    )),
...
        let id = state.mint_resource_id("connector-operation", &connector_name);
        // The operation's ARN is derived from the connector's, so it names the
        // same partition, region and account the connector itself lives in. The
        // fallback covers only a connector ARN of a shape this service did not
        // mint, which keeps the handler total without a panic.
        let arn = crate::state::connector_operation_arn(&connector_arn, &id).unwrap_or_else(|| {
            ::engine::emulator_arn(
                crate::state::ARN_SERVICE,
                &region,
                &format!("connector-operation/{connector_name}/{id}"),
            )
        });
```

The crate's own precedent that such a shared rule belongs in ONE place — services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/state.rs:277-291: "The ONE capacity check every operation that takes a capacity runs: a create on the `Capacity` its request carried, an update on the `Capacity` its `CapacityUpdate` converts to ([`capacity_from_update`]), so neither can accept a capacity the other refuses."

Test coverage that pins the behaviour these copies produce (and that must stay green and unchanged):
- tests/all/connector_operations.rs:320-330 — the restart path's operation ARN shape: `let prefix = format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector-operation/{}/", ...)`, asserted with "expected a connectorOperationArn starting {prefix}".
- tests/all/connector_update.rs:463-467 — the SAME assertion written again for the update path: `let prefix = format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector-operation/{name}/");`.
- tests/all/restart_connector_scope.rs:334, tests/all/connector_operation_window.rs:507, tests/all/region_scoping.rs:508 and tests/all/describe_connector_operation.rs:487 all read back `connectorOperationArn` from one of the two paths.
- No test exercises the 429 branch (`grep -n "Limit exceeded" tests/` matches nothing), so the shared message text must be preserved verbatim when it moves.

## Steps to Reproduce

1. Open services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/restart_connector.rs and read lines 207-232.
2. Open services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/update_connector.rs and read lines 279-304; the quota check with its message and the mint+derive-ARN computation with its comment are the same code twice, differing only in the per-operation error enum name.
3. Change the minted id kind string ("connector-operation") or the ARN fallback resource format in ONE of the two files: the other path keeps minting ARNs the old way, and tests/all/connector_operations.rs:320-330 and tests/all/connector_update.rs:463-467 would then assert two different ARN shapes for the same store.
4. Confirm the crate already centralises the sibling rule (crate::state::capacity_refusal / crate::state::connector_is_not_running), which both handlers call instead of duplicating.
