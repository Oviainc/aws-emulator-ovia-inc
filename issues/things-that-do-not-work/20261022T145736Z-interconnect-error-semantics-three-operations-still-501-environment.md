Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: interconnect.accept-connection-proposal-unknown-key-rejected interconnect.describe-connection-proposal-unknown-key-rejected interconnect.tag-resource-cross-account-arn-rejected interconnect.tag-resource-nonexistent-connection-rejected interconnect.get-environment-nonexistent-id-rejected interconnect.create-connection-nonexistent-environment-rejected interconnect.get-connection-nonexistent-id-rejected interconnect.delete-connection-nonexistent-id-rejected interconnect.list-connections-max-results-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.interconnect#Interconnect
Issue-Key: interconnect.accept-connection-proposal-unknown-key-rejected|run#5|AssertionError|assert code == "InterconnectValidationException", f"got {code!r}"
Issue-Key: interconnect.describe-connection-proposal-unknown-key-rejected|run#3|AssertionError|assert code == "InterconnectValidationException", f"expected InterconnectValidationException, got {code!r}"
Issue-Key: interconnect.tag-resource-cross-account-arn-rejected|run#7|AssertionError|assert tag_code == "InterconnectValidationException", f"got {tag_code!r}"
Issue-Key: interconnect.tag-resource-nonexistent-connection-rejected|run#8|AssertionError|assert tag_code == "InterconnectClientException", f"got {tag_code!r}"
Issue-Key: interconnect.get-environment-nonexistent-id-rejected|run#4|AssertionError|assert code == "InterconnectClientException", f"got {code!r}"
Issue-Key: interconnect.create-connection-nonexistent-environment-rejected|run#10|AssertionError|assert bad_code == "InterconnectClientException", f"got {bad_code!r}"
Issue-Key: interconnect.get-connection-nonexistent-id-rejected|run#6|AssertionError|assert message == expected, f"got {message!r}"
Issue-Key: interconnect.delete-connection-nonexistent-id-rejected|run#6|AssertionError|assert message == expected, f"got {message!r}"
Issue-Key: interconnect.list-connections-max-results-out-of-range-rejected|run#4|AssertionError|assert too_high_message == (
Issue-Commit: 89e1d3dd4cea8aa37cb69d127fa115133cff4722

# Interconnect error semantics: three operations still 501, Environment-not-found misclassified as ResourceNotFoundException, and non-AWS not-found/validation message formats

> _Note: this issue was identified against the code base at commit `89e1d3dd4cea8aa37cb69d127fa115133cff4722`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptConnectionProposal, DescribeConnectionProposal, TagResource, GetEnvironment, CreateConnection, GetConnection, DeleteConnection, ListConnections

## Description
Six distinct root causes in the Interconnect emulator, all surfaced by the behavioral error-semantics checks. They are numbered Cause 1 of 6 .. Cause 6 of 6; each must be closed before this issue is submitted.

=== Cause 1 of 6: AcceptConnectionProposal is a scaffold stub (501 NotImplemented) ===
AWS: Smithy model com-amazonaws-interconnect-interconnect.json binds AcceptConnectionProposal with input members attachPoint (required, AttachPoint) and activationKey (required, ActivationKey @length 1..2048, @sensitive), and declares the error set AccessDeniedException(403), InterconnectClientException(400), InterconnectServerException(500), InterconnectValidationException(400), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429). Recorded behavior: an activationKey that was never issued by a real proposal and is not valid base64 is rejected with error code InterconnectValidationException (HTTP 400); the recorded message, as an example, is "Unable to process request: The provided activation key is not valid base64". The requirement is the status/code pair plus the rule that a syntactically undecodable activationKey is a validation rejection, not a 501.
Emulator: returns error code NotImplemented (scaffold 501) for every AcceptConnectionProposal call.
Why: a client that handles InterconnectValidationException to surface "bad activation key" to the user instead sees an unmodeled NotImplemented/501, which SDK retry and error-mapping layers treat as a server-side/unsupported condition; no negative-path handling can be exercised against the emulator.
Fix: the AcceptConnectionProposal handler — implement activationKey decoding/lookup and return the modeled InterconnectValidationException for an undecodable key, and InterconnectClientException/ResourceNotFoundException for a well-formed but unknown key.
Traces: interconnect.accept-connection-proposal-unknown-key-rejected.

=== Cause 2 of 6: DescribeConnectionProposal is a scaffold stub (501 NotImplemented) ===
AWS: Smithy: DescribeConnectionProposal input activationKey (required, @length 1..2048, @sensitive), @readonly, same modeled error set including InterconnectValidationException(400). Recorded behavior: an activationKey that is not valid base64 is rejected with code InterconnectValidationException (HTTP 400).
Emulator: returns code NotImplemented (501).
Why: the proposal-describe read path cannot be exercised at all, and the negative class a client branches on (validation vs not-found) is indistinguishable from "operation missing".
Fix: the DescribeConnectionProposal handler — implement the read and its validation rejection.
Traces: interconnect.describe-connection-proposal-unknown-key-rejected.

