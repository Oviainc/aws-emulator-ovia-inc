Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.create-user-duplicate-username-rejected transfer.duplicate-ssh-public-key-rejected transfer.import-ssh-public-key-duplicate-rejected transfer.import-sshpublickey-duplicate-resource-exists-rejected transfer.import-ssh-public-key-body-validated-and-scoped-to-user transfer.sshpublickey-import-attach-delete-reflected transfer.create-user-logical-home-directory-mapping-rules transfer.update-server-ip-address-type-online-rejected transfer.update-server-ip-address-type-conflict-while-online-rejected transfer.update-server-conflicting-state-blocked transfer.update-server-concurrent-modification-conflict transfer.create-user-posix-id-out-of-range-rejected transfer.certificate-unicode-description-rejected
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.create-user-duplicate-username-rejected|run#11|AssertionError|assert message == "User already exists", f"unexpected duplicate-user message: {message!r}"
Issue-Key: transfer.duplicate-ssh-public-key-rejected|run#14|AssertionError|assert message == "Public key already exists", f"unexpected duplicate-key message: {message!r}"
Issue-Key: transfer.import-ssh-public-key-duplicate-rejected|run#15|AssertionError|assert message == "Public key already exists", f"unexpected duplicate key message {message!r}"
Issue-Key: transfer.import-sshpublickey-duplicate-resource-exists-rejected|run#12|AssertionError|assert message == "Public key already exists", f"unexpected duplicate key message {message!r}"
Issue-Key: transfer.import-ssh-public-key-body-validated-and-scoped-to-user|run#33|AssertionError|assert msg_dup == 'Public key already exists', (
Issue-Key: transfer|DeleteSshPublicKey|ResourceNotFoundException
Issue-Key: transfer.create-user-logical-home-directory-mapping-rules|run#15|AssertionError|assert msg_nomap == 'HomeDirectoryMappings must not be empty', (
Issue-Key: transfer.update-server-ip-address-type-online-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: transfer.update-server-ip-address-type-conflict-while-online-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: transfer.update-server-conflicting-state-blocked|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: transfer.update-server-concurrent-modification-conflict|run#13|AssertionError|assert errors == [expected_error, expected_error], (
Issue-Key: transfer.create-user-posix-id-out-of-range-rejected|run#7|AssertionError|assert message == (
Issue-Key: transfer.certificate-unicode-description-rejected|run#6|AssertionError|assert error == ("ValidationException", "1 validation error detected: Value at 'description' failed to satisfy constraint: Member must satisfy regular expressio
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# Transfer error Message rules diverge: fixed service texts rebuilt from request values, generic not-found template, and ValidationException echoing the value and the anchored pattern (6 causes)

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
The HTTP status and error code are right in all six cases below; the Message is produced by a different rule than Transfer's. Each of these texts has a known rule — it is either a FIXED constant the service emits for that condition (so it carries no request data) or, for the ValidationException family, a template built from the request with the member path and the modeled constraint. The emulator instead synthesizes request-derived prose, uses one generic template for all resource types, or echoes data AWS leaves out. The recorded texts are quoted as examples of the rule, not as byte-exact goals.

Cause 1 of 6: duplicate UserName.
AWS: CreateUser for an existing UserName on the same ServerId answers ResourceExistsException (Smithy: httpError 409, members Message/Resource/ResourceType all @required) whose Message is the fixed text "User already exists" — no server id or user name in it.
Emulator: 'User sim2realdupd10398e361cc already exists on server s-00000000000000001'.
Why: the emulator teaches that the duplicate message names the user and server; a client parsing or asserting on the condition sees a shape AWS never emits (and the id echo leaks request data into an error AWS keeps constant).
Fix: the ResourceExistsException construction in the CreateUser handler (services/service-com-amazonaws-transfer-transferservice/src/ops/create_user) must use the service's fixed text.
Traces: transfer.create-user-duplicate-username-rejected.

Cause 2 of 6: duplicate SSH public key.
AWS: ImportSshPublicKey for a key body already held by that user answers ResourceExistsException with the fixed Message "Public key already exists".
Emulator: 'User <user> on server <server-id> already holds this SSH public key' — again rebuilt from the request.
Why: four separate checks pin this exact condition; any caller keying on the duplicate-key diagnostic gets emulator-authored prose.
Fix: the ResourceExistsException construction in the ImportSshPublicKey handler (import_ssh_public_key).
Traces: transfer.duplicate-ssh-public-key-rejected, transfer.import-ssh-public-key-duplicate-rejected, transfer.import-sshpublickey-duplicate-resource-exists-rejected, transfer.import-ssh-public-key-body-validated-and-scoped-to-user.

Cause 3 of 6: not-found text is one generic template across resource types.
AWS: Transfer's ResourceNotFoundException carries a per-resource fixed Message — "Unknown public key ID" for a deleted SshPublicKeyId (the sibling texts "Unknown server", "Unknown user" and "Unknown SecurityPolicy" are the ones the same suite asserts elsewhere and the emulator already matches).
Emulator: 'Unknown sshpublickey' — the internal resource token interpolated into one template, so this resource type is the one that misses the service's wording.
Why: the text is the only thing distinguishing which reference was bad; a generic template built from internal type names changes as internals change.
Fix: per-resource not-found text in the DeleteSshPublicKey/DescribeUser key lookup path, not a shared format string over the internal type name.
Traces: transfer.sshpublickey-import-attach-delete-reflected.

