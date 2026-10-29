Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.concurrent-update-webapp-optimistic-lock transfer.connector-config-exclusivity-and-described-shape transfer.create-profile-invalid-type-rejected transfer.create-server-rejects-invalid-protocol-and-idp-config transfer.delete-workflow-referenced-by-server-rejected transfer.start-remote-move-invalid-path-error
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.concurrent-update-webapp-optimistic-lock|run#12|AssertionError|assert all(error == ("ConflictException", _CONFLICT_MESSAGE) for error in refused), (
Issue-Key: transfer.connector-config-exclusivity-and-described-shape|run#16|AssertionError|assert (code_both, msg_both) == (
Issue-Key: transfer.create-profile-invalid-type-rejected|run#5|AssertionError|assert error == ('ValidationException', "1 validation error detected: Value at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [
Issue-Key: transfer.create-server-rejects-invalid-protocol-and-idp-config|run#8|AssertionError|assert msg_ftps == msg_ftp, (
Issue-Key: transfer.delete-workflow-referenced-by-server-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: transfer.start-remote-move-invalid-path-error|run#11|AssertionError|assert msg_malformed_path == "Remote move not supported for AS2 connectors", (
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# Transfer refusal fidelity: six operations answer emulator-authored refusal text, render an enum set in the wrong order, or run their validation gates in the wrong order (6 causes)

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServer

## Description
SIX net-new root causes in the transfer emulator's refusal paths, found by replaying simparity-behavioral-aws-v1. All six share one family of fault — the refusal is authored by the emulator (its own prose, its own value ordering, its own gate ordering) instead of reproduced from the service — but each is a separate defect in a separate handler and each must be closed before submitting. There are SIX.

Related open issues that do NOT cover any cause below (named here so a fixer can see the overlap): '20261029T184519Z-transfer-refusal-paths-diverge-nine-operations-answer-emulator.md' describes the same class for a different, wire-grounded set of steps — its Cause 2 (CreateProfile enum ordering) and Cause 5 (StartRemoteMove on an AS2 connector) touch the same two operations as Cause 3 and Cause 6 here and are still OPEN, so those two causes are re-grounded here in behavioral checks rather than wire steps; '20261022T202037Z-transfer-input-validation-authorization-and-reference-data-diverge.md', '20261024T042010Z-transfer-fidelity-gaps-api-gateway-createserver-always-refused.md' and '20261026T022407Z-transfer-host-key-and-web-app-planes-still.md' cover other operations. The resolved issue '20261025T114306Z-transfer-createworkflow-enum-constraint-validationexception-message-lists-the-workflowsteptype.md' fixed the enum-ordering renderer for CreateWorkflow only; Cause 3 is the same renderer fault, unfixed for ProfileType.

Cause 1 of 6 — UpdateWebApp's optimistic-lock refusal carries emulator prose.
AWS: Smithy: UpdateWebApp(WebAppId required, WebAppUnits) declares ConflictException (errorKind client, httpError 409). The model states no message text, so: recorded behavior only — real AWS refuses the losing writer of two overlapping UpdateWebApp calls on one web app with ConflictException and the fixed text 'Unable to update WebApp'. The whole message is fixed text for this error class; it has no variable slot (it does not name the web app id).
Emulator: the right error code, but Message 'An update is already in progress for this web app; wait for it to complete and try again.' — a sentence AWS never emits.
Why: a client or IaC tool that matches the service's conflict text to decide 'retry after the in-flight roll-out' does not recognise the string, and anything logging/triaging on it sees wording no AWS runbook matches.
Fix: UpdateWebApp's refusal path — the ConflictException message helper must emit the service's fixed string.
Traces: check transfer.concurrent-update-webapp-optimistic-lock; emitted from services/service-com-amazonaws-transfer-transferservice/src/ops/update_web_app.rs:199-203 via crate::state::web_app_update_conflict.

