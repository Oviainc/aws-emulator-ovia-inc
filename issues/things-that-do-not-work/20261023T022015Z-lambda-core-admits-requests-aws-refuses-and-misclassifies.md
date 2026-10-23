Issue-Source: trace-sweep-divergence
Issue-Trace: lambdacore-absent-connector-envelopes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdacore-absent-connector-envelopes lambdacore-create-validation-rejections lambdacore-identifier-and-list-validation
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda-core|UpdateNetworkConnector|4xx->4xx|$.message
Issue-Key: lambda-core|UpdateNetworkConnector|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda-core|UpdateNetworkConnector|4xx->4xx|status
Issue-Key: lambda-core|CreateNetworkConnector|4xx->2xx|$.Type
Issue-Key: lambda-core|CreateNetworkConnector|4xx->2xx|$.message
Issue-Key: lambda-core|CreateNetworkConnector|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda-core|CreateNetworkConnector|4xx->2xx|status
Issue-Key: lambda-core|CreateNetworkConnector|4xx->4xx|$.Type
Issue-Key: lambda-core|CreateNetworkConnector|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda-core|CreateNetworkConnector|4xx->4xx|status
Issue-Key: lambda-core|GetNetworkConnector|4xx->4xx|$.Type
Issue-Key: lambda-core|GetNetworkConnector|4xx->4xx|$.message
Issue-Key: lambda-core|GetNetworkConnector|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda-core|GetNetworkConnector|4xx->4xx|status
Issue-Related: 20261023T022015Z-lambda-core-rejection-messages-diverge-from-aws-not.md
Issue-Commit: 946f42f6a143eaa9f4f23faa899ea5f11b0d1603

# lambda-core admits requests AWS refuses and misclassifies identifier refusals across create, update and get

> _Note: this issue was identified against the code base at commit `946f42f6a143eaa9f4f23faa899ea5f11b0d1603`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetworkConnector

## Description
Five net-new divergences where the emulator returns the wrong HTTP status and the wrong error code because a refusal AWS performs is missing or is reached by the wrong decision path. Unlike message-only divergences these change the outcome a client sees.

Cause 1 of 5 — UpdateNetworkConnector does not enforce the required ClientToken before resolving the identifier.
AWS: Smithy (com.amazonaws.lambdacore) UpdateNetworkConnector input carries ClientToken -> ClientTokenString (@length 1..64); the recorded service refuses a request that omits it with HTTP 400 / InvalidParameterValueException (x-amzn-errortype: InvalidParameterValueException) and the request-derived message 'ClientToken is a required field' — i.e. request validation runs before resource resolution, so the missing-field refusal wins even when the target connector does not exist.
Emulator: resolves the identifier first and answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and no $.message at all.
Why: a client that forgot a required field is told the resource is missing; it retries against a different identifier instead of fixing its request, and SDK retry/error-classification logic branches on the wrong error.
Fix: UpdateNetworkConnector's input validation must run (and emit InvalidParameterValueException) ahead of the handler's identifier lookup.
Traces: lambdacore-absent-connector-envelopes#task#4.

