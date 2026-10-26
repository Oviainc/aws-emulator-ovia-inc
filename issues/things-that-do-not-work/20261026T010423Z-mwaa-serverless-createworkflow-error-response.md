Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9b93cbc54d897386c30bfd66ea5bd8f85f03a356

# CreateWorkflow's ClientToken fingerprint is positionally ambiguous, so a reused token with a different request replays the first workflow instead of answering ConflictException

_Filed: 2026-10-26T01:04:23Z_
_Operation: CreateWorkflow | Class: error-response_

> _Note: this issue was identified against the code base at commit `9b93cbc54d897386c30bfd66ea5bd8f85f03a356`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkflow` decides whether a repeated `@idempotencyToken` is a replay or a conflict by comparing a SHA-256 digest of the request (`request_fingerprint`). The digest is taken over a `Vec<String>` whose length DEPENDS on which optional members the request carries: `EncryptionConfiguration` contributes 2 fields when present and 0 when absent, `LoggingConfiguration` 1 or 0, `NetworkConfiguration` one field per security group id plus one per subnet id, and `Tags` two fields per entry — all appended with no placeholder marking which member they came from. The only optional member that keeps its position is `Code` (`None => fields.push(String::new())`).

Because of that, two genuinely different requests can produce the SAME field list and therefore the same fingerprint. Examples (both pass every modelled constraint):

* A: `LoggingConfiguration = {LogGroupName: "sg-1"}` (no NetworkConfiguration) vs B: `NetworkConfiguration = {SecurityGroupIds: ["sg-1"]}` (no LoggingConfiguration) — both append exactly `["sg-1"]`.
* A: `Tags = {"subnet-x": "subnet-y"}` vs B: `NetworkConfiguration = {SubnetIds: ["subnet-x", "subnet-y"]}` — both append exactly `["subnet-x", "subnet-y"]`.

Presented with the same `ClientToken`, the second request is taken for a repeat of the first: the handler returns 200 with the FIRST workflow's ARN/version and silently discards the second request's configuration, instead of the `ConflictException` the handler's own comment (and the operation's modelled error set) says a token reused with different parameters must get. The caller is told its create succeeded while nothing it asked for (the network configuration, or the tags) was stored anywhere.

The same ambiguity exists in `UpdateWorkflow::request_fingerprint` (src/ops/update_workflow.rs:363-398), where it only feeds the minted version id and is masked by the nonce — so the fix belongs in how the fingerprint enumerates members, not in the comparison.

## Evidence

The rule the code states for itself, src/ops/create_workflow.rs:274-277:
```
        // A repeated @idempotencyToken replays its first answer rather than minting a
        // second workflow; the same token with a different request is a conflict. The
        // token is remembered per region, because a token presented at one regional
        // endpoint says nothing about what another endpoint created.
```
and the comparison it is enforced with, src/ops/create_workflow.rs:280:
```
                if recorded.request_fingerprint() != fingerprint {
```

The fingerprint that cannot support that comparison, src/ops/create_workflow.rs:528-554 — `Code` gets a positional placeholder, the other optional members get none:
```
    match input.code.as_ref() {
        Some(Code::S3Location(location)) => { ... }
        None => fields.push(String::new()),
    }
    if let Some(encryption) = input.encryption_configuration.as_ref() {
        fields.push(encryption.r#type.clone());
        fields.push(encryption.kms_key_id.clone().unwrap_or_default());
    }
    if let Some(logging) = input.logging_configuration.as_ref() {
        fields.push(logging.log_group_name.clone());
    }
    if let Some(network) = input.network_configuration.as_ref() {
        fields.extend(network.security_group_ids.clone().unwrap_or_default());
        fields.extend(network.subnet_ids.clone().unwrap_or_default());
    }
```
The digest only protects against ambiguity WITHIN a field, not against two different member sets producing the same field list — src/state.rs:771-773:
```
/// A lowercase-hex SHA-256 digest over `fields`, each field length-delimited so
/// `["a", "bc"]` and `["ab", "c"]` never collide.
```

Test coverage: tests/all/workflow_lifecycle.rs:484-500 (`a_repeated_client_token_replays_the_first_create`) exercises the mismatch branch with a request that differs only in `Name` — a member at a FIXED position — so the collision families above are not covered by any existing test:
```
        // The same token with a DIFFERENT request is a conflict, not a second workflow.
        ... "Name": format!("tokened{index}-other"), ... "ClientToken": token,
        assert_eq!(status, 409, ...);
```

## Steps to Reproduce

1. CreateWorkflow with Name="fp-a", RoleArn/DefinitionS3Location valid, ClientToken="tok", and LoggingConfiguration={"LogGroupName": "sg-1"} — note the returned WorkflowArn.
2. CreateWorkflow again with the SAME ClientToken="tok" and the SAME Name/RoleArn/DefinitionS3Location, but with LoggingConfiguration omitted and NetworkConfiguration={"SecurityGroupIds": ["sg-1"]} instead.
3. Observe HTTP 200 carrying the first call's WorkflowArn/WorkflowVersion instead of the 409 ConflictException the handler documents for "the same token with a different request".
4. GetWorkflow on that ARN: it reports the first request's LoggingConfiguration and no NetworkConfiguration — the second request's configuration was never stored, and no error told the caller.
5. The same happens for Tags={"subnet-x":"subnet-y"} versus NetworkConfiguration={"SubnetIds":["subnet-x","subnet-y"]} under one ClientToken.
