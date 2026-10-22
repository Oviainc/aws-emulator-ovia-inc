Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.create-server-missing-iam-role-denied transfer.update-user-invalid-policy-rejected transfer.create-server-missing-permission-denied transfer.create-server-invalid-endpoint-type-rejected transfer.create-server-rejects-invalid-protocol-and-idp-config transfer.create-access-invalid-role-arn-rejected transfer.update-user-malformed-role-arn-rejected transfer.nonexistent-server-id-refused-not-empty-success transfer.ssh-public-key-bytes-preserved
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.create-server-missing-iam-role-denied|_client_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: transfer.update-user-invalid-policy-rejected|_error#10|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: transfer.create-server-missing-permission-denied|_client_error#16|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: transfer.create-server-invalid-endpoint-type-rejected|run#4|AssertionError|assert error == ("InvalidRequestException", "FTP and FTPS protocols unsupported for IdentityProviderType SERVICE_MANAGED"), error
Issue-Key: transfer.create-server-rejects-invalid-protocol-and-idp-config|run#5|AssertionError|assert msg_ftp == (
Issue-Key: transfer.create-access-invalid-role-arn-rejected|run#5|AssertionError|assert error == ("InvalidRequestException", "Invalid Role ARN"), error
Issue-Key: transfer.update-user-malformed-role-arn-rejected|run#9|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: transfer.nonexistent-server-id-refused-not-empty-success|run#12|AssertionError|assert cross_code == 'AccessDeniedException', (
Issue-Key: transfer.ssh-public-key-bytes-preserved|run#19|AssertionError|assert got == expected, (
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# Transfer control-plane writes are over-permissive: unverifiable identity providers, malformed policies and role ARNs accepted or refused by the wrong rule (7 causes)

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServer

## Description
The emulated Transfer control plane accepts requests real AWS refuses, and where it does refuse it refuses by a different rule or with an error shape Transfer does not model. Seven distinct root causes, each with its own failing checks.

Cause 1 of 7: CreateServer does not verify an API_GATEWAY identity provider.
AWS: CreateServer with IdentityProviderType=API_GATEWAY and IdentityProviderDetails={Url, InvocationRole} synchronously verifies that the given role can invoke the given execute-api endpoint and refuses an unverifiable one with InvalidRequestException (Smithy: CreateServer errors include InvalidRequestException, errorKind client, httpError 400; the modeled 200 output is only {ServerId}). Recorded message example: "Unable to verify access to API <url>". (Measured per the check: an untrusted or nonexistent LoggingRole is by contrast silently accepted, so only this precondition is synchronous.)
Emulator: returns HTTP 200 with a ServerId for an InvocationRole with no invoke permission pointing at an API that does not exist.
Why: a client gets a server that can never authenticate any user, and its "was my IdP wiring valid?" test always passes; a stack that should fail at deploy time deploys clean.
Fix: CreateServer's handler/validation in services/service-com-amazonaws-transfer-transferservice/src/ops/ (create_server) must gate API_GATEWAY identity-provider details before persisting the server.
Traces: transfer.create-server-missing-iam-role-denied.

Cause 2 of 7: UpdateUser accepts a session Policy that is not valid JSON.
AWS: UpdateUser rejects a syntactically invalid scope-down policy document with InvalidRequestException (Smithy: UpdateUser errors = InvalidRequestException 400 / ResourceNotFoundException 404 / ThrottlingException / InternalServiceError / ServiceUnavailableException; Policy is @length 0..2048 with no pattern, so the JSON check is service-side). Recorded message example: "Policy is not a valid JSON document".
Emulator: returns 200 {ServerId, UserName} for Policy='{"Version": "2012-10-17", "Statement": [ this is not json'.
Why: the malformed document is stored, so the user's effective permissions silently diverge from what the caller believes was rejected, and a config-validation path that AWS fails passes here.
Fix: UpdateUser's validation (update_user handler) must parse Policy before persisting.
Traces: transfer.update-user-invalid-policy-rejected.

