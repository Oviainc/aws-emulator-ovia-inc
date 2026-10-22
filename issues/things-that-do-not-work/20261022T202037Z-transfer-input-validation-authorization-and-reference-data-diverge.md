Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.create-connector-malformed-url-rejected transfer.create-user-logical-home-directory-mapping-rules transfer.import-ssh-public-key-body-validated-and-scoped-to-user transfer.describe-webapp-access-denied-unauthorized-caller transfer.send-workflow-step-state-unauthorized-denied transfer.create-webapp-without-identity-center-denied transfer.access-create-on-service-managed-server-invalid-request-rejected transfer.security-policy-catalog-is-fixed-reference-data transfer.describe-workflow-computed-step-defaults-and-order
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.create-connector-malformed-url-rejected|_client_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected but it succeeded: {result!r}")
Issue-Key: transfer.create-user-logical-home-directory-mapping-rules|_err#18|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: transfer.import-ssh-public-key-body-validated-and-scoped-to-user|_err#35|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: transfer.describe-webapp-access-denied-unauthorized-caller|run#6|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException for an unauthorized caller, got {code}"
Issue-Key: transfer.send-workflow-step-state-unauthorized-denied|run#11|AssertionError|assert message == expected_denial, f"unexpected AccessDeniedException message {message!r}"
Issue-Key: transfer.create-webapp-without-identity-center-denied|run#6|AssertionError|raise AssertionError(f'expected CreateWebApp to be denied, but it succeeded and returned {created!r}')
Issue-Key: transfer.access-create-on-service-managed-server-invalid-request-rejected|run#10|AssertionError|assert code2 == "InvalidRequestException", f"expected InvalidRequestException, got {code2}"
Issue-Key: transfer.security-policy-catalog-is-fixed-reference-data|run#5|AssertionError|assert len(names) >= 20, (
Issue-Key: transfer.describe-workflow-computed-step-defaults-and-order|run#25|AssertionError|assert wf.get('Tags') == [], f"untagged workflow must describe Tags as [], got {wf.get('Tags')!r}"
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# Transfer input validation, authorization and reference data diverge: malformed connector Url and unmapped LOGICAL home directory accepted, duplicate SSH key re-imported, IAM denials not produced, DescribeAccess misclassified, security-policy catalog short, DescribeWorkflow omits Tags

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnector

## Description
Six distinct root causes in the transfer emulator, all grounded in failing behavioral checks of this suite.

=== Cause 1 of 6: CreateConnector accepts a Url with no scheme ===
AWS: Smithy CreateConnector.Url -> Url @length 0..255 (free-form on the wire, so the scheme rule is service-side); modeled error InvalidRequestException (client, HTTP 400). Recorded behavior: an SFTP connector whose Url omits the scheme ('ftp.example.com' instead of 'sftp://ftp.example.com') is refused with HTTP 400 InvalidRequestException and no connector is created; the identical request with a well-formed 'sftp://...' Url succeeds and DescribeConnector echoes the Url verbatim.
Emulator: CreateConnector(Url='ftp.example.com', AccessRole=<role>, SftpConfig={'UserSecretId': <secret arn>}) returns HTTP 200 and a ConnectorId, creating the connector.
Why: a client that relies on the service to validate its connector endpoint persists a connector that can never route a transfer; the error surfaces much later (or never in a test), and infrastructure state diverges from what AWS would have allowed.
Fix: Url validation in the CreateConnector handler (and the same rule in UpdateConnector) in the transfer service crate, raising the modeled InvalidRequestException.
Traces: transfer.create-connector-malformed-url-rejected.

