Issue-Source: trace-sweep-divergence
Issue-Trace: glue-integration-property-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-integration-property-error-paths glue-integration-resource-and-table-properties-errors glue-integration-resource-property-round-trip glue-integration-resource-property-lifecycle glue-integration-table-properties-round-trip
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CreateIntegrationResourceProperty|4xx->4xx|$.Message
Issue-Key: glue|CreateIntegrationResourceProperty|4xx->4xx|$.__type
Issue-Key: glue|GetIntegrationResourceProperty|4xx->4xx|$.Message
Issue-Key: glue|GetIntegrationResourceProperty|4xx->4xx|$.__type
Issue-Key: glue|CreateIntegrationResourceProperty|2xx->2xx|$.ResourcePropertyArn
Issue-Key: glue|GetIntegrationResourceProperty|2xx->2xx|$.ResourcePropertyArn
Issue-Key: glue|CreateIntegrationTableProperties|4xx->4xx|$.Message
Issue-Key: glue|CreateIntegrationTableProperties|4xx->4xx|$.__type
Issue-Key: glue|GetIntegrationTableProperties|4xx->4xx|$.Message
Issue-Key: glue|GetIntegrationTableProperties|4xx->4xx|$.__type
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue integration resource and table property errors and ARNs diverge from AWS

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegrationResourceProperty

## Description
This issue bundles 7 net-new causes in the zero-ETL integration-property family. Shared fault: the emulator validates these requests with a generic ARN/resource check that fires before (and instead of) the operation's own rules, so the error code is wrong in four causes and the message in all of them; and the ARN it mints for a resource property is keyed by an internal digest rather than by the target resource. HIGH because the error codes are wrong.

Cause 1 of 7: CreateIntegrationResourceProperty carrying both source and target processing properties.
AWS: Smithy: the operation's errors include ValidationException and EntityNotFoundException. Supplying SourceProcessingProperties and TargetProcessingProperties in one request is a validation failure: __type ValidationException, Message the fixed text 'Both source processing properties and target processing properties cannot be provided in the same request.'
Emulator: __type EntityNotFoundException with Message 'Entity not found for resource <resourceArn>' — it resolves the target first and never reaches the mutual-exclusion rule.
Why: a client fixing its request body is told the resource is missing, so it creates resources instead of correcting the payload.
Fix: CreateIntegrationResourceProperty validation ordering (apply the mutual-exclusion rule before resolving ResourceArn).
Traces: glue-integration-property-error-paths#task#3 (also glue-integration-resource-and-table-properties-errors#task#1, glue-integration-resource-property-round-trip#task#1).

Cause 2 of 7: GetIntegrationResourceProperty for a connection ARN that does not exist.
AWS: __type EntityNotFoundException with Message following the template 'Failed to find the resource property with arn <resourceArn> in the table' — fixed text with the requested ResourceArn substituted.
Emulator: right code, Message 'The target resource <resourceArn> is not found.'
Why: message assertions fail.
Fix: GetIntegrationResourceProperty not-found message.
Traces: glue-integration-property-error-paths#task#1.

Cause 3 of 7: GetIntegrationResourceProperty for a table ARN (an unsupported resource kind).
AWS: __type EntityNotFoundException, Message template 'The target resource <resourceArn> is not found.'
Emulator: __type InvalidInputException, Message 'ResourceArn "<arn>" is not the ARN of a Glue connection or a Glue database' — the generic ARN-kind check replaces the not-found classification.
Why: the client sees a permanent validation error where AWS reports a missing resource (retryable after creation).
Fix: GetIntegrationResourceProperty error classification (resolve the ARN, then refuse with EntityNotFoundException).
Traces: glue-integration-resource-and-table-properties-errors#task#2 (also glue-integration-resource-property-round-trip#task#3).

Cause 4 of 7: CreateIntegrationResourceProperty returns a digest-keyed ResourcePropertyArn.
AWS: ResourcePropertyArn is derived from the target resource: 'arn:aws:glue:<region>:<accountId>:integrationresourceproperty/<resourceKind>/<resourceName>' — the kind and name of the ResourceArn the property was created for.
Emulator: returns 'arn:aws:glue:<region>:<accountId>:integrationresourceproperty/<32-char hex digest>' — an internal id instead of the resource path.
Why: a client that constructs or parses this ARN (to correlate a property with its database/connection) cannot.
Fix: CreateIntegrationResourceProperty handler's ARN composition.
Traces: glue-integration-resource-property-lifecycle#task#4.

