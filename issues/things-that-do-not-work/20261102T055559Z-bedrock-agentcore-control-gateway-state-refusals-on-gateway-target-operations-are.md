Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-gateway-target-workflow
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-gateway-target-workflow
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateGatewayTarget|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|DeleteGatewayTarget|4xx->4xx|$.message
Issue-Commit: 307ae70e1176afe8148743d9dbd874f3de53a94d

# Gateway-state refusals on gateway target operations are reworded and no longer match the recorded ValidationException sentence

> _Note: this issue was identified against the code base at commit `307ae70e1176afe8148743d9dbd874f3de53a94d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGatewayTarget

## Description
A target operation issued against a gateway that is not yet in a usable state is refused with the right status and error class but with a message of a different shape than the one the corpus recorded, on both the create and the delete path. 2 causes must both be fixed before this issue is closed.

NOTE for the fixer: open issue 20261029T212239Z-bedrock-agentcore-control-refusal-messages-diverge-from-aws.md (its cause 3) reports the INVERSE for UpdateGateway — there the recorded sentence is "<Operation> operation can't be performed on gateway when it is in <State> state. Retry the request when the gateway is in a valid state." and the emulator produced "Cannot perform operation UpdateGateway when gateway is in UPDATING status". The two corpora therefore pin DIFFERENT sentences for the gateway-level operation (UpdateGateway/DeleteGateway) and for the target-level operations (CreateGatewayTarget/DeleteGatewayTarget). Do not fix this by re-flipping one shared formatter: the invalid-state message must be formatted per operation family so both the gateway-level and the target-level recordings are satisfied. That issue does not carry these replay keys and names neither target operation.

Cause 1 of 2: CreateGatewayTarget against a gateway still in CREATING is refused with the wrong message template.
AWS: Smithy CreateGatewayTarget errors include ValidationException (httpError 400, members message required + reason required); the gatewayIdentifier is an @httpLabel on POST /gateways/{gatewayIdentifier}/targets/. The model does not state the sentence, so this is recorded behavior only: the 400 message is "Cannot perform operation <OperationName> when gateway is in <STATUS> status" — fixed text "Cannot perform operation ", " when gateway is in " and " status"; the variable parts are the operation being attempted (CreateGatewayTarget) and the gateway's current status rendered as the raw GatewayStatus enum token in upper case, taken from the emulator's state for the gateway named in the path.
Emulator: answers 400 with "CreateGatewayTarget operation can't be performed on gateway when it is in Creating state. Retry the request when the gateway is in a valid state." — a different sentence, the status capitalised rather than the enum token, and an extra retry clause.
Why: a client that matches the refusal text to decide whether to poll GetGateway until the gateway leaves CREATING and then retry the target create does not recognise the sentence, and anything parsing the status token out of the message reads "Creating" where the contract carries the enum value "CREATING".
Fix: the gateway-state guard in the CreateGatewayTarget handler's validation in the bedrock-agentcore-control service crate (the invalid-state message formatter it calls), kept distinct from the gateway-level formatter named in the related issue above.
Traces: bedrock-agentcore-control-gateway-target-workflow#task#2.

Cause 2 of 2: DeleteGatewayTarget against a gateway still in CREATING is refused with the same wrong template.
AWS: Smithy DeleteGatewayTarget (DELETE /gateways/{gatewayIdentifier}/targets/{targetId}/) errors include ValidationException (httpError 400). Recorded behavior only: the 400 message is the same template, "Cannot perform operation DeleteGatewayTarget when gateway is in <STATUS> status", with the operation name and the raw upper-case gateway status as its only variable parts.
Emulator: answers 400 with "DeleteGatewayTarget operation can't be performed on gateway when it is in Creating state. Retry the request when the gateway is in a valid state."
Why: same as cause 1 — teardown logic that distinguishes a transient state conflict from a permanent validation failure by message text cannot do so, and the operation name/status pair it would extract is in a different form.
Fix: the gateway-state guard in the DeleteGatewayTarget handler's validation (same shared formatter as cause 1; both fall out of one correction).
Traces: bedrock-agentcore-control-gateway-target-workflow#task#4.

## Evidence
Cause 1 — fixture bedrock-agentcore-control-gateway-target-workflow, step task#2 (CreateGatewayTarget): status 400 expected and 400 returned, error class matched; $.message value_mismatch — expected 'Cannot perform operation CreateGatewayTarget when gateway is in CREATING status', got "CreateGatewayTarget operation can't be performed on gateway when it is in Creating state. Retry the request when the gateway is in a valid state."
Cause 2 — same fixture, step task#4 (DeleteGatewayTarget): status 400 expected and returned; $.message expected 'Cannot perform operation DeleteGatewayTarget when gateway is in CREATING status', got "DeleteGatewayTarget operation can't be performed on gateway when it is in Creating state. Retry the request when the gateway is in a valid state."
In both, the only diverging field is the message; the operation name and the gateway status are the template's variable parts (here CreateGatewayTarget/DeleteGatewayTarget and CREATING, taken from the request and the gateway's state), the rest is fixed text.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Replay fixture bedrock-agentcore-control-gateway-target-workflow: setup#1 creates a gateway (CreateGateway, 202) which remains in CREATING.
3. Cause 1: step task#2 POSTs /gateways/{gatewayIdentifier}/targets/ (CreateGatewayTarget) while that gateway is CREATING; read $.message of the 400.
4. Cause 2: step task#4 DELETEs /gateways/{gatewayIdentifier}/targets/{targetId}/ (DeleteGatewayTarget) in the same state; read $.message of the 400.
5. Compare each message with the recorded 'Cannot perform operation <Operation> when gateway is in CREATING status'.
