Issue-Source: trace-sweep-divergence
Issue-Trace: storagegateway-tape-pool-create-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: storagegateway-tape-pool-create-validation-errors storagegateway-gateway-scoped-not-found-errors
Issue-Shape-Id: com.amazonaws.storagegateway#StorageGateway_20130630
Issue-Key: storagegateway|CreateTapePool|4xx->4xx|$.message
Issue-Key: storagegateway|CreateTapePool|4xx->4xx|$.Message
Issue-Key: storagegateway|CreateTapePool|4xx->4xx|$.__type
Issue-Key: storagegateway|DescribeGatewayInformation|4xx->4xx|$.message
Issue-Commit: 7eda33408fad28e5ce4a9dbc5d19eef9d00fdd67

# Storage Gateway error replies use non-AWS exception types and message wording

> _Note: this issue was identified against the code base at commit `7eda33408fad28e5ce4a9dbc5d19eef9d00fdd67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTapePool

## Description
Storage Gateway (awsJson1_1, API version 2013-06-30) returns the right HTTP status on failure but the wrong error payload: the emulator substitutes generic cross-service validation machinery for Storage Gateway's own InvalidGatewayRequestException wording, and reports a JSON deserialization failure as a modelled validation error. Three distinct causes, all in the Storage Gateway error path.

Cause 1 of 3 - CreateTapePool input validation returns the generic AWS validation-framework message instead of Storage Gateway's InvalidGatewayRequestException wording (fixture storagegateway-tape-pool-create-validation-errors, steps task#1, task#2, task#3, task#5).
AWS: Smithy: storagegateway#CreateTapePool declares PoolName (required, @length 1..100, @pattern ^[ -\.0-\[\]-~]*[!-\.0-\[\]-~][ -\.0-\[\]-~]*$) and StorageClass (required, enum TapeStorageClass = DEEP_ARCHIVE|GLACIER), and its only client error is InvalidGatewayRequestException (httpError 400, members `message` and `error`/StorageGatewayError{errorCode,errorDetails}). The recorded response is HTTP 400 whose `message` is the single aggregated sentence 'The following request fields contain invalid input: poolName, storageClass.' The fixed text of that message class is 'The following request fields contain invalid input: ' followed by a comma-separated list and a terminating '.'; the variable part is the list of offending input member names, rendered in the service's lower-camel spelling (poolName, storageClass, retentionLockType, retentionLockTimeInDays) and taken from the request - every invalid or missing field is named in ONE message, not one message per field. No ValidationException appears in the operation's modelled error set.
Emulator: answers HTTP 400 but with the generic validation-framework text "1 validation error detected: Value null at 'poolName' failed to satisfy constraint: Member must not be null" - a per-field message in a different wording, naming only the first offending field and using a different member spelling convention.
Why: a client that matches on Storage Gateway's message to discover which request fields it got wrong sees neither the expected sentence nor the full set of bad fields, so a caller that submitted several invalid fields re-submits with only one fixed and loops; log/alarm patterns and SDK-level message assertions written against AWS text never match.
Fix: the CreateTapePool input validation in the Storage Gateway service - replace the shared validation-message formatter with Storage Gateway's aggregated InvalidGatewayRequestException message builder that collects ALL invalid/missing member names and renders them as one sentence.
Traces: storagegateway-tape-pool-create-validation-errors#task#1 (representative), plus task#2, task#3, task#5 in the same fixture.

Cause 2 of 3 - a JSON body whose numeric member carries a string is reported as ValidationException instead of SerializationException (fixture storagegateway-tape-pool-create-validation-errors, step task#4).
AWS: Storage Gateway speaks awsJson1_1. A request whose JSON value cannot be coerced to the modelled type - here RetentionLockTimeInDays, modelled as an integer with @range 0..36500 - fails before input validation, in the protocol deserializer. The recorded response is HTTP 400 with `__type` = 'SerializationException' and a capitalised `Message` member whose text is '<value> can not be converted to an Integer'; the fixed text is ' can not be converted to an Integer' and the variable part is the offending JSON value taken from the request body (the recorded run happens to carry the literal STRING_VALUE). Note the member name is `Message`, not the `message` used by modelled Storage Gateway errors.
Emulator: answers HTTP 400 with `__type` = 'ValidationException' and no `Message` member at all, i.e. it routes a protocol deserialization failure into the modelled-validation path.
Why: SerializationException is a protocol framing error and ValidationException is not even in CreateTapePool's modelled error set, so an SDK cannot classify the failure, retry/error-mapping logic keyed on `__type` takes the wrong branch, and a client reading `Message` to learn which value was unparseable gets nothing.
Fix: the awsJson1_1 request deserializer for the Storage Gateway service - raise SerializationException (with the capitalised `Message` member) when a body value cannot be coerced to the modelled shape, before handing the input to operation validation.
Traces: storagegateway-tape-pool-create-validation-errors#task#4.

Cause 3 of 3 - the gateway-not-found error interpolates the gateway ARN into a message whose AWS text is fixed (fixture storagegateway-gateway-scoped-not-found-errors, step task#1).
AWS: Smithy: storagegateway#DescribeGatewayInformation takes GatewayARN (required, @length 50..500) and declares InvalidGatewayRequestException (httpError 400) as its only client error. For a gateway ARN that does not exist, the recorded response is HTTP 400 whose `message` is exactly 'The specified gateway was not found.' - the whole message is fixed text with NO variable part; the identity of the missing gateway is conveyed by the accompanying StorageGatewayError errorCode, not by the message.
Emulator: answers HTTP 400 but with 'The gateway <gatewayArn> was not found.', splicing the request's GatewayARN into the text.
Why: the message is part of the error contract; a client or test matching AWS's fixed sentence fails against the emulator, and the emulator's variant additionally makes the message vary per request so it cannot be matched as a constant at all.
Fix: the DescribeGatewayInformation handler's not-found path (and the shared gateway-lookup error constructor it uses) in the Storage Gateway service - emit the fixed InvalidGatewayRequestException message and carry the gateway identity in the `error` member instead.
Traces: storagegateway-gateway-scoped-not-found-errors#task#1.

Related but NOT covering this: open issue 20261109T163411Z-iam-storagegateway-storage-gateway-createtapepool-answers-notimplemented-so-tape-pool reports CreateTapePool answering NotImplemented from a behavioral check; it carries no wire replay key for these fixtures and does not describe the error-payload wording or the SerializationException classification. The replay shows CreateTapePool now reaching validation, so these three causes must be closed separately.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200): 3 divergence clusters, all service=storagegateway, 6 steps.

Cause 1 - CreateTapePool, expected 400 / actual 400:
  $.message: expected 'The following request fields contain invalid input: poolName, storageClass.'
               actual "1 validation error detected: Value null at 'poolName' failed to satisfy constraint: Member must not be null"
  cluster members: storagegateway-tape-pool-create-validation-errors#task#1, #task#2, #task#3, #task#5

Cause 2 - CreateTapePool, expected 400 / actual 400:
  $.__type: expected 'SerializationException', actual 'ValidationException'
  $.Message: expected 'STRING_VALUE can not be converted to an Integer', actual missing (None)
  cluster member: storagegateway-tape-pool-create-validation-errors#task#4

Cause 3 - DescribeGatewayInformation, expected 400 / actual 400:
  $.message: expected 'The specified gateway was not found.'
               actual 'The gateway arn:aws:storagegateway:<region>:<account>:gateway/<gatewayId> was not found.'
  cluster member: storagegateway-gateway-scoped-not-found-errors#task#1

Model basis: services/service-com-amazonaws-storagegateway-storagegateway-20130630/smithy-model - protocols ['awsJson1_1'], common_errors ['InternalServerError','InvalidGatewayRequestException']; CreateTapePool input PoolName (required), StorageClass (required enum DEEP_ARCHIVE|GLACIER), RetentionLockTimeInDays (integer @range 0..36500); DescribeGatewayInformation input GatewayARN (required).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST to the Storage Gateway endpoint with X-Amz-Target StorageGateway_20130630.CreateTapePool and a body omitting PoolName and StorageClass (fixture storagegateway-tape-pool-create-validation-errors, step task#1). Observe HTTP 400 with the generic '1 validation error detected: ...' message instead of 'The following request fields contain invalid input: poolName, storageClass.'
3. Cause 2: call CreateTapePool with RetentionLockTimeInDays set to a JSON string rather than a number (same fixture, step task#4). Observe __type 'ValidationException' with no 'Message' member instead of __type 'SerializationException' with Message '<value> can not be converted to an Integer'.
4. Cause 3: call DescribeGatewayInformation with a well-formed GatewayARN naming a gateway that does not exist (fixture storagegateway-gateway-scoped-not-found-errors, step task#1). Observe the message interpolating the ARN instead of the fixed text 'The specified gateway was not found.'
