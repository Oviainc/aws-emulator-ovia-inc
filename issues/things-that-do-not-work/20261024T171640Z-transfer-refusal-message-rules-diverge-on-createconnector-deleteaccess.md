Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.create-connector-missing-url-rejected transfer.delete-access-on-service-managed-server-invalid-request-rejected transfer.list-profiles-pagination-next-token-exactness
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.create-connector-missing-url-rejected|run#6|AssertionError|assert error == ("InvalidRequestException", "One of As2Config or SftpConfig must be specified"), error
Issue-Key: transfer.delete-access-on-service-managed-server-invalid-request-rejected|run#18|AssertionError|assert message == "Cannot delete access on server with IdP type: SERVICE_MANAGED", (
Issue-Key: transfer.list-profiles-pagination-next-token-exactness|run#18|AssertionError|assert msg == 'Invalid nextToken', f"unexpected forged-token diagnostic {msg!r}"
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# Transfer refusal Message rules diverge on CreateConnector, DeleteAccess/DescribeAccess and ListProfiles: emulator-authored prose and request-derived text instead of the service's fixed texts (3 causes)

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnector

## Description
Three refusals in the Transfer control plane carry the right HTTP status and the right error code but a Message produced by a different rule than Transfer's. For each of these conditions AWS emits a FIXED service constant that carries no request data; that constant IS the rule (the same rule class already established and accepted for this service in 20261022T110916Z-transfer-error-message-rules-diverge-fixed-service-texts.md, which covered CreateUser / ImportSshPublicKey / DeleteSshPublicKey / UpdateServer / ImportCertificate and does NOT cover the three operations below). The emulator instead synthesizes its own prose, and in one case rebuilds the text from request values. The recorded texts are quoted as examples of the rule, not as byte-exact goals.

Cause 1 of 3: CreateConnector with neither As2Config nor SftpConfig.
AWS: CreateConnector whose input has no As2Config and no SftpConfig cannot know which connector type to build and is refused with InvalidRequestException (Smithy: CreateConnector errors include InvalidRequestException, errorKind client, httpError 400; As2Config and SftpConfig are both optional members so the cross-member requirement is service-side, not modeled). The fixed text the service emits for the condition is, as recorded, "One of As2Config or SftpConfig must be specified".
Emulator: same code and status, Message 'exactly one of As2Config and SftpConfig must be specified' — emulator-authored wording for the same condition.
Why: a client (or a runbook / log assertion / support tooling) keyed on the service's missing-config diagnostic sees a string AWS never emits, and the "exactly one" phrasing additionally states a stricter rule (mutual exclusion) than the condition actually being reported (at least one).
Fix: the InvalidRequestException text in the CreateConnector handler's cross-member validation (services/service-com-amazonaws-transfer-transferservice/src/ops/create_connector*) must be the service's fixed constant.
Traces: transfer.create-connector-missing-url-rejected.

Cause 2 of 3: DeleteAccess / DescribeAccess on a SERVICE_MANAGED server rebuild the refusal text from request values.
AWS: Access is a directory-group concept, so DeleteAccess and DescribeAccess against a server whose IdentityProviderType is SERVICE_MANAGED are refused with InvalidRequestException (Smithy: both operations list InvalidRequestException, errorKind client, httpError 400; the IdP-type check precedes any ExternalId existence lookup, so a never-granted ExternalId gets this and not ResourceNotFoundException). The Message is a fixed per-operation constant with no request data in it — recorded as "Cannot delete access on server with IdP type: SERVICE_MANAGED" for DeleteAccess and "Cannot describe access on server with IdP type: SERVICE_MANAGED" for DescribeAccess.
Emulator: the code and status are right, but the text is rebuilt from the request: 'IdentityProviderType must be AWS_DIRECTORY_SERVICE when an access is addressed by ExternalId on server s-00000000000000001, whose IdentityProviderType is SERVICE_MANAGED' (the server id shown is an example; the emulator interpolates whatever ServerId was sent).
Why: this is the same defect class as the already-fixed duplicate-user / duplicate-public-key causes — caller-supplied request bytes are reflected back into an error body AWS keeps constant, so any client matching the condition on its diagnostic, and any log of it, differs from AWS and changes with the request.
Fix: the InvalidRequestException construction in the DeleteAccess and DescribeAccess handlers' IdP-type guard (services/service-com-amazonaws-transfer-transferservice/src/ops/delete_access*, describe_access*) — a fixed per-operation text, not a format string over the server record.
Traces: transfer.delete-access-on-service-managed-server-invalid-request-rejected.