=== Cause 3 of 6: TagResource is a scaffold stub (501 NotImplemented) ===
AWS: Smithy: TagResource input arn (required, AmazonResourceName @length 59..150, @pattern ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$) and tags (required, TagMap); input references ConnectionResource by arn; modeled errors include InterconnectValidationException(400), InterconnectClientException(400), ResourceNotFoundException(404). Recorded behavior: an ARN whose account id is not the caller's is rejected with code InterconnectValidationException, and an ARN that is well-formed and in-account but names no existing Connection is rejected with code InterconnectClientException. Those two codes, and the fact that the two negative classes are DIFFERENT codes, are the requirement.
Emulator: both calls return code NotImplemented (501); the handler body is `Err(crate::scaffolded::Scaffolded::not_implemented("TagResource"))` and the file still carries the SCAFFOLDED marker.
Why: tagging is unusable, and a client cannot distinguish "wrong account ARN" from "no such connection" — both of which it must report differently — because neither code is produced.
Fix: the TagResource handler (plus the connection/tag state it writes) — resolve the ARN against the caller's account and the connection store and return the two distinct modeled errors above.
Traces: interconnect.tag-resource-cross-account-arn-rejected, interconnect.tag-resource-nonexistent-connection-rejected.

=== Cause 4 of 6: an unknown Environment id is answered ResourceNotFoundException(404) instead of InterconnectClientException(400) ===
AWS: Smithy: GetEnvironment input id (required, EnvironmentId @length 1..64) and CreateConnection input environmentId (required, EnvironmentId) both model InterconnectClientException(400) and ResourceNotFoundException(404). Recorded behavior: for an id that corresponds to no Environment, BOTH GetEnvironment and CreateConnection fail with error code InterconnectClientException (HTTP 400) — not ResourceNotFoundException — with the message built from the request: "Unable to process request: This environment model with id <environmentId> could not be found". The requirement is the code/status and that the message names the supplied environment id in that form; the exact id in any recording is an example.
Emulator: both paths return ResourceNotFoundException. GetEnvironment's handler constructs ResourceNotFoundException with message "no environment with id '<id>'"; CreateConnection's environment lookup does the same.
Why: a client branching on InterconnectClientException for "unknown environment" (and on ResourceNotFoundException only for connection-scoped lookups) takes the wrong branch, and the HTTP status differs (400 vs 404), so status-based retry/alarm classification diverges too.
Fix: the GetEnvironment handler and the CreateConnection handler's environment-resolution step — classify an unresolvable environmentId as InterconnectClientException and build the message from the requested id. Citations: services/service-com-amazonaws-interconnect-interconnect/src/ops/get_environment.rs lines 143-151.
Traces: interconnect.get-environment-nonexistent-id-rejected, interconnect.create-connection-nonexistent-environment-rejected.

=== Cause 5 of 6: connection-not-found message is not AWS's request-derived form ===
AWS: Smithy: GetConnection and DeleteConnection both take identifier (required, ConnectionId @pattern ^(mcc|lmcc)-[a-z0-9]{8}$) and model ResourceNotFoundException(404). Recorded behavior: for a pattern-valid but nonexistent id, both return code ResourceNotFoundException (which the emulator gets right) with a message built from the request: "Unable to process request: Unable to retrieve connection <identifier>, as it does not exist". The rule is known (fixed prefix/suffix around the supplied identifier), so the FORM is the requirement; the particular id in a recording is an example.
Emulator: message is "no connection with identifier '<identifier>'" for both operations (see get_connection.rs lines 153-163), i.e. the right code with a message that does not follow AWS's "Unable to process request: ..." construction.
Why: clients and operators that parse or log the AWS wording (and tests asserting on it) see a different string for an otherwise correctly classified failure; the "Unable to process request:" prefix is the service-wide envelope AWS puts on these rejections and is absent everywhere in this emulator.
Fix: the shared not-found message construction used by the GetConnection and DeleteConnection handlers.
Traces: interconnect.get-connection-nonexistent-id-rejected, interconnect.delete-connection-nonexistent-id-rejected.

=== Cause 6 of 6: ListConnections maxResults range rejection embeds the offending value in the validation message ===
AWS: Smithy: ListConnections.maxResults targets MaxResults with @range min 1, max 100. Recorded behavior: maxResults=101 is rejected with code ValidationException (which the emulator gets right) and message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"; maxResights below the floor (maxResults=0, sent with client-side parameter validation disabled) is rejected with code ValidationException and "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1". The rule is known — the message is generated from the member name and the violated @range bound and does NOT echo the submitted value for this member.
Emulator: emits "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100" — the same rule plus the rejected value spliced in. Because the check fails on the ceiling assertion, the floor (maxResults=0) path was never reached and is unverified; fix and re-check both bounds.
Why: callers that match AWS's generated validation text (or compare it across a real/emulated pair) diverge on every range violation of this service, and the extra value changes the message for a member AWS reports without one.
Fix: the request-validation layer that renders @range violations for this service's input shapes (the generated validation that runs before ListConnections' handler).
Traces: interconnect.list-connections-max-results-out-of-range-rejected.