Cause 4 of 6: LOGICAL home directory with no mappings.
AWS: CreateUser with HomeDirectoryType=LOGICAL and no HomeDirectoryMappings answers InvalidRequestException (400, Message @required) with the fixed text "HomeDirectoryMappings must not be empty" (Smithy: HomeDirectoryMappings is @length min 1, so the empty/absent list is the modeled violation).
Emulator: 'HomeDirectoryMappings must be specified when HomeDirectoryType is LOGICAL'.
Why: same condition, different stated rule — the emulator's text attributes the requirement to the type field rather than the list's own minimum.
Fix: the InvalidRequestException text in the CreateUser/UpdateUser home-directory validation.
Traces: transfer.create-user-logical-home-directory-mapping-rules.

Cause 5 of 6: IpAddressType change while the server is ONLINE.
AWS: UpdateServer changing IpAddressType on a running server answers InvalidRequestException (400) with the fixed text "Server must be OFFLINE to change IpAddressType"; the rule itself is documented on the model (Smithy DescribedServer.IpAddressType: "It cannot be changed while the server is online. You must stop the server before modifying this parameter.").
Emulator: 'IpAddressType cannot be changed while the server is in the ONLINE state; stop the server first' — the code, status and the consistency of the refusal under concurrency are all correct, only the text is emulator-authored.
Why: four checks pin this one text, including a concurrency check that compares both racers' messages; emulator-authored prose is what a user's runbook or log assertion then matches.
Fix: the state-conflict text in the UpdateServer handler (update_server).
Traces: transfer.update-server-ip-address-type-online-rejected, transfer.update-server-ip-address-type-conflict-while-online-rejected, transfer.update-server-conflicting-state-blocked, transfer.update-server-concurrent-modification-conflict.

Cause 6 of 6: ValidationException message form echoes the rejected value and the anchored pattern.
AWS: the generic constraint-violation message names the member path and the modeled constraint WITHOUT the offending value, and quotes a pattern without its anchors: recorded "1 validation error detected: Value at 'posixProfile.uid' failed to satisfy constraint: Member must have value less than or equal to 4294967295" (Smithy PosixProfile.Uid @range min 0 max 4294967295) and "1 validation error detected: Value at 'description' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\u0021-\\u007E]+" (Smithy ImportCertificate.Description @pattern ^[\\u0021-\\u007E]+$, @length 1..200).
Emulator: inserts the submitted value — "Value '99999999999' at 'posixProfile.uid' ..." and "Value '证书说明 – café ✅' at 'description' ..." — and quotes the pattern with its ^$ anchors.
Why: the message rule is the requirement here (member path + constraint, value omitted); echoing request data into validation errors both diverges from AWS and reflects caller-supplied bytes back in an error body.
Fix: the shared model-constraint validation message formatter used by the Transfer ops (value omitted, pattern rendered unanchored).
Traces: transfer.create-user-posix-id-out-of-range-rejected, transfer.certificate-unicode-description-rejected.

## Evidence
Cause 1: expected Message 'User already exists'; got 'User sim2realdupd10398e361cc already exists on server s-00000000000000001' (code ResourceExistsException matched).
Cause 2: expected 'Public key already exists'; got 'User <user> on server s-00000000000000001 already holds this SSH public key' (4 checks).
Cause 3: expected 'Unknown public key ID'; got 'Unknown sshpublickey'.
Cause 4: expected 'HomeDirectoryMappings must not be empty'; got 'HomeDirectoryMappings must be specified when HomeDirectoryType is LOGICAL'.
Cause 5: expected 'Server must be OFFLINE to change IpAddressType'; got 'IpAddressType cannot be changed while the server is in the ONLINE state; stop the server first' (both racers in the concurrency check returned the emulator text).
Cause 6: expected "1 validation error detected: Value at 'posixProfile.uid' failed to satisfy constraint: Member must have value less than or equal to 4294967295"; got "... Value '99999999999' at 'posixProfile.uid' ...". Same for 'description': expected pattern rendered as [\\u0021-\\u007E]+ with no value; got the value plus ^[\\u0021-\\u007E]+$.

## Steps to Reproduce
Replay the behavioral suite and run the named checks:
1. transfer.create-user-duplicate-username-rejected — CreateUser twice with the same UserName on one server; read Error.Message of the 409.
2. transfer.duplicate-ssh-public-key-rejected (and the three sibling import checks) — ImportSshPublicKey twice with the same body for one user.
3. transfer.sshpublickey-import-attach-delete-reflected — DeleteSshPublicKey twice for the same SshPublicKeyId.
4. transfer.create-user-logical-home-directory-mapping-rules — CreateUser(HomeDirectoryType='LOGICAL') with no HomeDirectoryMappings.
5. transfer.update-server-ip-address-type-online-rejected (+3 siblings) — UpdateServer(IpAddressType=...) while the server is ONLINE.
6. transfer.create-user-posix-id-out-of-range-rejected — CreateUser(PosixProfile={'Uid': 99999999999,'Gid':1000}); transfer.certificate-unicode-description-rejected — ImportCertificate(Description='证书说明 – café ✅').