Cause 2 of 6 — CreateConnector's As2Config/SftpConfig exclusivity refusal carries emulator prose.
AWS: Smithy: CreateConnector.As2Config -> As2ConnectorConfig and CreateConnector.SftpConfig -> SftpConnectorConfig are both optional in the model, so the mutual-exclusivity rule is service-side; error InvalidRequestException (errorKind client, httpError 400). Recorded behavior: HTTP 400, code InvalidRequestException, Message 'Only one of As2Config or SftpConfig must be specified' — entirely fixed text for this class (the sibling 'neither' case is 'One of As2Config or SftpConfig must be specified', which the emulator already reproduces).
Emulator: correct status and code, but Message 'As2Config and SftpConfig cannot both be specified'.
Why: the two sibling refusals (neither vs both) are deliberately parallel strings in AWS; an emulator that reproduces one and rewrites the other makes the pair indistinguishable from a different rule, and exact-match assertions on the refusal fail.
Fix: CreateConnector's input validation — the both-configs branch must emit the service's fixed string.
Traces: check transfer.connector-config-exclusivity-and-described-shape (the check's first leg; its remaining legs — the 'neither' refusal, the bad-Url refusal, the three individually-required As2Config algorithm members and the described shape — were not reached and are not asserted to pass).

Cause 3 of 6 — CreateProfile's enum ValidationException lists the value set in model-declaration order.
AWS: Smithy: CreateProfile.ProfileType required, target enum ProfileType with values LOCAL, PARTNER. Recorded behavior: HTTP 400, code ValidationException, message "1 validation error detected: Value at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [PARTNER, LOCAL]". Fixed text: the '<n> validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [...]' template. Variable parts: the member name from the request in its lowerCamel wire spelling (profileType) and the enum value list, which the service renders in ITS published order — PARTNER then LOCAL — not the model's declaration order.
Emulator: identical status, code and template, but the set is printed as '[LOCAL, PARTNER]', i.e. iterated straight off the model shape.
Why: an exact-match assertion on the refusal text fails, and a client that parses the advertised set to repair its request sees an ordering the service never emits.
Fix: CreateProfile's validation — the enum value-set renderer must use the service's per-enum published order (the same fix already applied to WorkflowStepType for CreateWorkflow), not the model shape's member order.
Traces: check transfer.create-profile-invalid-type-rejected.

Cause 4 of 6 — CreateServer evaluates the FTPS certificate rule BEFORE the identity-provider/protocol-family gate.
AWS: Smithy: CreateServer.Protocols -> Protocols (@length 1..4), CreateServer.IdentityProviderType -> IdentityProviderType enum, CreateServer.Certificate optional — all cross-member rules are service-side; error InvalidRequestException (httpError 400). Recorded behavior: with IdentityProviderType=SERVICE_MANAGED and EndpointType=PUBLIC, adding EITHER FTP or FTPS to Protocols is refused with ONE family-level message, 'FTP and FTPS protocols unsupported for IdentityProviderType SERVICE_MANAGED'. Fixed text: 'FTP and FTPS protocols unsupported for IdentityProviderType ' plus one variable part, the IdentityProviderType taken from the request. The per-protocol rules (a VPC endpoint for FTP, an ACM Certificate for FTPS) are never reached from a SERVICE_MANAGED baseline, so FTP and FTPS produce the IDENTICAL string.
Emulator: the FTP leg is right, but the FTPS leg answers Message 'Certificate must be specified when Protocols includes FTPS' — it runs the certificate precondition first, and the sentence is emulator prose on top of that.
Why: the client is told to supply a certificate, which cannot make the request valid: the configuration is refused because SERVICE_MANAGED does not support the FTP/FTPS family at all. A caller (or IaC) that obediently attaches an ACM certificate retries and is refused again, and the two protocols no longer collapse to the one diagnostic AWS gives.
Fix: CreateServer's input validation — order the gates so the IdentityProviderType x protocol-family check runs before the per-protocol certificate/endpoint checks, and emit the service's family-level string with the provider name filled from the request.
Traces: check transfer.create-server-rejects-invalid-protocol-and-idp-config (the later legs — EndpointType=VPC and the three detail-requiring identity providers — were not reached in this run and are not asserted to pass).

Cause 5 of 6 — DeleteWorkflow's in-use refusal carries emulator prose and the wrong shape of identifier list.
AWS: Smithy: DeleteWorkflow(WorkflowId required, @pattern ^w-([a-z0-9]{17})$), idempotent, errors include InvalidRequestException (httpError 400). Recorded behavior: deleting a workflow still named by a server's WorkflowDetails is refused with HTTP 400, code InvalidRequestException, Message 'Workflow is still in use by server <serverId>'. Fixed text: 'Workflow is still in use by server '; variable part: the server id taken from emulator/service state (the server that references the workflow).
Emulator: correct code, but Message 'the workflow cannot be deleted while it is attached to server(s) <serverId>' — a rebuilt sentence with a pluralised 'server(s)' label.
Why: callers that detect 'the workflow is pinned by a server, detach it first' by matching the service's refusal text miss it, and the referencing server id is delivered inside a string shaped differently from AWS's.
Fix: DeleteWorkflow's refusal path — emit AWS's fixed text with the referencing server id interpolated. See services/service-com-amazonaws-transfer-transferservice/src/ops/delete_workflow.rs:187-197.
Traces: check transfer.delete-workflow-referenced-by-server-rejected (its positive control — detach via UpdateServer, then delete, then DescribeWorkflow => ResourceNotFoundException 'Unknown workflow' — was not reached).

Cause 6 of 6 — StartRemoteMove has NO AS2-connector gate, so path-shape validation answers for it and a well-formed path is accepted.
AWS: Smithy: StartRemoteMove(ConnectorId required, SourcePath required, TargetPath required) -> MoveId; errors InvalidRequestException (400) and ResourceNotFoundException (404). StartRemoteMove is documented as moving/renaming a file on the remote SFTP server, so it is unsupported for an AS2 connector. Recorded behavior: against an existing AS2 connector the call is refused with HTTP 400, code InvalidRequestException, Message 'Remote move not supported for AS2 connectors' — entirely fixed text — and the refusal is UNCONDITIONAL on the paths: a malformed relative SourcePath and a well-formed absolute one get the identical code and the identical message. A ConnectorId that does not exist at all is instead ResourceNotFoundException.
Emulator: for a relative SourcePath it answers InvalidRequestException with Message 'SourcePath must be an absolute path on the remote server'; the connector's protocol is never consulted, so for a well-formed absolute path on the same AS2 connector it allocates a MoveId and answers 200.
Why: the caller is told the path is wrong when the operation is not available for this connector type at all; after 'fixing' the path it gets a MoveId for a move that can never happen and polls a move that will never exist, masking an unsupported-protocol bug in the caller.
Fix: StartRemoteMove's handler — add the connector-protocol gate (AS2 => InvalidRequestException with the service's fixed string) after the connector lookup and BEFORE the path-shape validation. See services/service-com-amazonaws-transfer-transferservice/src/ops/start_remote_move.rs:174-196, which resolves the connector, then validates path absoluteness, then records the move, with no protocol branch anywhere.
Traces: check transfer.start-remote-move-invalid-path-error.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (70/110 passed; transfer 69/109). Expected = real AWS behavior as the check records it, Actual = emulator.