## Evidence
Behavioral suite: 0/15 passed for interconnect (9 mismatched, 6 errored). Per failing check, expected vs actual:

- accept-connection-proposal-unknown-key-rejected: expected code InterconnectValidationException; got 'NotImplemented'. (AssertionError: got 'NotImplemented')
- describe-connection-proposal-unknown-key-rejected: expected InterconnectValidationException; got 'NotImplemented'.
- tag-resource-cross-account-arn-rejected: expected InterconnectValidationException; got 'NotImplemented'.
- tag-resource-nonexistent-connection-rejected: expected InterconnectClientException; got 'NotImplemented'.
- get-environment-nonexistent-id-rejected: expected code InterconnectClientException (message "Unable to process request: This environment model with id <id> could not be found"); got 'ResourceNotFoundException'.
- create-connection-nonexistent-environment-rejected: after a positive control proving GetEnvironment resolves a real environmentId, CreateConnection with an unknown environmentId expected InterconnectClientException; got 'ResourceNotFoundException'.
- get-connection-nonexistent-id-rejected: code ResourceNotFoundException matched; expected message "Unable to process request: Unable to retrieve connection mcc-00000000, as it does not exist"; got "no connection with identifier 'mcc-00000000'".
- delete-connection-nonexistent-id-rejected: code ResourceNotFoundException matched; expected message "Unable to process request: Unable to retrieve connection lmcc-00000000, as it does not exist"; got "no connection with identifier 'lmcc-00000000'".
- list-connections-max-results-out-of-range-rejected: code ValidationException matched; expected message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"; got "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".

Source corroboration in the checkout:
- services/service-com-amazonaws-interconnect-interconnect/src/ops/tag_resource.rs lines 3-6 (SCAFFOLDED marker) and line 143 (`Err(crate::scaffolded::Scaffolded::not_implemented("TagResource"))`).
- services/service-com-amazonaws-interconnect-interconnect/src/ops/get_environment.rs lines 143-151: unknown id -> ResourceNotFoundException { message: "no environment with id '<id>'" }.
- services/service-com-amazonaws-interconnect-interconnect/src/ops/get_connection.rs lines 153-163: unknown identifier -> ResourceNotFoundException { message: "no connection with identifier '<id>'" }.

Model evidence (smithy_query on services/service-com-amazonaws-interconnect-interconnect/smithy-model/com-amazonaws-interconnect-interconnect.json): service Interconnect, protocols awsJson1_0/rpcv2Cbor; common errors AccessDeniedException, InterconnectClientException, InterconnectServerException, InterconnectValidationException, ResourceNotFoundException, ServiceQuotaExceededException, ThrottlingException; ListConnections.maxResults @range 1..100; TagResource.arn @pattern ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$; GetConnection/DeleteConnection.identifier @pattern ^(mcc|lmcc)-[a-z0-9]{8}$.

## Steps to Reproduce
Boot the emulator and point a botocore/AWS SDK interconnect client at it, then:

1. Cause 1: ic.accept_connection_proposal(attachPoint={"directConnectGateway": "12345678-1234-1234-1234-123456789012"}, activationKey="sim2real-behav-unknown-activation-key-0000000000") -> expect ClientError code InterconnectValidationException (400); observe 'NotImplemented'.
2. Cause 2: ic.describe_connection_proposal(activationKey="<not-base64>") -> expect InterconnectValidationException; observe 'NotImplemented'.
3. Cause 3a: ic.tag_resource(arn="arn:aws:interconnect:<region>:<other-12-digit-account>:connection/mcc-00000000", tags={"k": "v"}) -> expect InterconnectValidationException; observe 'NotImplemented'. 3b: same with the caller's own account id and a nonexistent connection suffix -> expect InterconnectClientException; observe 'NotImplemented'.
4. Cause 4a: ic.get_environment(id="sim2real-behav-no-env-<suffix>") -> expect InterconnectClientException (400) with "Unable to process request: This environment model with id <id> could not be found"; observe ResourceNotFoundException (404). 4b: take a real id from ic.list_environments(), confirm ic.get_environment resolves it, then ic.create_connection(bandwidth="1Gbps", attachPoint={"directConnectGateway": "12345678-1234-1234-1234-123456789012"}, environmentId="sim2real-behav-no-env-<suffix>", remoteAccount={"identifier": <caller account>}) -> expect InterconnectClientException; observe ResourceNotFoundException.
5. Cause 5: ic.get_connection(identifier="mcc-00000000") and ic.delete_connection(identifier="lmcc-00000000") -> code is ResourceNotFoundException as expected, but compare the message to "Unable to process request: Unable to retrieve connection <identifier>, as it does not exist".
6. Cause 6: ic.list_connections(maxResults=101) -> ValidationException whose message must not contain the submitted value; then, with Config(parameter_validation=False), ic.list_connections(maxResults=0) -> ValidationException "... Member must have value greater than or equal to 1" (currently unverified because the ceiling case fails first). ic.list_connections(maxResults=5) must still succeed.

Re-replay the checks in trace_ids to confirm.
