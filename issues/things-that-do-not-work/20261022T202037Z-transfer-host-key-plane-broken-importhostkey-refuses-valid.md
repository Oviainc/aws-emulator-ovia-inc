Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.host-key-lifecycle-import-describe-delete transfer.hostkey-import-describe-delete-404 transfer.host-key-added-timestamp-precision-preserved transfer.import-host-key-duplicate-rejected transfer.import-hostkey-duplicate-resource-exists-rejected transfer.imported-host-key-active-without-downtime-window transfer.delete-host-key-not-found-error transfer.server-auto-generates-host-key-when-none-supplied transfer.server-side-default-host-key-managed transfer.deploy-server-cfn-start-stop-behavior transfer.deploy-server-cfn-then-start-stop transfer.deploy-server-cloudformation-then-start-stop-behaves transfer.list-file-transfer-results-preserves-submission-order transfer.start-file-transfer-inactive-connector-rejected transfer.start-remote-move-invalid-path-error transfer.test-identity-provider-wrong-server-type-error transfer.concurrent-update-webapp-optimistic-lock transfer.update-webapp-concurrent-modification-conflict
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|ImportHostKey|InvalidRequestException
Issue-Key: transfer.server-auto-generates-host-key-when-none-supplied|run#8|AssertionError|assert host_keys, (
Issue-Key: transfer.server-side-default-host-key-managed|run#9|AssertionError|assert matching, (
Issue-Key: transfer|DescribeServer|ResourceNotFoundException
Issue-Key: transfer|StartFileTransfer|NotImplemented
Issue-Key: transfer.start-remote-move-invalid-path-error|run#10|AssertionError|assert code_malformed_path == "InvalidRequestException", (
Issue-Key: transfer|TestIdentityProvider|NotImplemented
Issue-Key: transfer.concurrent-update-webapp-optimistic-lock|run#11|AssertionError|assert len(successes) == 1, f"expected exactly one winning UpdateWebApp call, got {results}"
Issue-Key: transfer.update-webapp-concurrent-modification-conflict|run#11|AssertionError|assert len(successes) == 1, f"expected exactly one winner, got codes={codes}"
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# Transfer host-key plane broken: ImportHostKey refuses valid OpenSSH private keys, auto-generated default host key invisible, CloudFormation-provisioned servers unknown to DescribeServer, and four modeled operations are still NotImplemented

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportHostKey

## Description
Four distinct root causes in the transfer emulator, all grounded in failing behavioral checks of this suite.