Cause 1 — transfer.concurrent-update-webapp-optimistic-lock [mismatch]:
AssertionError: a refused racer must get ConflictException with its exact message, got [('ConflictException', 'An update is already in progress for this web app; wait for it to complete and try again.')]
Expected ('ConflictException', 'Unable to update WebApp').

Cause 2 — transfer.connector-config-exclusivity-and-described-shape [mismatch]:
AssertionError: CreateConnector with BOTH As2Config and SftpConfig must be rejected, got 'InvalidRequestException': 'As2Config and SftpConfig cannot both be specified'
Expected ('InvalidRequestException', 'Only one of As2Config or SftpConfig must be specified').

Cause 3 — transfer.create-profile-invalid-type-rejected [mismatch]:
AssertionError: ('ValidationException', "1 validation error detected: Value at 'profileType' failed to satisfy constraint: Member must satisfy enum value set: [LOCAL, PARTNER]")
Expected the same template with '[PARTNER, LOCAL]'.

Cause 4 — transfer.create-server-rejects-invalid-protocol-and-idp-config [mismatch]:
AssertionError: FTP and FTPS share ONE family-level refusal under SERVICE_MANAGED; got 'Certificate must be specified when Protocols includes FTPS' vs 'FTP and FTPS protocols unsupported for IdentityProviderType SERVICE_MANAGED'
(the FTP leg produced AWS's family message; only the FTPS leg diverges, and it diverges by running the certificate rule first.)

Cause 5 — transfer.delete-workflow-referenced-by-server-rejected [mismatch]:
AssertionError: expected message 'Workflow is still in use by server <serverId>', got 'the workflow cannot be deleted while it is attached to server(s) <serverId>'
(the server id in both strings is the server created by the check; only the surrounding fixed text differs.)

Cause 6 — transfer.start-remote-move-invalid-path-error [mismatch]:
AssertionError: unexpected malformed-path message 'SourcePath must be an absolute path on the remote server'
Expected ('InvalidRequestException', 'Remote move not supported for AS2 connectors') for BOTH the relative and the absolute path against the same AS2 connector.
Source confirmation: services/service-com-amazonaws-transfer-transferservice/src/ops/start_remote_move.rs lines 174-215 contain a connector-existence check, a per-member absolute-path check, a capacity check and then accept_connector_transfer — there is no AS2/SFTP branch, so the absolute-path leg returns 200 with a MoveId.

Model grounding (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): UpdateWebApp declares ConflictException (409); CreateConnector, CreateProfile, CreateServer, DeleteWorkflow and StartRemoteMove all declare InvalidRequestException (400); ProfileType enum members are declared LOCAL, PARTNER; CreateConnector.As2Config and .SftpConfig are both optional; CreateServer.Protocols/IdentityProviderType/Certificate are all optional, so every cross-member rule above is service-side.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite.
3. Re-replay the six checks bound to the six causes:
   - transfer.concurrent-update-webapp-optimistic-lock (two overlapping UpdateWebApp calls on one web app; inspect the refused racer's Message)
   - transfer.connector-config-exclusivity-and-described-shape (CreateConnector with BOTH As2Config and SftpConfig)
   - transfer.create-profile-invalid-type-rejected (CreateProfile with ProfileType='BOGUS')
   - transfer.create-server-rejects-invalid-protocol-and-idp-config (CreateServer Protocols=['SFTP','FTPS'], EndpointType=PUBLIC, IdentityProviderType=SERVICE_MANAGED; compare its Message with the FTP variant's)
   - transfer.delete-workflow-referenced-by-server-rejected (CreateWorkflow, attach it via CreateServer/UpdateServer WorkflowDetails, then DeleteWorkflow)
   - transfer.start-remote-move-invalid-path-error (CreateProfile x2 + CreateConnector with As2Config, then StartRemoteMove with SourcePath='not-an-absolute-path.txt' and again with SourcePath='/x/source.txt')
4. For each, compare the HTTP status, the error code and the full error Message against the expected values listed in Evidence.