=== Cause 2 of 6: CreateUser accepts HomeDirectoryType=LOGICAL whose HomeDirectory is absent from HomeDirectoryMappings ===
AWS: Smithy CreateUser.HomeDirectory @length 0..1024 @pattern ^(|/.*)$, HomeDirectoryMappings @length 1..50000, HomeDirectoryType enum; modeled error InvalidRequestException (400). Recorded behavior: for a LOGICAL user, HomeDirectory must name one of the mapping Entries; a HomeDirectory that is not an Entry is refused with HTTP 400 InvalidRequestException and the user is not created (the companion rule, LOGICAL with no HomeDirectoryMappings at all, IS already refused correctly by the emulator, so only this leg diverges).
Emulator: CreateUser(HomeDirectoryType='LOGICAL', HomeDirectory='/<bucket>/home', HomeDirectoryMappings=<mappings that do not contain that Entry>) returns HTTP 200 and creates the user.
Why: the user is created with a virtual root that cannot be resolved, so the SFTP session would fail at login; a client gets no signal at provisioning time where AWS gives a 400.
Fix: cross-member validation of HomeDirectory against HomeDirectoryMappings Entries in the CreateUser handler (and UpdateUser) of the transfer crate.
Traces: transfer.create-user-logical-home-directory-mapping-rules.

=== Cause 3 of 6: ImportSshPublicKey duplicate detection is byte-exact instead of whitespace-normalized ===
AWS: Smithy ImportSshPublicKey.SshPublicKeyBody @pattern ^\s*(ssh|ecdsa)-[a-z0-9-]+[ \t]+((base64))(\s*|[ \t]+[\S \t]*\s*)$ — the pattern itself permits leading/trailing whitespace; ResourceExistsException is modeled (client, HTTP 409). Recorded behavior: re-importing an already-stored key body differing only by an added trailing newline is still the same key and is refused with 409 ResourceExistsException; the user's key set is unchanged. The byte-identical re-import case is already handled correctly by the emulator.
Emulator: ImportSshPublicKey with the stored body + '\n' returns HTTP 200 and a second SshPublicKeyId (example 'key-00000000000000003'), so the user now holds the same key twice.
Why: a client that re-applies a key file whose trailing newline differs silently accumulates duplicate keys, and the 409 it uses to detect 'already present' never fires.
Fix: normalize surrounding whitespace before the duplicate comparison in the ImportSshPublicKey handler / user key state of the transfer crate.
Traces: transfer.import-ssh-public-key-body-validated-and-scoped-to-user.