=== Cause 1 of 4: ImportHostKey rejects a well-formed OpenSSH-format private key ===
AWS: Smithy (com.amazonaws.transfer) ImportHostKey.HostKeyBody -> HostKey, @required, @length min 0 max 4096, @sensitive; modeled errors InvalidRequestException (client, HTTP 400), ResourceExistsException (409), ResourceNotFoundException (404), ThrottlingException (429). A private key serialized in the OpenSSH private-key container (PEM armour '-----BEGIN OPENSSH PRIVATE KEY-----', i.e. cryptography's PrivateFormat.OpenSSH / NoEncryption) is exactly what Transfer accepts, and the call succeeds returning HostKeyId matching ^hostkey-[0-9a-f]{17}$.
Emulator: every ImportHostKey call in this suite is refused with HTTP 400 InvalidRequestException, message example 'HostKeyBody must be a private key in OpenSSH format', even though the body supplied IS an OpenSSH-format private key (Ed25519, OpenSSH container, unencrypted). The HostKeyBody validator therefore accepts no key material at all.
Why: the entire host-key plane of Transfer is unreachable for a client. No host key can be imported, so DescribeHostKey, UpdateHostKey, ListHostKeys and DeleteHostKey can never be exercised, and any customer flow or IaC that seeds a server's host key fails at the first call. It also masks the correctness of every downstream host-key behavior (not-found classification, duplicate detection, DateImported stability).
Fix: the ImportHostKey handler's HostKeyBody validation in the transfer service crate (services/service-com-amazonaws-transfer-transferservice) — it must recognize the OpenSSH private-key container (and the PEM/PKCS#8 forms AWS accepts) rather than refusing everything.
Traces: transfer.host-key-lifecycle-import-describe-delete, transfer.hostkey-import-describe-delete-404, transfer.host-key-added-timestamp-precision-preserved, transfer.import-host-key-duplicate-rejected, transfer.import-hostkey-duplicate-resource-exists-rejected, transfer.imported-host-key-active-without-downtime-window, transfer.delete-host-key-not-found-error.

=== Cause 2 of 4: a server's auto-generated default host key is not exposed as a queryable resource ===
AWS: recorded behavior only (the model does not state it). CreateServer with no host key supplied generates a default SFTP host key; DescribeServer reports its fingerprint, and that same key is enumerable through ListHostKeys / DescribeHostKey as a managed host-key resource whose fingerprint matches the server's active one.
Emulator: after CreateServer (EndpointType=PUBLIC, IdentityProviderType=SERVICE_MANAGED, Protocols=[SFTP]) with no ImportHostKey call, ListHostKeys returns an empty list; the server nevertheless reports an active fingerprint (example 'SHA256:Wi2Vp5OXhUlxBjvkUSHyTLDLiWusGp4q2r8hBQCz8hI'), so no listed key can match it.
Why: a client that enumerates a server's host keys to audit or rotate them sees zero keys on a server that is actually serving SFTP with a key, and rotation logic that expects to find the current default key to replace has nothing to act on.
Fix: the CreateServer handler / server state in the transfer crate must materialize the generated default host key into the host-key collection that ListHostKeys and DescribeHostKey read.
Traces: transfer.server-auto-generates-host-key-when-none-supplied, transfer.server-side-default-host-key-managed.

=== Cause 3 of 4: a CloudFormation-provisioned AWS::Transfer::Server is unknown to the Transfer service ===
AWS: recorded behavior only. A stack containing a single AWS::Transfer::Server (Protocols=[SFTP], EndpointType=PUBLIC, IdentityProviderType=SERVICE_MANAGED) reaching CREATE_COMPLETE produces a real server; !GetAtt Server.ServerId is that server's id, and transfer:DescribeServer on it succeeds and reports State in (OFFLINE, ONLINE), after which StartServer/StopServer drive it between ONLINE and OFFLINE.
Emulator: the stack reaches CREATE_COMPLETE and the ServerId output is returned, but DescribeServer on that id fails with HTTP 404 ResourceNotFoundException (message example 'Unknown server'). The id handed out by CloudFormation does not correspond to any server in Transfer's state.
Why: every IaC-driven workflow is broken end to end — the deploy appears to succeed and then nothing can be done with the resource it claims to have created, which is strictly worse than a failed stack because it hides the gap until the first post-deploy call.
Fix: the CloudFormation resource provisioning for AWS::Transfer::Server must create the server through the same state the Transfer handlers read (and return that server's real id from GetAtt ServerId), in the CloudFormation provider wiring plus the transfer crate's server state.
Traces: transfer.deploy-server-cfn-start-stop-behavior, transfer.deploy-server-cfn-then-start-stop, transfer.deploy-server-cloudformation-then-start-stop-behaves.

=== Cause 4 of 4: four modeled Transfer operations are scaffolded but not implemented ===
AWS: Smithy models all four with full signatures and error sets — StartFileTransfer (ConnectorId @required ^c-([0-9a-f]{17})$, output TransferId; errors InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 429), StartRemoteMove (ConnectorId/SourcePath/TargetPath @required; errors InvalidRequestException 400, ResourceNotFoundException 404), TestIdentityProvider (ServerId/UserName @required, output StatusCode @required; errors InvalidRequestException 400, ResourceNotFoundException 404), UpdateWebApp (WebAppId @required ^webapp-[0-9a-f]{17}$; errors AccessDeniedException 403, ConflictException 409, InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 429).
Emulator: each returns an unmodeled 'NotImplemented' error, message example 'operation <Op> is scaffolded but not implemented in this emulator'. Observed codes: StartFileTransfer -> NotImplemented; StartRemoteMove -> NotImplemented where InvalidRequestException is required for a malformed path on an existing AS2 connector; TestIdentityProvider -> NotImplemented; UpdateWebApp -> NotImplemented for both concurrent writers, so no optimistic-lock winner exists.
Why: NotImplemented is not in any of these operations' modeled error sets, so SDK clients get an error class they cannot handle, retry or distinguish from a real service fault; connector transfers, remote moves, IdP testing and web-app updates are entirely unavailable.
Fix: implement the four operation handlers in the transfer crate (ops/start_file_transfer.rs, ops/start_remote_move.rs, ops/test_identity_provider.rs, ops/update_web_app.rs) against the connector/server/web-app state, including the modeled 404/400/409 classification and (for UpdateWebApp) a single winner under concurrent modification.
Traces: transfer.list-file-transfer-results-preserves-submission-order, transfer.start-file-transfer-inactive-connector-rejected, transfer.start-remote-move-invalid-path-error, transfer.test-identity-provider-wrong-server-type-error, transfer.concurrent-update-webapp-optimistic-lock, transfer.update-webapp-concurrent-modification-conflict.

Note: open issue 20261022T110916Z-transfer-control-plane-writes-are-over-permissive-unverifiable.md covers a different set of control-plane write refusals and does not cover any of the four causes above.

## Evidence
Cause 1 — expected: ImportHostKey(ServerId=s-..., HostKeyBody=<OpenSSH Ed25519 private key PEM>) -> 200 with HostKeyId ^hostkey-[0-9a-f]{17}$. actual: HTTP 400 InvalidRequestException 'HostKeyBody must be a private key in OpenSSH format' (botocore.errorfactory.InvalidRequestException raised on the ImportHostKey call in 7 checks).

Cause 2 — expected: ListHostKeys(ServerId=<server created with no host key>) -> at least one entry whose fingerprint equals the server's active fingerprint. actual: 'ListHostKeys returned no entries for a server that was never given an explicit ImportHostKey call'; 'none of the listed host keys [] match the server's active fingerprint SHA256:...'.

Cause 3 — expected: DescribeServer(ServerId=<CFN GetAtt Server.ServerId>) -> 200 with Server.State in (OFFLINE, ONLINE). actual: HTTP 404 ResourceNotFoundException 'Unknown server', after cfn.describe_stacks reported StackStatus CREATE_COMPLETE and returned the ServerId output.

Cause 4 — expected: StartFileTransfer/StartRemoteMove/TestIdentityProvider/UpdateWebApp answer from their modeled error sets (e.g. StartRemoteMove with SourcePath='not-an-absolute-path.txt' on an existing connector -> 400 InvalidRequestException; two concurrent UpdateWebApp calls -> exactly one 200 winner). actual: 'An error occurred (NotImplemented) when calling the <Op> operation: operation <Op> is scaffolded but not implemented in this emulator'; the concurrency checks observed codes=['NotImplemented', 'NotImplemented'].

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. Cause 1: transfer.create_server(EndpointType='PUBLIC', IdentityProviderType='SERVICE_MANAGED', Protocols=['SFTP']); generate an Ed25519 key and serialize it with cryptography PrivateFormat.OpenSSH + NoEncryption; call transfer.import_host_key(ServerId=..., HostKeyBody=pem). Observe 400 InvalidRequestException instead of a HostKeyId.
3. Cause 2: create the same server with no ImportHostKey call, then transfer.list_host_keys(ServerId=...) and compare against transfer.describe_server(ServerId=...)['Server'] host-key fingerprint. Observe an empty HostKeys list.
4. Cause 3: cloudformation.create_stack with a template whose only resource is AWS::Transfer::Server (Protocols [SFTP], EndpointType PUBLIC, IdentityProviderType SERVICE_MANAGED) and an output !GetAtt Server.ServerId; wait for CREATE_COMPLETE; read the ServerId output; call transfer.describe_server(ServerId=that id). Observe 404 ResourceNotFoundException.
5. Cause 4: call transfer.start_file_transfer, transfer.start_remote_move, transfer.test_identity_provider and transfer.update_web_app with valid inputs. Observe NotImplemented from each.
