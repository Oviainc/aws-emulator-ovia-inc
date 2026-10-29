Issue-Source: trace-sweep-divergence
Issue-Trace: supplychain-instance-error-paths
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: supplychain-instance-error-paths supplychain-instance-lifecycle supplychain-list-instances-pagination supplychain-tagging-workflow
Issue-Shape-Id: com.amazonaws.supplychain#GalaxyPublicAPIGateway
Issue-Key: scn|GetInstance|4xx->4xx|$.message
Issue-Key: scn|CreateDataLakeNamespace|4xx->4xx|$.message
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# scn refusal messages are emulator-authored prose instead of AWS's not-found and validation wording

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInstance

## Description
TWO (2) net-new divergence clusters on AWS Supply Chain (service token `scn`, sdkId `SupplyChain`, endpointPrefix/sigv4Name `scn`; bootstrap crate `service-com-amazonaws-supplychain-galaxypublicapigateway` is RESOLVED, so these are fidelity findings). In both the HTTP status and the error class are already correct - only the `message` member diverges, and in both cases because the emulator composes its own prose instead of emitting the service's message for that error class. A fixer MUST close both causes before submitting.

Cause 1 of 2: ResourceNotFoundException message on a read of an absent instance (fixture `supplychain-instance-error-paths`, step `task#6`, operation GetInstance). AWS: `GetInstance` declares `ResourceNotFoundException` @httpError 404 with a single `message` member, documented as "Request references a resource which does not exist"; the recording shows the service answering 404 with the message `Instance not found.` - FIXED TEXT in its entirety, with NO variable part: the requested instanceId is NOT interpolated into it. Emulator: 404 (status and error class correct) with the message `instance <instanceId> does not exist` - emulator-authored prose that both differs from AWS's wording and splices the requested identifier in. Why: a client that distinguishes the service's not-found text from other 404s, or that surfaces the message to a user, gets text AWS never produces; and because the emulator interpolates the id, the message is not even stable for a fixed error class, so no exact-match assertion on it can hold. Fix: the shared ResourceNotFoundException constructor used by the instance read/update/delete handlers in services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/ must emit the fixed text `Instance not found.` for an absent instance rather than formatting the id into a sentence. The same wiring serves the 5 steps this cluster covers, so one edit closes them all.

Cause 2 of 2: ValidationException message template for a constraint-violating path label (fixture `supplychain-instance-error-paths`, step `task#7`, operation CreateDataLakeNamespace). AWS: `CreateDataLakeNamespace` is PUT /api/datalake/instance/{instanceId}/namespaces/{name} and its `name` member (@httpLabel, @required, DataLakeNamespaceName, @length 1..50, @pattern `^[a-z0-9_]+$`) is refused with the modelled `ValidationException` @httpError 400. The recording shows the service's standard constraint-violation template, whose FIXED TEXT is `1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` - note two things the emulator gets wrong about that template: AWS does NOT include a `Value '<supplied>'` slot for this member (the offending value is omitted, so the message's only variable parts are the member name taken from the request binding and the pattern taken from the model), and AWS prints the pattern UNANCHORED as `[a-z0-9_]+`, without the `^`/`$` the model's @pattern carries. Emulator: 400 with `1 validation error detected: Value 'Invalid-Name!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9_]+$` - it adds the `Value '...'` slot AWS omits and echoes the anchored model pattern. (The quoted `Invalid-Name!` is the request's value filling the emulator's extra slot; it is not part of the template.) Why: validation messages are how a caller tells which member it got wrong and what shape is expected; a message that carries an extra value slot and a differently-spelled pattern fails any exact-match assertion a client or test harness makes on AWS's template, and the anchored pattern leaks the emulator's internal regex spelling rather than the one the service publishes. Fix: the ValidationException message formatter shared by this service's input validation (services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/) - drop the `Value '<supplied>' ` segment for this member and strip the anchors when rendering the @pattern into the message.

No open issue covers any scn/SupplyChain divergence (list_existing_issues finds none for `scn` or `supplychain`), so neither cause is owned elsewhere; neither cluster carried a `stub_path` or `owned_by`. The `Instance`-shape divergences from the same replay are filed separately as the scn Instance-projection bundle.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=101, passed=14, mismatched=82, divergence_count=11, score_pct=13.9.

Cause 1 - supplychain-instance-error-paths#task#6, GetInstance, expected 404 / actual 404 (status and error class MATCH; 5 steps affected: also supplychain-instance-lifecycle#teardown#7, supplychain-list-instances-pagination#teardown#9, #teardown#10, supplychain-tagging-workflow#teardown#10):
  $.message: value_mismatch (expected 'Instance not found.', got 'instance 00000000-0000-0000-0000-000000000000 does not exist')
  Fixed text (whole message): "Instance not found."  Variable parts: NONE - the emulator's interpolated instanceId is not part of AWS's message.

Cause 2 - supplychain-instance-error-paths#task#7, CreateDataLakeNamespace, expected 400 / actual 400 (status and error class MATCH):
  $.message: value_mismatch
    expected: "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9_]+"
    actual:   "1 validation error detected: Value 'Invalid-Name!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9_]+$"
  Fixed text: "1 validation error detected: Value at '" + "' failed to satisfy constraint: Member must satisfy regular expression pattern: ".
  Variable parts: the member name ('name', from the request binding) and the pattern ([a-z0-9_]+, from the model, rendered WITHOUT anchors).
  Emulator's two extra deviations: an inserted "Value '<supplied>' " slot (filled here with the per-request value Invalid-Name!) and the anchored form ^[a-z0-9_]+$.

Model evidence (services/service-com-amazonaws-supplychain-galaxypublicapigateway/smithy-model/com-amazonaws-supplychain-galaxypublicapigateway.json):
- ResourceNotFoundException: errorKind client, httpError 404, members {message: String}, doc "Request references a resource which does not exist." Declared by GetInstance (GET /api/instance/{instanceId}).
- ValidationException: errorKind client, httpError 400, members {message: String}, doc "The input does not satisfy the constraints specified by an AWS service." Declared by CreateDataLakeNamespace.
- CreateDataLakeNamespace input `name`: target DataLakeNamespaceName, httpLabel true, required true, length {min 1, max 50}, pattern ^[a-z0-9_]+$; http PUT /api/datalake/instance/{instanceId}/namespaces/{name}, code 200.
- Service GalaxyPublicAPIGateway v2024-01-01, restJson1, awsService{sdkId SupplyChain, endpointPrefix scn}; ValidationException and ResourceNotFoundException are both service-common errors.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo (mainline) and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture `supplychain-instance-error-paths` step `task#6` - GET /api/instance/00000000-0000-0000-0000-000000000000 (any well-formed UUID no instance holds). Observe 404 with body {"message": "instance <id> does not exist"}; AWS's recorded body is {"message": "Instance not found."}. The same divergence reproduces on supplychain-instance-lifecycle#teardown#7, supplychain-list-instances-pagination#teardown#9 and #teardown#10, and supplychain-tagging-workflow#teardown#10, where the delete/read follows an actual deletion.
4. Cause 2: replay fixture `supplychain-instance-error-paths` step `task#7` - PUT /api/datalake/instance/{instanceId}/namespaces/Invalid-Name! (a namespace name violating @pattern ^[a-z0-9_]+$). Observe 400 ValidationException whose message carries the extra "Value 'Invalid-Name!' " slot and the anchored pattern ^[a-z0-9_]+$; AWS's recorded message omits the value slot and prints the pattern as [a-z0-9_]+.
5. Diff each response's $.message against the fixture's expected message; status and error class already match, so the message member is the whole divergence.