=== Cause 4 of 6: IAM request authorization is not evaluated — no AccessDeniedException, and when one is produced its diagnostic is not the IAM denial sentence ===
AWS: Smithy models AccessDeniedException as a client error with httpError 403 on DescribeWebApp, CreateWebApp, UpdateWebApp and the workflow operations. Request authorization is evaluated BEFORE resource lookup, so an unauthorized caller gets 403 AccessDeniedException even for an id that does not exist, while the authorized caller gets 404 ResourceNotFoundException for the same id. AWS's implicit-deny message is a documented, request-derived sentence: 'User: <assumed-role arn> is not authorized to perform: <service:Action> on resource: <resource arn> because no identity-based policy allows the <service:Action> action'.
Emulator: (a) DescribeWebApp called with credentials from a freshly assumed IAM role carrying zero permissions returns HTTP 404 ResourceNotFoundException instead of HTTP 403 AccessDeniedException — authorization is never evaluated, so the authorized and unauthorized callers are indistinguishable. (b) SendWorkflowStepState from the same kind of zero-permission principal does return AccessDeniedException, but with a fixed generic message (example 'You do not have sufficient access to perform this action.') instead of the request-derived denial sentence naming the principal, the action transfer:SendWorkflowStepState and the workflow resource ARN. (c) CreateWebApp with no IAM Identity Center application permission available succeeds with HTTP 200 and a WebAppId instead of being denied with 403 AccessDeniedException at the sso:CreateApplication boundary.
Why: any client or test that asserts the authorization boundary — least-privilege regression tests, permission-probing tools, error handlers that branch on 403 vs 404 — gets the wrong answer; and a client parsing the standard denial sentence to report which action/resource was denied gets nothing actionable.
Fix: the authorization layer the transfer handlers run behind (evaluate the caller's identity-based policy before resource lookup and return the modeled 403 AccessDeniedException), plus the shared AccessDeniedException renderer so the message is built from the principal ARN, the action and the resource ARN.
Traces: transfer.describe-webapp-access-denied-unauthorized-caller, transfer.send-workflow-step-state-unauthorized-denied, transfer.create-webapp-without-identity-center-denied.

=== Cause 5 of 6: DescribeAccess on a SERVICE_MANAGED server returns ResourceNotFoundException instead of InvalidRequestException ===
AWS: Smithy DescribeAccess takes ServerId + ExternalId (@pattern ^S-1-[\d-]+$) and models BOTH InvalidRequestException (client, HTTP 400) and ResourceNotFoundException (client, HTTP 404). Recorded behavior: Access is a directory-backed concept, so on a server whose IdentityProviderType is SERVICE_MANAGED the IdP-type precondition is evaluated first and DescribeAccess is refused with HTTP 400 InvalidRequestException — the same class CreateAccess uses on that server — rather than 404. The emulator already classifies the CreateAccess leg as InvalidRequestException, so only the read path diverges.
Emulator: DescribeAccess(ServerId=<SERVICE_MANAGED server>, ExternalId=<well-formed SID>) returns HTTP 404 ResourceNotFoundException.
Why: a client cannot distinguish 'this server type has no Access concept at all' (a permanent 400, retrying or creating is pointless) from 'this particular access mapping is absent' (a 404 it can resolve by creating one), so it takes the wrong recovery branch.
Fix: apply the IdentityProviderType precondition before the access lookup in the DescribeAccess handler (and keep DeleteAccess/ListAccesses consistent) in the transfer crate.
Traces: transfer.access-create-on-service-managed-server-invalid-request-rejected.

=== Cause 6 of 6: reference-data and representation gaps — short security-policy catalog, DescribeWorkflow omits Tags ===
AWS: (a) Smithy ListSecurityPolicies returns SecurityPolicyNames (@required list, member @pattern ^Transfer[A-Za-z0-9]*SecurityPolicy-[A-Za-z0-9-]+$); this is fixed reference data and the built-in catalog contains at least 20 policies. (b) Smithy DescribeWorkflow returns DescribedWorkflow, whose Tags member exists; recorded behavior only: an untagged workflow is described with Tags present as an empty list, not with the member absent.
Emulator: (a) ListSecurityPolicies enumerates only 19 names, so at least one built-in policy AWS publishes is missing from the catalog. (b) DescribeWorkflow on a workflow created without Tags omits Tags entirely (the key is absent / None) — note every other assertion in that check passed, including step order and the service-computed SourceFileLocation='${previous.file}' and OverwriteExisting='FALSE' defaults, so this is the single remaining divergence there.
Why: (a) a client that validates a SecurityPolicyName against the catalog, or that iterates it to pick a FIPS/PQ policy, rejects or cannot find a policy real Transfer offers. (b) a client that reads Workflow['Tags'] without a default raises KeyError against the emulator where AWS hands it an empty list.
Fix: (a) the static security-policy catalog backing ListSecurityPolicies/DescribeSecurityPolicy in the transfer crate. (b) the DescribeWorkflow serializer / workflow state so Tags is always emitted, empty when the workflow carries none.
Traces: transfer.security-policy-catalog-is-fixed-reference-data, transfer.describe-workflow-computed-step-defaults-and-order.

Note: open issue 20261022T110916Z-transfer-control-plane-writes-are-over-permissive-unverifiable.md owns a separate set of over-permissive control-plane write refusals (CreateServer protocol/IdP config, malformed role ARNs, UpdateUser policy, SSH key byte preservation) and does not cover any of the six causes above.

## Evidence
Cause 1 — expected: CreateConnector(Url='ftp.example.com', SftpConfig={'UserSecretId': <arn>}) -> HTTP 400 InvalidRequestException, no connector created. actual: HTTP 200 {'ConnectorId': 'c-00000000000000001'} ('expected the call to be rejected but it succeeded').

Cause 2 — expected: CreateUser(HomeDirectoryType='LOGICAL', HomeDirectory='/<bucket>/home', HomeDirectoryMappings=<without that Entry>) -> HTTP 400 InvalidRequestException, user not created. actual: HTTP 200 {'ServerId': 's-00000000000000001', 'UserName': 'sim2realbehavlogical...'}.

Cause 3 — expected: ImportSshPublicKey(SshPublicKeyBody=<already stored body> + '\n') -> HTTP 409 ResourceExistsException, key set unchanged. actual: HTTP 200 {'SshPublicKeyId': 'key-00000000000000003', ...} — a second key stored.

Cause 4 — expected: (a) DescribeWebApp(WebAppId='webapp-00000000000000000') with zero-permission assumed-role credentials -> HTTP 403 AccessDeniedException (the authorized principal gets 404 ResourceNotFoundException for the same id). actual: HTTP 404 ResourceNotFoundException ('expected AccessDeniedException for an unauthorized caller, got ResourceNotFoundException'). (b) expected SendWorkflowStepState denial message 'User: arn:aws:sts::<acct>:assumed-role/<role>/<session> is not authorized to perform: transfer:SendWorkflowStepState on resource: arn:aws:transfer:<region>:<acct>:workflow/<id> because no identity-based policy allows the transfer:SendWorkflowStepState action'; actual message 'You do not have sufficient access to perform this action.' (code AccessDeniedException was correct). (c) expected CreateWebApp -> HTTP 403 AccessDeniedException; actual HTTP 200 {'WebAppId': 'webapp-00000000000000001'}.

Cause 5 — expected: DescribeAccess(ServerId=<SERVICE_MANAGED>, ExternalId='S-1-5-21-...') -> HTTP 400 InvalidRequestException. actual: HTTP 404 ResourceNotFoundException ('expected InvalidRequestException, got ResourceNotFoundException'); the preceding CreateAccess leg on the same server correctly returned InvalidRequestException.

Cause 6 — expected: ListSecurityPolicies -> >= 20 built-in policy names; actual 19 returned ['TransferSFTPConnectorSecurityPolicy-2023-07' ... 'TransferSecurityPolicy-Restricted-2020-06']. expected: DescribeWorkflow(<untagged workflow>)['Workflow']['Tags'] == []; actual Tags absent (None).

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. Cause 1: create an IAM role and a Secrets Manager secret, then transfer.create_connector(Url='ftp.example.com', AccessRole=<role arn>, SftpConfig={'UserSecretId': <secret arn>}). Observe HTTP 200 with a ConnectorId instead of 400 InvalidRequestException.
3. Cause 2: create a server and a role, then transfer.create_user(ServerId=..., UserName=..., Role=..., HomeDirectoryType='LOGICAL', HomeDirectory='/<bucket>/home', HomeDirectoryMappings=[{'Entry': '/home', 'Target': '/<bucket>/shared'}]). Observe HTTP 200.
4. Cause 3: transfer.import_ssh_public_key(ServerId=..., UserName=..., SshPublicKeyBody=BODY), then repeat with SshPublicKeyBody=BODY + '\n'. Observe a second SshPublicKeyId instead of 409 ResourceExistsException.
5. Cause 4: iam.create_role with no attached policies, sts.assume_role it, build a transfer client from those credentials and call describe_web_app(WebAppId='webapp-'+'0'*17) — observe 404 instead of 403; call send_workflow_step_state with the same credentials — observe AccessDeniedException with the generic message; call transfer.create_web_app(IdentityProviderDetails=...) with no Identity Center application permission — observe HTTP 200.
6. Cause 5: transfer.create_server() (defaults to IdentityProviderType=SERVICE_MANAGED), then transfer.describe_access(ServerId=..., ExternalId='S-1-5-21-111111111-222222222-333333333-1001'). Observe 404 ResourceNotFoundException.
7. Cause 6: transfer.list_security_policies() and count the names; transfer.create_workflow(Steps=[...]) with no Tags then transfer.describe_workflow(WorkflowId=...) and inspect ['Workflow'].get('Tags').
