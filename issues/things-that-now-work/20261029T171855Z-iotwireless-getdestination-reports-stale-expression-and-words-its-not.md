Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-destination-lifecycle
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-destination-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|GetDestination|4xx->4xx|$.Message
Issue-Key: iotwireless|GetDestination|4xx->4xx|$.ResourceId
Issue-Key: iotwireless|GetDestination|4xx->4xx|$.ResourceType
Issue-Key: iotwireless|GetDestination|2xx->2xx|$.Expression
Issue-Key: iotwireless|GetDestination|2xx->2xx|$.ExpressionType
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# GetDestination reports stale expression and words its not-found refusal wrongly

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDestination

## Description
Two net-new causes on the destination read path: the 404 refusal is worded and shaped unlike AWS's, and a successful read serves the destination's create-time expression rather than its current one.

Cause 1 of 2 — the not-found refusal (fixture iot-wireless-destination-lifecycle, step task#9).
AWS: 404 ResourceNotFoundException (Smithy: GetDestination errors include ResourceNotFoundException @httpError 404; the shape has members Message, ResourceId, ResourceType). The recorded refusal of a destination name the account does not hold carries Message = "Destination doesn't exist" — entirely FIXED text, with no part taken from the request or from state — and carries NO ResourceId and NO ResourceType member at all. That the two optional members are absent for this refusal is recorded behavior only; the model merely permits them.
Emulator: 404 with Message = "Destination <name> not found", i.e. its own wording with the requested name interpolated, and it populates ResourceId with the requested name and ResourceType with "Destination" (services/service-com-amazonaws-iotwireless-iotwireless/src/ops/get_destination.rs:160-168).
Why: a client or assertion matching AWS's fixed sentence never matches, and a client reading ResourceId/ResourceType sees members AWS leaves unset for this refusal — so code that branches on their presence takes a path a real account never produces.
Fix: GetDestination's not-found refusal in its handler — the error-composition site that builds ResourceNotFoundException for a missing destination (message text fixed, the two optional members left unset).
Traces: iot-wireless-destination-lifecycle#task#9.

Cause 2 of 2 — a successful read serves the create-time Expression/ExpressionType after the destination has been updated (fixture iot-wireless-destination-lifecycle, step task#6).
AWS: 200 with the destination's CURRENT routing target (Smithy: GetDestinationResponse members Expression (length 0..2048) and ExpressionType (enum), described as "The rule name or topic rule to send messages to" / "The type of value in Expression"). After an UpdateDestination that moves the destination from a rule target to an MQTT topic target, the read reports the new Expression and ExpressionType (recorded: an MQTT topic value with ExpressionType "MqttTopic").
Emulator: 200, but Expression/ExpressionType are still the values the create set (recorded run: a rule-name value with ExpressionType "RuleName"). The destination record is never re-written, because UpdateDestination is still an unimplemented scaffold that answers 501 and touches no state (services/service-com-amazonaws-iotwireless-iotwireless/src/ops/update_destination.rs:147-165), while GetDestination faithfully echoes whatever CreateDestination stored (ops/get_destination.rs:170-177).
Why: a client that updates a destination's routing target and then reads it back is told its update did not take effect; every subsequent decision about where device messages are routed is made on the create-time value.
Fix: UpdateDestination's handler and the destination record in this crate's state — the update must replace Expression/ExpressionType (and Description/RoleArn) on the stored destination so the read path reports them. GetDestination's projection itself needs no change beyond reading the updated record.
Traces: iot-wireless-destination-lifecycle#task#6.

Related: the blocked UpdateDestination step in the same fixture is owned by the scaffold-stub issue filed for that operation; this issue is about the read path's observable divergence (a stale 200 projection and a mis-worded 404), which that stub issue does not state.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline:

1) iot-wireless-destination-lifecycle#task#9 GetDestination — expected 404, actual 404; $.Message: value_mismatch (expected "Destination doesn't exist", got 'Destination sim2real-probe-iotw-dest-7a4b87f7 not found'); $.ResourceId: type_mismatch (expected None, got the requested destination name); $.ResourceType: type_mismatch (expected None, got 'Destination').
2) iot-wireless-destination-lifecycle#task#6 GetDestination — expected 200, actual 200; $.Expression: value_mismatch (expected the MQTT topic the preceding update set, got the rule-name value the create set); $.ExpressionType: value_mismatch (expected 'MqttTopic', got 'RuleName').

Model evidence (services/service-com-amazonaws-iotwireless-iotwireless/smithy-model/com-amazonaws-iotwireless-iotwireless.json): GetDestination GET /destinations/{Name} -> 200, output { Arn, Name, Expression, ExpressionType, Description, RoleArn }, errors include ResourceNotFoundException @httpError 404 { Message, ResourceId, ResourceType }; UpdateDestination PATCH /destinations/{Name} -> 204 with optional ExpressionType/Expression/Description/RoleArn.
Crate evidence: src/ops/get_destination.rs:160-168 builds the not-found error with an interpolated message and both optional members set; src/ops/get_destination.rs:170-177 projects the stored record verbatim; src/ops/update_destination.rs:147-165 is still `Err(Scaffolded::not_implemented("UpdateDestination"))`, so no update is ever applied to the stored destination.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 2: replay fixture iot-wireless-destination-lifecycle through step task#6 — create a destination with a RuleName expression, update it to an MqttTopic expression, then GetDestination; the response still reports the create-time Expression and ExpressionType "RuleName".
3. Cause 1: replay the same fixture's step task#9 — GetDestination for a destination name that is not held; the 404 body carries the emulator's interpolated "Destination <name> not found" plus ResourceId and ResourceType, instead of the fixed "Destination doesn't exist" with both members absent.
