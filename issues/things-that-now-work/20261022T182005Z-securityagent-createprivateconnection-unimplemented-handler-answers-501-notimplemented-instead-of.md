Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: securityagent.create-private-connection-nonexistent-self-managed-resource-rejected securityagent.create-private-connection-missing-mode-rejected
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent.create-private-connection-nonexistent-self-managed-resource-rejected|run#7|AssertionError|assert code == "ResourceNotFoundException", f"unexpected code {code!r}"
Issue-Key: securityagent.create-private-connection-missing-mode-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 00577afa9aa0eead33f7dafc8adac2dd3659e1fc

# CreatePrivateConnection: unimplemented handler answers 501 NotImplemented instead of modeled errors, and empty-union rejection reports a generic deserializer message

> _Note: this issue was identified against the code base at commit `00577afa9aa0eead33f7dafc8adac2dd3659e1fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePrivateConnection

## Description
Two distinct root causes in securityagent's CreatePrivateConnection path. Both are grounded in failing behavioral checks; neither is covered by an ALREADY FILED/OWNED marker in the replay report (the auto-filed stub issues name CreateAgentSpace, CreateSecurityRequirementPack, ListSecurityRequirementPacks and CreateTargetDomain — not CreatePrivateConnection), and list_existing_issues reports no open securityagent issue at all.

Cause 1 of 2: CreatePrivateConnection is a scaffold stub, so every well-formed request is answered with HTTP 501 NotImplemented instead of the operation's modeled behavior/errors.
AWS: Smithy model (services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json) binds CreatePrivateConnection to POST /CreatePrivateConnection with success status 201 and models the errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). ResourceNotFoundException is documented as "The specified resource was not found. Verify that the resource identifier is correct and that the resource exists in the specified agent space or account." Recorded behavior: a request whose mode is selfManaged={resourceConfigurationId: <an ARN of a VPC Lattice resource configuration that does not exist} is rejected with error code ResourceNotFoundException (HTTP 404) and the connection is never persisted; a subsequent DescribePrivateConnection for that name also reports ResourceNotFoundException and ListPrivateConnections does not contain it. The recorded message, as an example, is "A resource referenced by private connection '<name>' was not found. Verify the resource configuration exists and try again."
Emulator: the call fails with error code NotImplemented (HTTP 501, "operation CreatePrivateConnection is scaffolded but not implemented in this emulator"). The handler body in src/ops/create_private_connection.rs still returns crate::scaffolded::Scaffolded::not_implemented("CreatePrivateConnection") and the file still carries the SCAFFOLDED, NOT IMPLEMENTED marker.
Why: a client cannot create a private connection at all, and cannot distinguish a bad/missing referenced resource from a server-side gap: the retry classification differs (501 is not a modeled, client-actionable error), the status code is 501 rather than 404, and the error code NotImplemented is not in the operation's modeled error set, so typed SDK error handling (catch ResourceNotFoundException) never matches.
Fix: implement the operation's handler and its reference-resolution/validation in services/service-com-amazonaws-securityagent-securityagent/src/ops/create_private_connection.rs — resolve the selfManaged resourceConfigurationId (and the serviceManaged path) against service state, return ResourceNotFoundException for an unresolvable reference without persisting anything, and otherwise persist and serialize the 201 output (name, type, status, plus the optional resourceGatewayId/hostAddress/vpcId/resourceConfigurationId/certificateExpiryTime/dnsResolution/tags). Remove the SCAFFOLDED marker once implemented.
Traces: securityagent.create-private-connection-nonexistent-self-managed-resource-rejected.

Cause 2 of 2: an empty `mode` union is refused by the generic union deserializer rather than by the operation's input validation, so the ValidationException message does not state the member and the alternatives it requires.
AWS: Smithy: CreatePrivateConnectionInput.mode targets the union PrivateConnectionMode (@required), whose only members are serviceManaged and selfManaged — "Specify either a service-managed or a self-managed mode." A request reaching the service with neither member set is refused with error code ValidationException (HTTP 400). The rule producing the message is known: it is a validation error built from the offending request member and its constraint — it names `mode` and both permitted alternatives. Recorded example text: "Private Connection mode must specify either serviceManaged or selfManaged".
Emulator: status and error code match (400 / ValidationException), but the message is "union 'PrivateConnectionMode' had no member set" — emitted by the shared deserialization layer, naming the internal Smithy shape instead of the request member `mode` and its two permitted alternatives.
Why: a client (or an operator reading a log) is told about an internal shape name rather than which input field to fix and what the valid choices are; tooling that surfaces the validation message to an end user shows a framework-internal string instead of the actionable field-level guidance AWS returns.
Fix: the operation's input validation for CreatePrivateConnection — refuse the empty union in/alongside the handler's validation path (src/ops/create_private_connection.rs, using crate::scaffolded::Scaffolded::validation or the modeled ValidationException), or have the shared union deserializer phrase the failure in terms of the containing member and its member names, so the message names `mode` and the serviceManaged/selfManaged alternatives.
Traces: securityagent.create-private-connection-missing-mode-rejected.

## Evidence
securityagent.create-private-connection-nonexistent-self-managed-resource-rejected [mismatch]:
  expected: error code ResourceNotFoundException (HTTP 404), connection not persisted
  actual:   AssertionError: unexpected code 'NotImplemented' — botocore ClientError (NotImplemented, HTTP 501) "operation CreatePrivateConnection is scaffolded but not implemented in this emulator"
  source:   services/service-com-amazonaws-securityagent-securityagent/src/ops/create_private_connection.rs:190 returns Scaffolded::not_implemented("CreatePrivateConnection"); SCAFFOLDED marker at line 4.

securityagent.create-private-connection-missing-mode-rejected [mismatch]:
  call:     create_private_connection(privateConnectionName=<name>, mode={}) with botocore parameter_validation=False so the malformed request actually reaches the service
  expected: ValidationException whose message names the member and its alternatives, e.g. 'Private Connection mode must specify either serviceManaged or selfManaged'
  actual:   ValidationException with message "union 'PrivateConnectionMode' had no member set"
  (the error code/status assertion passed; only the message rule diverges)

Smithy consulted: operation CreatePrivateConnection (POST /CreatePrivateConnection, success 201; errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400); shape PrivateConnectionMode (union; members serviceManaged -> ServiceManagedInput, selfManaged -> SelfManagedInput); shapes ResourceNotFoundException (client error, httpError 404) and ValidationException (client error, httpError 400, members message/fieldList).

## Steps to Reproduce
1. Boot the emulator and point a botocore securityagent client at it.
2. Cause 1: sts.get_caller_identity() for the account id, then call create_private_connection(privateConnectionName="behav-privconn-1", mode={"selfManaged": {"resourceConfigurationId": "arn:aws:vpc-lattice:<region>:<account>:resourceconfiguration/rcfg-<17 random hex>"}}) for a resource configuration that does not exist. Observe ClientError code NotImplemented / HTTP 501 instead of ResourceNotFoundException / HTTP 404. Replay check securityagent.create-private-connection-nonexistent-self-managed-resource-rejected.
3. Cause 2: build a second client with botocore Config(parameter_validation=False) and call create_private_connection(privateConnectionName="behav-privconn-2", mode={}). Observe ValidationException (400) whose message is "union 'PrivateConnectionMode' had no member set" rather than a message naming the `mode` member and its serviceManaged/selfManaged alternatives. Replay check securityagent.create-private-connection-missing-mode-rejected.