Cause 3 of 7: no IAM authorization gate on the Transfer control plane.
AWS: a principal whose policy contains an explicit Deny on transfer:CreateServer gets AccessDeniedException (Smithy: AccessDeniedException is modeled on CreateServer, errorKind client, httpError 403).
Emulator: the same call from the explicitly-denied access key returns 200 with a ServerId.
Why: every negative authorization test in a customer's suite silently passes, and least-privilege regressions are invisible against the emulator.
Fix: authorization check in the Transfer request path (shared auth layer consulted by services/service-com-amazonaws-transfer-transferservice/src/ops/*), not the individual handler bodies.
Traces: transfer.create-server-missing-permission-denied.

Cause 4 of 7: the FTP/FTPS refusal is derived from EndpointType instead of IdentityProviderType.
AWS: with the default IdentityProviderType=SERVICE_MANAGED, CreateServer(Protocols=[FTP]) is refused as an identity-provider x protocol incompatibility before any endpoint-type rule is reached. Smithy DescribedServer.Protocols documents both rules: "If Protocol includes either FTP or FTPS, then the EndpointType must be VPC and the IdentityProviderType must be either AWS_DIRECTORY_SERVICE, AWS_LAMBDA, or API_GATEWAY" — and the recorded AWS rejection for this input is InvalidRequestException "FTP and FTPS protocols unsupported for IdentityProviderType SERVICE_MANAGED".
Emulator: answers InvalidRequestException 'EndpointType must be VPC when Protocols includes FTP' — the wrong predicate, which means the IdP x protocol rule is not implemented at all: a request with EndpointType=VPC + SERVICE_MANAGED + FTP would be accepted here while AWS refuses it.
Why: callers probing which combination is legal are told to fix the endpoint type, and the genuinely illegal SERVICE_MANAGED+FTP combination gets created.
Fix: CreateServer validation ordering/predicates (create_server handler): evaluate the IdentityProviderType x Protocols rule before the EndpointType rule.
Traces: transfer.create-server-invalid-endpoint-type-rejected, transfer.create-server-rejects-invalid-protocol-and-idp-config.

Cause 5 of 7: malformed Role ARNs are refused with an unmodeled ValidationException instead of InvalidRequestException.
AWS: Role is modeled @length 20..2048 @pattern ^arn:.*role/\\S+$ on both CreateAccess and UpdateUser, and Transfer surfaces the refusal as its own client error: InvalidRequestException (400). ValidationException appears in NEITHER operation's modeled error list (Smithy CreateAccess errors = InternalServiceError, InvalidRequestException, ResourceExistsException, ResourceNotFoundException, ServiceUnavailableException; UpdateUser errors = InternalServiceError, InvalidRequestException, ResourceNotFoundException, ServiceUnavailableException, ThrottlingException). Recorded AWS message example for CreateAccess: "Invalid Role ARN".
Emulator: answers ValidationException with the generic framework text "1 validation error detected: Value 'not-an-iam-role-arn-at-all' at 'role' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.*role/\\S+$".
Why: an error code the SDK's service model does not list is not catchable by transfer.exceptions.InvalidRequestException, so client retry/branch logic on this rejection never fires.
Fix: the role-ARN rejection path of the create_access / update_user handlers must raise the service's InvalidRequestException rather than the generic model-constraint error.
Traces: transfer.create-access-invalid-role-arn-rejected, transfer.update-user-malformed-role-arn-rejected.

Cause 6 of 7: CreateUser resolves the ServerId before refusing a cross-account pass-role.
AWS: CreateUser validates the ACCOUNT of the passed Role ARN before resolving ServerId, so a role ARN in a foreign account on an absent server is refused with AccessDeniedException (recorded message example: "Cross-account pass role is not allowed."); a nonexistent role in the CALLER's own account on the same absent server yields ResourceNotFoundException instead, which is how the ordering is observable.
Emulator: answers ResourceNotFoundException for the foreign-account role, i.e. it resolves the server first and never checks the role's account at all.
Why: a caller cannot distinguish "your role belongs to another account" from "that server does not exist", and cross-account pass-role — a privilege-escalation guard — is unenforced.
Fix: CreateUser handler validation ordering (create_user), with an account check on Role before the server lookup.
Traces: transfer.nonexistent-server-id-refused-not-empty-success.

Cause 7 of 7: ImportSshPublicKey stores trailing whitespace instead of stripping it.
AWS: the submitted SshPublicKeyBody is returned by DescribeUser with trailing whitespace stripped and interior content (key material and comment) byte-identical; the modeled member allows the trailing-whitespace form (Smithy SshPublicKey.SshPublicKeyBody @pattern ...(\\s*|[ \\t]+[\\S \\t]*\\s*)$), so the normalization is service behavior (recorded behavior only).
Emulator: returns the body verbatim — 417 bytes back for the 417 bytes sent, including the two trailing spaces AWS removes.
Why: a client that compares the key it uploaded against what DescribeUser reports (or fingerprints it) gets a mismatch on AWS but a match here, so the normalization bug is masked in the other direction.
Fix: ImportSshPublicKey handler/state normalization (import_ssh_public_key) before the key is stored.
Traces: transfer.ssh-public-key-bytes-preserved.

## Evidence
Cause 1: expected ('InvalidRequestException', 'Unable to verify access to API <url>'); got HTTP 200 {'ServerId': 's-...'}.
Cause 2: expected ('InvalidRequestException', 'Policy is not a valid JSON document'); got HTTP 200 {'ServerId': 's-...', 'UserName': '...'}.
Cause 3: expected AccessDeniedException (403) for the explicitly-denied principal; got HTTP 200 {'ServerId': 's-...'}.
Cause 4: expected ('InvalidRequestException', 'FTP and FTPS protocols unsupported for IdentityProviderType SERVICE_MANAGED'); got ('InvalidRequestException', 'EndpointType must be VPC when Protocols includes FTP').
Cause 5: expected code InvalidRequestException (recorded message 'Invalid Role ARN'); got code ValidationException ('1 validation error detected: Value '...' at 'role' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.*role/\\S+$'). ValidationException is not modeled on either operation.
Cause 6: expected code AccessDeniedException for a Role ARN in account 111122223333 against server s-00000000000000000; got code ResourceNotFoundException.
Cause 7: DescribeUser returned the key body with the submitted trailing whitespace intact (417 bytes sent, 417 bytes returned); AWS returns the rstrip'ed form (415 bytes).

## Steps to Reproduce
Replay the behavioral suite against the emulator and run the named checks:
1. transfer.create-server-missing-iam-role-denied — CreateServer(IdentityProviderType='API_GATEWAY', IdentityProviderDetails={'Url': <nonexistent execute-api URL>, 'InvocationRole': <role with no invoke permission>}).
2. transfer.update-user-invalid-policy-rejected — CreateUser, then UpdateUser(Policy='{"Version": "2012-10-17", "Statement": [ this is not json').
3. transfer.create-server-missing-permission-denied — create an IAM user with Allow transfer:* plus explicit Deny transfer:CreateServer, then call CreateServer with that user's access key.
4. transfer.create-server-invalid-endpoint-type-rejected / transfer.create-server-rejects-invalid-protocol-and-idp-config — CreateServer(EndpointType='PUBLIC', Protocols=['FTP']) with the default SERVICE_MANAGED IdP.
5. transfer.create-access-invalid-role-arn-rejected / transfer.update-user-malformed-role-arn-rejected — CreateAccess(Role='not-an-iam-role-arn-at-all', ...) and UpdateUser(Role='not-a-valid-role-arn').
6. transfer.nonexistent-server-id-refused-not-empty-success — CreateUser(ServerId='s-'+'0'*17, Role='arn:aws:iam::111122223333:role/sim2real-behav-nonexistent-role').
7. transfer.ssh-public-key-bytes-preserved — ImportSshPublicKey with a body ending in two spaces, then DescribeUser and compare to body.rstrip().
