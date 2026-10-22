Issue-Source: trace-sweep-divergence
Issue-Trace: application-signals-slo-budget-report-batch
Issue-Step: teardown#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-signals-slo-budget-report-batch application-signals-slo-error-paths application-signals-slo-exclusion-windows application-signals-instrumentation-config-errors
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals|GetServiceLevelObjective|4xx->4xx|$.Message
Issue-Key: application-signals|GetServiceLevelObjective|4xx->4xx|$.ResourceId
Issue-Key: application-signals|GetServiceLevelObjective|4xx->4xx|$.ResourceType
Issue-Key: application-signals|DeleteServiceLevelObjective|4xx->4xx|$.Message
Issue-Key: application-signals|DeleteServiceLevelObjective|4xx->4xx|$.ResourceId
Issue-Key: application-signals|DeleteServiceLevelObjective|4xx->4xx|$.ResourceType
Issue-Key: application-signals|UpdateServiceLevelObjective|4xx->4xx|$.Message
Issue-Key: application-signals|UpdateServiceLevelObjective|4xx->4xx|$.ResourceId
Issue-Key: application-signals|UpdateServiceLevelObjective|4xx->4xx|$.ResourceType
Issue-Key: application-signals|CreateServiceLevelObjective|4xx->4xx|$.Message
Issue-Key: application-signals|CreateInstrumentationConfiguration|4xx->4xx|$.message
Issue-Key: application-signals|CreateServiceLevelObjective|4xx->4xx|$.message
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# application-signals error bodies identify an SLO by bare name instead of its ARN, and ValidationException messages are built wrongly

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetServiceLevelObjective

## Description
Six causes in this service's error-body construction. Statuses and error classes are correct throughout; the modeled members carry the wrong values. Two sub-faults: (a) SLO-identity errors report the bare SLO name where AWS reports the SLO ARN and the capitalised resource type, and (b) ValidationException messages are assembled with the wrong template.

Cause 1 of 6: GetServiceLevelObjective on a missing SLO.
AWS: Smithy GetServiceLevelObjective declares ResourceNotFoundException (404) whose required members are ResourceType, ResourceId and Message. The recording shows ResourceType = 'Slo', ResourceId = the full SLO ARN ('arn:aws:application-signals:<region>:<account>:slo/<name>') and a Message built from that ARN (example: "Slo with arn 'arn:aws:application-signals:us-west-2:000000000000:slo/sim2real-probe-...' does not exist."). The rule is that the error identifies the SLO by ARN and uses the type token 'Slo'.
Emulator: status 404 and the right error class, but ResourceId = the bare name, ResourceType = lowercase 'slo', and the Message names the bare name ('Service level objective <name> does not exist').
Why: ResourceId/ResourceType are modeled required members clients branch on. Code that reads ResourceId expecting a parseable ARN gets an un-namespaced name, and a ResourceType equality test against 'Slo' fails on 'slo'.
Fix: the ResourceNotFoundException construction in the SLO read handler (and the shared SLO-not-found helper it uses) — populate ResourceId from the SLO's ARN and ResourceType with 'Slo'.
Traces: application-signals-slo-budget-report-batch#teardown#5 (cluster also covers application-signals-slo-budget-report-batch#teardown#7, application-signals-slo-error-paths#task#1, application-signals-slo-exclusion-windows#teardown#8).

Cause 2 of 6: DeleteServiceLevelObjective on a missing SLO.
AWS: ResourceNotFoundException (404) with ResourceType = 'Slo' and ResourceId = the SLO ARN; the recorded message is the ARN-based "Slo with arn '...' does not exist." sentence.
Emulator: 404 with ResourceId = bare name, ResourceType = 'slo', Message naming the bare name.
Why: same as Cause 1, on the delete path — a teardown that keys off ResourceId cannot match the ARN it asked to delete.
Fix: the same not-found construction in the SLO delete handler.
Traces: application-signals-slo-error-paths#task#3.

Cause 3 of 6: UpdateServiceLevelObjective on a missing SLO.
AWS: ResourceNotFoundException (404) with ResourceType = 'Slo' and ResourceId = the SLO ARN. (The recorded message on this path is a different internal sentence — 'Work item with id: ... and namespace: slo does not exist' — whose construction rule is not documented, so treat the text as an example; the requirement is the status, the error class and the ResourceId/ResourceType values.)
Emulator: 404 with ResourceId = bare name and ResourceType = 'slo'.
Why: same as Cause 1, on the update path.
Fix: the same not-found construction in the SLO update handler.
Traces: application-signals-slo-error-paths#task#6.

Cause 4 of 6: CreateServiceLevelObjective on a name that already exists.
AWS: Smithy CreateServiceLevelObjective declares ConflictException (409) with a required Message. The recorded message is built from the request the same ARN-based way as the not-found path: "Slo with arn 'arn:aws:application-signals:us-west-2:000000000000:slo/<name>' already exists." — i.e. the SLO is named by its ARN.
Emulator: 409 ConflictException (correct) whose Message names the bare SLO name ('Service level objective <name> already exists').
Why: the conflict is reported, but a client that extracts the conflicting resource's ARN from the message to resolve or adopt it finds no ARN. It is the same bare-name-instead-of-ARN identity fault as Causes 1-3.
Fix: the duplicate-name rejection in the SLO create handler — build the ConflictException message from the SLO ARN.
Traces: application-signals-slo-error-paths#task#5.

