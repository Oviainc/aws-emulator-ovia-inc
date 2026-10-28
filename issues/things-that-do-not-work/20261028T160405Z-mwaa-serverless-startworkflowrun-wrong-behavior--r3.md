Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# StartWorkflowRun's ClientToken fingerprint hashes nested OverrideParameters through HashMap Debug order, so an identical retry can answer 409

_Filed: 2026-10-28T16:04:05Z_
_Operation: StartWorkflowRun | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartWorkflowRun`'s idempotency fingerprint canonicalises only the TOP level of `OverrideParameters`. The top-level entries are collected and `parameters.sort()`ed by key, but each VALUE is reduced to a string with `format!("{value:?}")`. An `OverrideParameters` value is an `aws_smithy_types::Document`, and a JSON object inside it is deserialised into a `std::collections::HashMap`, whose `Debug` rendering emits its entries in the map's own iteration order. Two separately deserialised HashMaps holding the same keys do not iterate in the same order (each `HashMap` gets its own `RandomState`), so the SAME request body digests to DIFFERENT fingerprints on two calls whenever any override value is an object with two or more members.

The consequence is the opposite of what the operation's `@idempotencyToken` promises: the retry with the identical body and the identical `ClientToken` compares unequal in `TokenLedger::replay`, which answers the modelled `ConflictException` ("ClientToken ... was already used for a StartWorkflowRun request with different parameters") instead of replaying the run the first call started. A caller retrying after a timeout therefore gets a nondeterministic 409 for a request that never changed — a success turned into an error — and the first run is left unreported. The handler's own comment states the rule it then breaks for nested maps ("`OverrideParameters` is an unordered map on the wire, so digest it in key order"), and the function's doc claims the digest is "stable".

## Evidence

The handler's own rule and the unstable rendering, `services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/start_workflow_run.rs:465-481`:
```
/// A stable digest of the request, so a repeated `ClientToken` can be told apart
/// from a token reused for a different request.
...
fn request_fingerprint(input: &StartWorkflowRunInput) -> String {
    // `OverrideParameters` is an unordered map on the wire, so digest it in key order.
    let mut parameters: Vec<(String, String)> = input
        .override_parameters
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, format!("{value:?}")))
        .collect();
    parameters.sort();
```
A document object IS a `HashMap`, `vendor/smithy-schema-server/src/json.rs:1047-1051`:
```
                Token::StartObject { .. } => {
                    let mut out = std::collections::HashMap::new();
                    loop {
                        match d.next_token()? {
                            Token::EndObject { .. } => return Ok(Document::Object(out)),
```
A differing fingerprint is answered as a conflict, `services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state.rs:1004-1013`:
```
        if recorded.request_fingerprint() != request_fingerprint {
            return Err(ConflictException {
                message: format!(
                    "ClientToken {token} was already used for a {} request with different \
                     parameters",
```
What the model promises for the member (`smithy_query(query="shape", shape="StartWorkflowRunRequest")`, `ClientToken`): "A unique, case-sensitive identifier that you provide to ensure the idempotency of the request. This token prevents duplicate workflow run requests."

Test coverage does not reach it: the replay test uses a FLAT single-key map, `tests/all/workflow_lifecycle.rs:931-945` (`"OverrideParameters": {"token": token}` — the value is a `Document::String`, whose Debug is stable), and `tests/all/idempotency_token_ledger.rs:109` uses `{"retries": 3}`. The nested-object case exists only in the persistence table, `tests/all/workflow_lifecycle.rs:665-671` (`{"config": {"region": "us-east-1", "tags": ["a", "b"]}, ...}`), which never re-sends a ClientToken.

## Steps to Reproduce

1. CreateWorkflow a workflow and note its WorkflowArn.
2. Call StartWorkflowRun with {"WorkflowArn": arn, "ClientToken": "tok-1", "OverrideParameters": {"config": {"region": "us-east-1", "zone": "a"}}} and observe 200 with a RunId.
3. Re-send the byte-identical StartWorkflowRun request (the retry a client makes after a timeout).
4. Observe that the second call does not reliably replay: because the nested object's two keys are rendered in a fresh HashMap's iteration order, the fingerprint frequently differs and the call answers 409 ConflictException ("already used ... with different parameters") instead of 200 with the first RunId.
5. Repeat the pair a few times (or add more keys to the nested object) to see the answer flip between a correct replay and a spurious conflict.