Cause 3 of 3: forged NextToken diagnostic on ListProfiles.
AWS: ListProfiles with a NextToken that is not a continuation token the service issued is refused with InvalidNextTokenException (Smithy: ListProfiles errors include InvalidNextTokenException, errorKind client, httpError 400; NextToken is @length 1..6144 with no pattern, so a garbage token reaches the service and is a server-side refusal). The Message is the fixed service constant, recorded as "Invalid nextToken".
Emulator: code InvalidNextTokenException is correct; Message is 'The NextToken is not valid'. The rest of the pagination contract in this check passed (MaxResults honoured, no repeated tokens, terminal page omits NextToken).
Why: the only remaining divergence on an otherwise-correct pagination path is the diagnostic every Transfer List* operation shares, so each List* handler that validates a token inherits the wrong text.
Fix: the InvalidNextTokenException text in the shared pagination-token validation used by the Transfer List* handlers (reached here via services/service-com-amazonaws-transfer-transferservice/src/ops/list_profiles*).
Traces: transfer.list-profiles-pagination-next-token-exactness.

## Evidence
Cause 1 (transfer.create-connector-missing-url-rejected): CreateConnector(AccessRole=<role>) with no As2Config/SftpConfig — expected ('InvalidRequestException', 'One of As2Config or SftpConfig must be specified'); got ('InvalidRequestException', 'exactly one of As2Config and SftpConfig must be specified'). Code and 400 status matched; the rejected call correctly created no connector.

Cause 2 (transfer.delete-access-on-service-managed-server-invalid-request-rejected): DeleteAccess(ServerId=<SERVICE_MANAGED server>, ExternalId=S-1-5-21-...) — expected code InvalidRequestException (matched) with Message 'Cannot delete access on server with IdP type: SERVICE_MANAGED'; got 'IdentityProviderType must be AWS_DIRECTORY_SERVICE when an access is addressed by ExternalId on server s-00000000000000001, whose IdentityProviderType is SERVICE_MANAGED'. The same server/ExternalId pair is asserted for DescribeAccess with 'Cannot describe access on server with IdP type: SERVICE_MANAGED' immediately after. Positive control in the same check (CreateUser/DescribeUser/DeleteUser on that same server with that same role, and the 'Unknown user' not-found text) passed, so the refusal is specific to the Access/IdP-type path.

Cause 3 (transfer.list-profiles-pagination-next-token-exactness): ListProfiles(NextToken='sim2real-behav-not-a-real-next-token') — expected ('InvalidNextTokenException', 'Invalid nextToken'); got code InvalidNextTokenException (matched) with Message 'The NextToken is not valid'. Everything earlier in the check passed: MaxResults=1 paged, no token reuse, terminal page omitted NextToken, no duplicate ProfileIds.

## Steps to Reproduce
Replay the behavioral suite and run these three checks against the emulator:
1. transfer.create-connector-missing-url-rejected — call CreateConnector with only AccessRole set (no Url, no As2Config, no SftpConfig) and read Error.Code/Error.Message of the 400.
2. transfer.delete-access-on-service-managed-server-invalid-request-rejected — CreateServer with the default IdentityProviderType (SERVICE_MANAGED), then DeleteAccess(ServerId=<that server>, ExternalId='S-1-5-21-<digits>') for an ExternalId that was never granted, then DescribeAccess with the same pair; read Error.Message of each 400.
3. transfer.list-profiles-pagination-next-token-exactness — create a few LOCAL profiles, then call ListProfiles(NextToken='sim2real-behav-not-a-real-next-token') and read Error.Message of the 400.
