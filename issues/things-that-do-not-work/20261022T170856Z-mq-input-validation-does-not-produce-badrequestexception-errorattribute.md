Issue-Source: trace-sweep-divergence
Issue-Trace: mq-error-paths
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-error-paths
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|CreateBroker|4xx->2xx|$.errorAttribute
Issue-Key: mq|CreateBroker|4xx->2xx|$.message
Issue-Key: mq|CreateBroker|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mq|CreateBroker|4xx->2xx|status
Issue-Key: mq|CreateConfiguration|4xx->4xx|$.errorAttribute
Issue-Key: mq|CreateConfiguration|4xx->4xx|$.message
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# mq input validation does not produce BadRequestException errorAttribute or reject missing users

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBroker

## Description
Two causes, both in Amazon MQ request validation: the emulator does not model MQ's BadRequestException payload, and it skips a required-input check on CreateBroker entirely. Both are reached by the mq-error-paths fixture.

Cause 1 of 2: CreateBroker accepts a request with no users and returns success.
AWS: Smithy (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json) binds CreateBroker to POST /v1/brokers with errors [BadRequestException(400), ConflictException(409), ForbiddenException(403), InternalServerErrorException(500), UnauthorizedException(401)]; CreateBrokerRequest.Users targets __listOfUser (jsonName "users"). The model marks Users as non-required, so the rule that a broker must be created with at least one user is recorded behavior only: the recording shows AWS rejecting a CreateBroker whose users list is absent/empty with HTTP 400, header x-amzn-errortype: BadRequestException, BadRequestException.errorAttribute = "users" (Smithy: BadRequestException.ErrorAttribute, jsonName "errorAttribute", "The attribute which caused the error") and a message naming the missing input (recorded example: "Specify at least one user.").
Emulator: returns HTTP 200 with a CreateBrokerResponse — no x-amzn-errortype header, no message, no errorAttribute. The broker is created.
Why: a client that omits users gets a broker id back and believes it provisioned a usable broker; SDK error handling for BadRequestException never fires, so misconfiguration surfaces later (or never) instead of at the call. Any test asserting that this call fails passes against the emulator and fails against AWS. The emulator also accumulates state AWS would never have created, so subsequent steps in the same session diverge too.
Fix: CreateBroker's input validation — add the at-least-one-user precondition ahead of broker creation in the operation handler, and raise BadRequestException (400) with errorAttribute set to the offending member rather than falling through to the success serializer.

Cause 2 of 2: CreateConfiguration's invalid-engineType rejection has the right status but the wrong error body shape — errorAttribute is omitted.
AWS: Smithy: CreateConfiguration input EngineType targets the enum EngineType with values [ACTIVEMQ, RABBITMQ], required; modeled error BadRequestException (httpError 400) whose members are ErrorAttribute, Message and ResourceShareErrors. For an engineType outside the enum set AWS answers 400 BadRequestException with errorAttribute = "engineType" — i.e. the error identifies the member that failed validation, which is a rule built from the request, not a fixed string. The recorded message is an example of MQ's enum-rejection wording: "Broker engine type [NOTAREALENGINE] is invalid. Valid values are: [ACTIVEMQ, RABBITMQ]".
Emulator: status 400 is correct, but $.errorAttribute is absent (None) and the body carries the framework's generic constraint-violation text ("1 validation error detected: Value 'NOTAREALENGINE' at 'engineType' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVEMQ, RABBITMQ]") rather than an MQ BadRequestException. The generic wording is evidence that the rejection comes from a shared protocol-level enum check rather than from the MQ error type; the message text alone is only an example, but the missing modeled member is a wire difference.
Why: clients and tools that branch on BadRequestException.errorAttribute to point a user at the bad field (the console and the CLI both surface it) see null, so the failure is unattributable to a member. Code that keys remediation off errorAttribute silently takes the wrong path.
Fix: MQ's error serialization / CreateConfiguration validation — route enum and required-member rejections through MQ's BadRequestException construction so ErrorAttribute is populated with the jsonName of the failing member, instead of letting the generic constraint validator render the response.

Not filed, for the record: the remaining three mq-error-paths divergences (DescribeBroker task#4, ListBrokers task#5, CreateUser task#7) match on status (404/400/404) and error type and differ only in message prose; the emulator's messages already interpolate the requested broker id / reject the bogus nextToken, and the model documents Message only as "The explanation of the error", so the exact wording is not a requirement.

## Evidence
Replay of fixture mq-error-paths (trace set simparity-wire-aws-v1):

Cause 1 — step task#8, CreateBroker: expected status 400, got 200. detail: "status: expected 400, got 200 / $.message: missing (expected 'Specify at least one user.', got None) / $.errorAttribute: missing (expected 'users', got None) / $headers.x-amzn-errortype: missing (expected 'BadRequestException', got None)". divergence_key service=mq|CreateBroker|status=400->200|keys=$.errorAttribute,$.message,$headers.x-amzn-errortype,status.

Cause 2 — step task#2, CreateConfiguration: expected status 400, got 400. detail: "$.message: value_mismatch (expected 'Broker engine type [NOTAREALENGINE] is invalid. Valid values are: [ACTIVEMQ, RABBITMQ]', got \"1 validation error detected: Value 'NOTAREALENGINE' at 'engineType' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVEMQ, RABBITMQ]\") / $.errorAttribute: missing (expected 'engineType', got None)". divergence_key service=mq|CreateConfiguration|status=400->400|keys=$.errorAttribute,$.message. The reportable part is the absent errorAttribute member; the message text is cited as an example only.

Model evidence: services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json — BadRequestException{ErrorAttribute(errorAttribute), Message(message), ResourceShareErrors} @error client httpError 400; EngineType enum values [ACTIVEMQ, RABBITMQ]; CreateBroker POST /v1/brokers with Users -> __listOfUser (jsonName "users").

Corpus context: 26 steps, 0 passed, 12 mismatched, 14 blocked by unimplemented operations (filed separately), 0 state-reset failures, emulator_up true.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from that checkout (health on /__admin/health, data listener accepting).
2. Replay wire fixture mq-error-paths from trace set simparity-wire-aws-v1.
3. Cause 1 (step task#8): POST /v1/brokers with a CreateBroker body that supplies brokerName, engineType, deploymentMode, hostInstanceType and publiclyAccessible but no users list (or an empty one). Expect HTTP 400 with x-amzn-errortype: BadRequestException and errorAttribute "users"; observe HTTP 200 with a brokerId and no error members.
4. Cause 2 (step task#2): POST /v1/configurations with engineType "NOTAREALENGINE" (outside the EngineType enum). Expect HTTP 400 BadRequestException with errorAttribute "engineType"; observe 400 with no errorAttribute member and the generic constraint-validator message.
5. Re-replay mq-error-paths#task#8 and mq-error-paths#task#2 to confirm both causes are closed.
