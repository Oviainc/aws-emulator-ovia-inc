Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# Connector-operation ARN is mis-derived when the connector name contains ":connector/"

_Filed: 2026-10-28T17:34:25Z_
_Operation: RestartConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::connector_operation_arn` derives the ARN of a new connector operation by splitting the connector's ARN on the LAST occurrence of `":connector/"` (`rsplit_once`). A connector name is a plain `__stringMin1Max128` with no `@pattern`, so a name may itself contain `":connector/"`, and it is embedded verbatim in the connector's ARN resource field by `CreateConnector`. For such a connector the split lands inside the NAME instead of at the ARN's resource-type boundary: part of the name is kept in the "scope" half and the minted ARN is no longer the connector's scope with the resource re-typed `connector-operation/<name>/<id>` — its resource type stays `connector`.

For a connector named `a:connector/b` with ARN `arn:aws:kafkaconnect:eu-west-1:000000000000:connector/a:connector/b/<uuid>`, both handlers that start an operation (`RestartConnector` and `UpdateConnector`, which share `state.start_connector_operation`) answer `connectorOperationArn = arn:aws:kafkaconnect:eu-west-1:000000000000:connector/a:connector-operation/b/<uuid>/<opid>`. That value is handed to the client as the operation's identity, and it contradicts both the helper's own documented rule and the rule this crate's test suite states (an ARN whose resource begins `connector-operation/`). Using `split_once` — the FIRST occurrence, which is the real resource-type boundary minted by `emulator_arn` — derives the documented ARN for every name the model admits.

## Evidence

The documented rule, and the code that breaks it, in the same function — `src/state.rs:1787-1796`:

    /// An operation belongs to its connector, so its ARN is the connector's own —
    /// same partition, region and account, so neither the region of the call that
    /// started the operation nor anything else can move it to another connector's
    /// scope — with the `connector/<name>/<id>` resource re-typed
    /// `connector-operation/<name>/<id>` and the operation's own minted id
    /// appended.
    pub fn connector_operation_arn(connector_arn: &str, operation_id: &str) -> Option<String> {
        let (scope, resource) = connector_arn.rsplit_once(":connector/")?;
        (!resource.is_empty())
            .then(|| format!("{scope}:{CONNECTOR_OPERATION_KIND}/{resource}/{operation_id}"))

The name reaches the ARN verbatim — `src/ops/create_connector.rs:412-417`:

        let id = state.mint_resource_id("connector", &input.connector_name);
        let arn = ::engine::emulator_arn(
            crate::state::ARN_SERVICE,
            &region,
            &format!("connector/{}/{}", input.connector_name, id),
        );

The model allows such a name — `OPERATIONS.md:29`: "`connectorName` (string, len 1..128, required)" (`__stringMin1Max128` carries a length only, no `@pattern`), so `:` and `/` are admitted characters.

Both operations that mint an operation ARN go through the shared seam — `src/state.rs:1604` (`start_connector_operation`): "Ok(connector_operation_arn(connector_arn, &id).unwrap_or_else(…", called from `src/ops/restart_connector.rs:226` and `src/ops/update_connector.rs:298`.

The crate's own test states the rule that is broken — `tests/all/connector_operation_minting_is_one_rule.rs:386-390`:

        let prefix = format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector-operation/");
        assert!(
            operation_arn.starts_with(&prefix),
            "case {case:?}: expected a connectorOperationArn starting {prefix}, got \
             {operation_arn:?}"

That test cannot catch this input twice over: every case uses a simple name, and its expectation helper re-implements the same split (`tests/all/connector_operation_minting_is_one_rule.rs:147`: "let (scope, resource) = connector_arn.rsplit_once(\":connector/\").unwrap_or_else(…"), so it mirrors the defect instead of checking the stated rule.

## Steps to Reproduce

1. Create an ACTIVE custom plugin (POST /v1/custom-plugins) and wait out crate::state::TRANSITION_SPAN.
2. POST /v1/connectors with "connectorName": "a:connector/b" (128 chars or fewer, so the model accepts it) referencing that plugin; note the returned connectorArn `arn:aws:kafkaconnect:<region>:000000000000:connector/a:connector/b/<uuid>`.
3. Wait out TRANSITION_SPAN so the connector is RUNNING.
4. POST /v1/connectors/{connectorArn}/restart (or PUT /v1/connectors/{connectorArn}?currentVersion=1 with a capacity body).
5. Read connectorOperationArn in the response: it is `arn:aws:kafkaconnect:<region>:000000000000:connector/a:connector-operation/b/<uuid>/<opid>` — the resource type is `connector`, not `connector-operation`, and part of the connector name has moved into the ARN's scope half, contrary to state.rs:1787-1792 and the prefix the suite asserts at connector_operation_minting_is_one_rule.rs:386.