Cause 5 of 7: GetIntegrationResourceProperty returns the same digest-keyed ARN.
AWS: as cause 4.
Emulator: same digest form.
Why: as cause 4.
Fix: same ARN composition, applied in the read path.
Traces: glue-integration-resource-property-lifecycle#task#5.

Cause 6 of 7: CreateIntegrationTableProperties with an invalid target table name.
AWS: __type ValidationException, Message the fixed text 'Target Table Name contains invalid characters.'
Emulator: __type InvalidInputException, Message 'ResourceArn "<arn>" is not the ARN of a Glue connection, a Glue database or a Glue Data Catalog table' — the generic ARN-kind check again pre-empts the operation's own rule (the operation legitimately takes a Redshift Serverless namespace ARN as its target).
Why: the integration cannot be configured at all, and the reported fault names the wrong field.
Fix: CreateIntegrationTableProperties validation (accept the non-Glue target ARNs this operation takes; validate the table name).
Traces: glue-integration-table-properties-round-trip#task#1.

Cause 7 of 7: GetIntegrationTableProperties with an unresolvable resource ARN.
AWS: __type ValidationException, Message template 'Encountered invalid resource arn <resourceArn>'.
Emulator: __type InvalidInputException with the same generic ARN-kind message.
Why: as cause 6.
Fix: GetIntegrationTableProperties validation/classification.
Traces: glue-integration-table-properties-round-trip#task#3.

## Evidence
Replay of simparity-wire-aws-v1:
1. CreateIntegrationResourceProperty 400/400: $.__type expected 'ValidationException' got 'EntityNotFoundException'; $.Message expected 'Both source processing properties and target processing properties cannot be provided in the same request.' got 'Entity not found for resource arn:aws:glue:us-west-2:000000000000:connection/...'
2. GetIntegrationResourceProperty 400/400: $.Message expected 'Failed to find the resource property with arn arn:aws:glue:...:connection/... in the table' got 'The target resource arn:aws:glue:...:connection/... is not found.'
3. GetIntegrationResourceProperty 400/400: $.__type expected 'EntityNotFoundException' got 'InvalidInputException'; $.Message expected 'The target resource arn:aws:glue:...:table/<db>/<table> is not found.' got 'ResourceArn "..." is not the ARN of a Glue connection or a Glue database'.
4. CreateIntegrationResourceProperty 200/200: $.ResourcePropertyArn expected 'arn:aws:glue:us-west-2:000000000000:integrationresourceproperty/database/<dbName>' got 'arn:aws:glue:us-west-2:000000000000:integrationresourceproperty/789899651a0814b1da318ba57c4f3514'.
5. GetIntegrationResourceProperty 200/200: same ResourcePropertyArn divergence.
6. CreateIntegrationTableProperties 400/400: $.__type expected 'ValidationException' got 'InvalidInputException'; $.Message expected 'Target Table Name contains invalid characters.' got 'ResourceArn "arn:aws:redshift-serverless:...:namespace/..." is not the ARN of a Glue connection, a Glue database or a Glue Data Catalog table'.
7. GetIntegrationTableProperties 400/400: $.__type expected 'ValidationException' got 'InvalidInputException'; $.Message expected 'Encountered invalid resource arn arn:aws:redshift-serverless:...:namespace/...' got the same generic ARN-kind message.
(ARNs, names and the digest above are that run's values.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Re-replay: glue-integration-property-error-paths#task#3, glue-integration-property-error-paths#task#1, glue-integration-resource-and-table-properties-errors#task#2, glue-integration-resource-property-lifecycle#task#4, glue-integration-resource-property-lifecycle#task#5, glue-integration-table-properties-round-trip#task#1, glue-integration-table-properties-round-trip#task#3.
3. Compare $.__type, $.Message and $.ResourcePropertyArn with the AWS behavior quoted per cause.