Cause 5 of 6: CreateInstrumentationConfiguration pattern-validation message.
AWS: Smithy CreateInstrumentationConfiguration.Service is a required String with @length 1..255 and @pattern ^[A-Za-z0-9:/+=,.@_-]+$; the modeled rejection is ValidationException (400). The recorded message follows AWS's standard constraint template: "1 validation error detected: Value at 'service' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9:/+=,.@_-]+" — the rejected value is not echoed and the pattern is rendered without its ^...$ anchors. That construction rule is the requirement.
Emulator: right status (400) and class, but the message echoes the rejected value ("Value 'bad service name' at 'service' ...") and prints the anchored pattern ^[A-Za-z0-9:/+=,.@_-]+$.
Why: the message is the only machine-readable detail ValidationException carries (the shape has just `message`). Clients and assertions that parse it mis-read the member/pattern, and echoing the rejected value leaks caller input AWS deliberately withholds.
Fix: the shared constraint-violation message builder used by this service's input validation — omit the rejected value and strip the pattern anchors.
Traces: application-signals-instrumentation-config-errors#task#3.

Cause 6 of 6: CreateServiceLevelObjective name pattern-validation message — same builder, second operation.
AWS: Smithy CreateServiceLevelObjective.Name has @pattern ^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$; ValidationException (400). Recorded message: "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]" — again no echoed value, anchors stripped.
Emulator: 400 with "Value 'a' at 'name' ... pattern: ^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$".
Why: as Cause 5; confirms the defect is in the shared builder rather than one operation.
Fix: same constraint-violation message builder.
Traces: application-signals-slo-error-paths#task#2.

## Evidence
Cause 1 — application-signals-slo-budget-report-batch#teardown#5 GetServiceLevelObjective (404 both sides, cluster of 4): $.ResourceType expected 'Slo', got 'slo'; $.ResourceId expected 'arn:aws:application-signals:us-west-2:000000000000:slo/sim2real-probe-application-signals-slo-budget-a-8710ac1d', got 'sim2real-probe-application-signals-slo-budget-a-8710ac1d'; $.Message expected "Slo with arn '<arn>' does not exist.", got 'Service level objective <name> does not exist'.
Cause 2 — application-signals-slo-error-paths#task#3 DeleteServiceLevelObjective (404): identical ResourceId/ResourceType/Message divergence.
Cause 3 — application-signals-slo-error-paths#task#6 UpdateServiceLevelObjective (404): $.ResourceId expected the ARN, got the bare name; $.ResourceType expected 'Slo', got 'slo'.
Cause 4 — application-signals-slo-error-paths#task#5 CreateServiceLevelObjective (409 both sides): $.Message expected "Slo with arn '<arn>' already exists.", got 'Service level objective <name> already exists'.
Cause 5 — application-signals-instrumentation-config-errors#task#3 CreateInstrumentationConfiguration (400 both sides): $.message expected "1 validation error detected: Value at 'service' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9:/+=,.@_-]+", got "1 validation error detected: Value 'bad service name' at 'service' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9:/+=,.@_-]+$".
Cause 6 — application-signals-slo-error-paths#task#2 CreateServiceLevelObjective (400 both sides): same echo-the-value / anchored-pattern divergence on 'name'.
Smithy citations: ResourceNotFoundException (httpError 404) requires ResourceType, ResourceId, Message; ConflictException (409) requires Message; ValidationException (400) carries only `message`; CreateInstrumentationConfiguration.Service @pattern ^[A-Za-z0-9:/+=,.@_-]+$; CreateServiceLevelObjective.Name @pattern ^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$.

## Steps to Reproduce
1. Replay application-signals-slo-budget-report-batch step teardown#5: GET /slo/{Id} for an SLO that does not exist. The 404 body's ResourceId is the bare name and ResourceType is 'slo' instead of the SLO ARN and 'Slo'. Same on teardown#7, application-signals-slo-error-paths#task#1 and application-signals-slo-exclusion-windows#teardown#8.
2. Replay application-signals-slo-error-paths step task#3 (DELETE /slo/{Id}) and step task#6 (PATCH /slo/{Id}) against a missing SLO: identical ResourceId/ResourceType divergence.
3. Replay application-signals-slo-error-paths step task#5: CreateServiceLevelObjective with a name that already exists. The 409 message names the bare SLO name instead of its ARN.
4. Replay application-signals-instrumentation-config-errors step task#3: CreateInstrumentationConfiguration with Service = 'bad service name'. The 400 message echoes the rejected value and prints the anchored pattern.
5. Replay application-signals-slo-error-paths step task#2: CreateServiceLevelObjective with Name = 'a'. Same message-template divergence.
