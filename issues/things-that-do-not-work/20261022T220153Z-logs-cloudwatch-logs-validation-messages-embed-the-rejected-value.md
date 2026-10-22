Issue-Source: trace-sweep-divergence
Issue-Trace: logs-b2-delivery-source-destination-pipeline
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-b2-delivery-source-destination-pipeline logs-delivery-source-crud-lifecycle logs-s3-table-integration-sources
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|GetDelivery|4xx->4xx|$.message
Issue-Key: logs|DeleteDelivery|4xx->4xx|$.message
Issue-Key: logs|UpdateDeliveryConfiguration|4xx->4xx|$.message
Issue-Key: logs|PutDeliverySource|4xx->4xx|$.message
Issue-Key: logs|DisassociateSourceFromS3TableIntegration|4xx->4xx|$.message
Issue-Key: logs|ListSourcesForS3TableIntegration|4xx->4xx|$.message
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# CloudWatch Logs validation messages embed the rejected value and an anchored pattern

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDelivery

## Description
CloudWatch Logs renders constraint failures with a request-derived template: `N validation error detected: Value at '<member>' failed to satisfy constraint: <constraint text>`. The rule is known because the message is built from the request member and that member's modeled constraint, so the template itself is the requirement. The emulator's renderer deviates from that template in two systematic ways: it interpolates the rejected value (`Value '<value>' at ...`) where the service omits it, and it prints @pattern constraints with the model's `^`/`$` anchors where the service prints the pattern unanchored. Status (400) and error code (ValidationException) match on all six causes; only the message rendering diverges. This bundle has 6 causes; all 6 must be closed.

Cause 1 of 6 -- GetDelivery, `id` pattern violation.
AWS: Smithy -- GetDelivery.id targets DeliveryId, required, @length 1..64, @pattern ^[0-9A-Za-z]+$; error ValidationException (client, 400). The recorded message names the member and its pattern constraint and does NOT include the submitted value.
Emulator: same 400/ValidationException, but the message interpolates the submitted delivery id into the `Value '<value>' at 'id'` slot.
Why: a client that surfaces or log-scrapes the refusal reason now echoes caller-supplied input back; and anything matching on the service's template (support tooling, error-mapping tests) fails to match. Echoing the submitted value also leaks request content into logs the service deliberately keeps value-free.
Fix: the shared validation-error message renderer used by the GetDelivery input validation -- drop the value interpolation.
Traces: logs-b2-delivery-source-destination-pipeline#task#15.

Cause 2 of 6 -- DeleteDelivery, `id` pattern violation.
AWS: Smithy -- DeleteDelivery.id targets DeliveryId, required, @length 1..64, @pattern ^[0-9A-Za-z]+$; ValidationException modeled. Recorded message omits the value.
Emulator: message interpolates the submitted id.
Why: as cause 1.
Fix: same renderer, reached from DeleteDelivery input validation.
Traces: logs-b2-delivery-source-destination-pipeline#task#16.

Cause 3 of 6 -- UpdateDeliveryConfiguration, `id` pattern violation.
AWS: Smithy -- UpdateDeliveryConfiguration.id targets DeliveryId, required, @length 1..64, @pattern ^[0-9A-Za-z]+$; ValidationException modeled. Recorded message omits the value.
Emulator: message interpolates the submitted id.
Why: as cause 1.
Fix: same renderer, reached from UpdateDeliveryConfiguration input validation.
Traces: logs-b2-delivery-source-destination-pipeline#task#17.

Cause 4 of 6 -- PutDeliverySource, `name` pattern violation: value interpolated AND pattern anchored.
AWS: Smithy -- PutDeliverySource.name targets DeliverySourceName, required, @length 1..60, @pattern ^[\w-]*$; ValidationException modeled. The recorded message omits the value and renders the pattern WITHOUT the leading `^` and trailing `$` (i.e. `[\w-]*`).
Emulator: message interpolates the value and prints the anchored form `^[\w-]*$`.
Why: this is the second half of the defect -- the emulator reproduces the raw @pattern string instead of the service's unanchored rendering, so even after the value interpolation is removed the message still diverges on every pattern-constrained member.
Fix: same renderer -- strip anchors when formatting a @pattern constraint into the message.
Traces: logs-delivery-source-crud-lifecycle#task#5.

Cause 5 of 6 -- DisassociateSourceFromS3TableIntegration, `identifier` length violation.
AWS: Smithy -- DisassociateSourceFromS3TableIntegration.identifier targets S3TableIntegrationSourceIdentifier, required, @length 1..2048; ValidationException modeled. Recorded message states the minimum-length constraint for the member with no value interpolated.
Emulator: interpolates the (empty) value into the message.
Why: as cause 1; shows the renderer interpolates for @length constraints too, not only @pattern.
Fix: same renderer, reached from DisassociateSourceFromS3TableIntegration input validation.
Traces: logs-s3-table-integration-sources#task#2.

Cause 6 of 6 -- ListSourcesForS3TableIntegration, `maxResults` range violation.
AWS: Smithy -- ListSourcesForS3TableIntegration.maxResults targets ListSourcesForS3TableIntegrationMaxResults @range min 1 max 100; ValidationException modeled. Recorded message states the maximum-value constraint for the member with no value interpolated.
Emulator: interpolates the submitted number into the message.
Why: as cause 1; shows the renderer interpolates for @range constraints too.
Fix: same renderer, reached from ListSourcesForS3TableIntegration input validation.
Traces: logs-s3-table-integration-sources#task#3.

## Evidence
All six steps agree on HTTP 400 and on __type ValidationException; $.message is the only diverging field.
- GetDelivery / DeleteDelivery / UpdateDeliveryConfiguration (member 'id'): expected "1 validation error detected: Value at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z]+$"; got the same text with "Value '<submitted delivery id>' at 'id'".
- PutDeliverySource (member 'name'): expected "... Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]*"; got "... Value '<submitted name>' at 'name' ... pattern: ^[\w-]*$" -- value interpolated and the pattern anchored.
- DisassociateSourceFromS3TableIntegration (member 'identifier'): expected "... Value at 'identifier' failed to satisfy constraint: Member must have length greater than or equal to 1"; got the same with "Value '' at 'identifier'".
- ListSourcesForS3TableIntegration (member 'maxResults'): expected "... Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"; got the same with "Value '<submitted number>' at 'maxResults'".
Smithy citations: DeliveryId @length 1..64 @pattern ^[0-9A-Za-z]+$; DeliverySourceName @length 1..60 @pattern ^[\w-]*$; S3TableIntegrationSourceIdentifier @length 1..2048; ListSourcesForS3TableIntegrationMaxResults @range 1..100; all six operations model ValidationException (client, httpError 400).
The quoted strings are examples of the template; the requirement is the template (member name + modeled constraint, no value, unanchored pattern), not the recorded text.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay logs-b2-delivery-source-destination-pipeline steps task#15, task#16 and task#17 (GetDelivery / DeleteDelivery / UpdateDeliveryConfiguration with a non-alphanumeric delivery id) and read $.message.
3. Replay logs-delivery-source-crud-lifecycle#task#5 (PutDeliverySource with a name containing a space and a slash) and read $.message -- note both the interpolated value and the anchored pattern.
4. Replay logs-s3-table-integration-sources#task#2 (empty identifier) and #task#3 (maxResults=101) and read $.message.
5. Every message contains a "Value '<submitted value>' at ..." clause the recorded service response omits; the PutDeliverySource message additionally renders the pattern anchored.