Cause 2 of 5 — CreateNetworkConnector accepts a malformed security-group id and reports success.
AWS: Smithy CreateNetworkConnector.Configuration -> NetworkConnectorConfiguration is required and names EC2 resources; the operation models InvalidParameterValueException (httpError 400) with members Type and message. The recording shows AWS validating the referenced EC2 resources and refusing the create with HTTP 400, Type='User' and a message that reports the downstream EC2 failure (recorded example: 'Error occurred while DescribeSecurityGroups. EC2 Error Code: InvalidGroupId.Malformed. EC2 Error Message: Invalid id: "sg-0123456789abcdef0"'), plus an x-amzn-errortype header. Recorded behavior only for the exact message text; the status, the error class and the fact that the create is refused are the requirement.
Emulator: accepts the request and answers HTTP 202 (the operation's success code) with no error members.
Why: the caller believes a connector was created over a security group that cannot exist; the failure surfaces later (or never), and negative tests of EC2-reference validation pass against a service that validates nothing.
Fix: CreateNetworkConnector's configuration validation (EC2 resource reference check) in the operation's handler, raising InvalidParameterValueException.
Traces: lambdacore-create-validation-rejections#task#4.

Cause 3 of 5 — CreateNetworkConnector reports a name conflict instead of refusing a non-existent subnet.
AWS: same operation and the same InvalidParameterValueException (httpError 400) path; a Configuration naming a subnet that does not exist is refused 400 with Type='User' and the downstream EC2 report (recorded example: "Error occurred while DescribeSubnets. EC2 Error Code: InvalidSubnetID.NotFound. EC2 Error Message: The subnet ID 'subnet-0123456789abcdef0' does not exist"). ResourceConflictException (httpError 409) is modeled for duplicate names, not for unresolvable EC2 references.
Emulator: answers HTTP 409 with x-amzn-errortype: ResourceConflictException and 'A network connector named sim2real-probe-lc-reject-0001 already exists in this account and Region' — the connector created by the preceding step of the same fixture was persisted even though the request should have been refused, so the retry collides on the name.
Why: the caller is told its name is taken rather than that its subnet does not exist, and a ghost connector now occupies that name; 409 is also a different retry class from 400.
Fix: validate EC2 references in CreateNetworkConnector before any state is written, and keep the conflict check for genuine duplicate names.
Traces: lambdacore-create-validation-rejections#task#5.

Cause 4 of 5 — GetNetworkConnector treats a malformed identifier as not-found instead of invalid.
AWS: GetNetworkConnector.Identifier -> NetworkConnectorIdentifier accepts an id, a name, or a network-connector ARN, and the operation models both InvalidParameterValueException (400) and ResourceNotFoundException (404). An identifier that is not a well-formed network-connector identifier (recorded case: an s3 ARN) is refused with HTTP 400, Type='User' and the request-derived message 'Network connector arn invalid: <identifier>' — the syntax refusal precedes any lookup.
Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and no $.message.
Why: a client with a syntactically wrong identifier (wrong service ARN, pasted bucket ARN) is told the connector is absent, so it cannot distinguish a bad request from a deleted resource.
Fix: identifier parsing/validation in GetNetworkConnector ahead of the state lookup, raising InvalidParameterValueException.
Traces: lambdacore-identifier-and-list-validation#task#2.

Cause 5 of 5 — GetNetworkConnector on a cross-account ARN answers 404 instead of a 403 access refusal.
AWS: for a well-formed network-connector ARN whose account differs from the caller's, the recorded service refuses with HTTP 403 and the message 'Cross-account access is not allowed.' rather than reporting absence.
Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and 'The network connector <arn> does not exist in this account and Region'.
Why: the authorization boundary is not modeled at all — a caller probing another account's ARNs gets an existence-style answer, and tests that assert the cross-account denial pass for the wrong reason.
Fix: an account-ownership check on the resolved ARN in GetNetworkConnector (and the shared identifier resolver) that returns the 403 access refusal before the not-found path.
Traces: lambdacore-identifier-and-list-validation#task#3.

state_reset_failures was 0 for this replay, so none of these is an artifact of leaked state between fixtures; the stale connector in cause 3 is created inside the same fixture. No open issue carries these replay keys.

## Evidence
Cause 1 — lambdacore-absent-connector-envelopes#task#4, UpdateNetworkConnector: status expected 400, got 404; $.message missing (expected 'ClientToken is a required field', got None); $headers.x-amzn-errortype expected 'InvalidParameterValueException', got 'ResourceNotFoundException'.

Cause 2 — lambdacore-create-validation-rejections#task#4, CreateNetworkConnector: status expected 400, got 202; $.Type missing (expected 'User'); $.message missing (expected 'Error occurred while DescribeSecurityGroups. EC2 Error Code: InvalidGroupId.Malformed. EC2 Error Message: Invalid id: "sg-0123456789abcdef0"'); x-amzn-errortype absent.

Cause 3 — lambdacore-create-validation-rejections#task#5, CreateNetworkConnector: status expected 400, got 409; $.message expected "Error occurred while DescribeSubnets. EC2 Error Code: InvalidSubnetID.NotFound. EC2 Error Message: The subnet ID 'subnet-0123456789abcdef0' does not exist", got 'A network connector named sim2real-probe-lc-reject-0001 already exists in this account and Region'; x-amzn-errortype got 'ResourceConflictException'; $.Type missing (expected 'User').

Cause 4 — lambdacore-identifier-and-list-validation#task#2, GetNetworkConnector: status expected 400, got 404; $.message missing (expected 'Network connector arn invalid: network-connector:arn:aws:s3:::sim2real-probe-bucket'); x-amzn-errortype got 'ResourceNotFoundException'; $.Type missing (expected 'User').

Cause 5 — lambdacore-identifier-and-list-validation#task#3, GetNetworkConnector: status expected 403, got 404; $.Message expected 'Cross-account access is not allowed.', got 'The network connector arn:aws:lambda:us-west-2:111122223333:network-connector:nc-11111111-2222-3333-4444-555555555555 does not exist in this account and Region'; x-amzn-errortype got 'ResourceNotFoundException'.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: PUT /2026-04-04/network-connectors/<absent identifier> with a body that omits ClientToken (fixture lambdacore-absent-connector-envelopes, step task#4) — observe 404 ResourceNotFoundException instead of 400 InvalidParameterValueException 'ClientToken is a required field'.
3. Cause 2: POST /2026-04-04/network-connectors with a Configuration referencing security group 'sg-0123456789abcdef0' (fixture lambdacore-create-validation-rejections, step task#4) — observe 202 instead of 400.
4. Cause 3: in the same fixture, POST again with a Configuration referencing subnet 'subnet-0123456789abcdef0' (step task#5) — observe 409 ResourceConflictException on the name instead of 400 for the missing subnet.
5. Cause 4: GET /2026-04-04/network-connectors/network-connector:arn:aws:s3:::sim2real-probe-bucket (fixture lambdacore-identifier-and-list-validation, step task#2) — observe 404 instead of 400.
6. Cause 5: GET /2026-04-04/network-connectors/arn:aws:lambda:us-west-2:111122223333:network-connector:nc-11111111-2222-3333-4444-555555555555 (same fixture, step task#3) — observe 404 instead of 403 'Cross-account access is not allowed.'
