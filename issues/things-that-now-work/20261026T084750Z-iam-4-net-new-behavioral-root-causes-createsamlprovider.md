Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.duplicate-create-returns-entityalreadyexists iam.get-nonexistent-entity-returns-nosuchentity iam.inactive-access-key-is-refused-while-its-active-twin-succeeds iam.op-contract-list-open-id-connect-providers
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.duplicate-create-returns-entityalreadyexists|run#57|AssertionError|assert msg_saml == f"Provider with name {saml_name} already exists.", f"saml: unexpected message {msg_saml!r}"
Issue-Key: iam.get-nonexistent-entity-returns-nosuchentity|_expect#18|AssertionError|assert got_message == message, f"{label}: expected message {message!r}, got {got_message!r}"
Issue-Key: iam.inactive-access-key-is-refused-while-its-active-twin-succeeds|_rejected_code#20|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: iam.op-contract-list-open-id-connect-providers|run#19|AssertionError|assert denied_msg == expected_denial, (
Issue-Commit: 13273ef63a283393f8b97bbdcadeb04fa836ecfd

# iam: 4 net-new behavioral root causes — CreateSAMLProvider/GetPolicyVersion error messages, an Inactive access key is accepted, and the implicit-deny message omits the authorization-details sentence

> _Note: this issue was identified against the code base at commit `13273ef63a283393f8b97bbdcadeb04fa836ecfd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSAMLProvider

## Description
Four distinct net-new root causes in the bootstrapped `iam` emulator, all grounded in failing behavioral checks of this suite. They are independent and must each be closed before this issue is submitted. None of them is the already-open scaffold-stub work: the operations involved (CreateSAMLProvider, GetPolicyVersion, GetUser, ListOpenIDConnectProviders) are all implemented and answer; they answer with the wrong message, or with success where AWS refuses.

=== Cause 1 of 4: CreateSAMLProvider's EntityAlreadyExists message is prefixed with the entity type ===
Failing check: iam.duplicate-create-returns-entityalreadyexists
AWS:      Smithy (com.amazonaws.iam#CreateSAMLProvider) models EntityAlreadyExistsException, awsQueryError code `EntityAlreadyExists`, HTTP 409. For a second CreateSAMLProvider with a Name that already exists in the account, AWS's message template is the fixed text `Provider with name ` + the request's `Name` value + the fixed text ` already exists.` — the only variable part is the caller-supplied Name. AWS does NOT name the provider type in this message (it is the same template SAML and OIDC providers share).
Emulator: returns the right code and status but the message `SAML Provider with name <Name> already exists.` — i.e. it prepends the literal `SAML ` to AWS's fixed text. Replay detail: expected message `Provider with name <name> already exists.`, got `'SAML Provider with name sim2real-behav-saml-f09f61d3e919 already exists.'` (the name there is the check's generated value and is NOT part of the template; it comes from the request).
Why:      Clients and IaC tools that branch on the duplicate-provider message (CloudFormation/Terraform idempotency shims, retry wrappers that match the exact AWS text) fail to recognise the collision and surface it as an unknown failure instead of "already exists".
Fix:      the CreateSAMLProvider handler's duplicate-name rejection path (its error construction in the iam service's create_saml_provider operation handler) must emit AWS's fixed text without the entity-type prefix.
Traces:   iam.duplicate-create-returns-entityalreadyexists (mismatch); other duplicate-create assertions in the same check — user, role, policy — already pass, so this is scoped to the SAML provider path.

=== Cause 2 of 4: GetPolicyVersion's NoSuchEntity message drops AWS's "or is not attachable" clause ===
Failing check: iam.get-nonexistent-entity-returns-nosuchentity
AWS:      Smithy (com.amazonaws.iam#GetPolicyVersion) models NoSuchEntityException, awsQueryError code `NoSuchEntity`, HTTP 404; VersionId is constrained by pattern `^v[1-9][0-9]*(\.[A-Za-z0-9-]*)?$`. For an existing policy ARN with a version id that does not exist, AWS's message template is the fixed text `Policy `, the request's PolicyArn, the fixed text ` version `, the request's VersionId, then the fixed text ` does not exist or is not attachable.` The trailing `or is not attachable` is part of AWS's fixed text, not a per-run value.
Emulator: same code and status, but the message stops at ` does not exist.` Replay detail: expected `'Policy arn:aws:iam::<account>:policy/<name> version v9 does not exist or is not attachable.'`, got `'Policy arn:aws:iam::000000000000:policy/sim2real-behav-policy-b70e126675f0 version v9 does not exist.'` (the account id, policy name and `v9` are request/state values filling the template; the divergence is the missing fixed clause).
Why:      A client that distinguishes "no such version" from "version exists but is not attachable" by the documented AWS text gets a message it cannot classify; message-matching assertions in SDK wrappers and policy tooling break.
Fix:      the GetPolicyVersion handler's not-found error construction (get_policy_version operation handler in the iam service).
Traces:   iam.get-nonexistent-entity-returns-nosuchentity (mismatch). Its sibling NoSuchEntity probes in the same check pass, so this is scoped to GetPolicyVersion's message.

=== Cause 3 of 4: an access key whose Status is Inactive is still accepted by the signer ===
Failing check: iam.inactive-access-key-is-refused-while-its-active-twin-succeeds
AWS:      recorded behavior only for the exact code — the IAM Smithy model describes UpdateAccessKey's Status member but not the authentication outcome, because the refusal is produced by SigV4 credential validation, not by a modeled IAM error. AWS refuses any signed request presented with an access key whose Status is `Inactive`: the response is an AUTHENTICATION failure with error code `InvalidClientTokenId` and the fixed message `The security token included in the request is invalid.` (no variable parts), not an authorization failure. Deactivating a key is documented as making it unusable for API calls while leaving it listed. Hypothesis (flagged by the check itself as unblessed): the precise code is `InvalidClientTokenId` rather than AccessDenied/SignatureDoesNotMatch/UnrecognizedClientException; the emulator must at minimum refuse the call rather than serve it.
Emulator: the request SUCCEEDS. The check creates two access keys for one user, sets the second to `Inactive` (and the emulator's own ListAccessKeys correctly reports one Active and one Inactive), then signs a byte-identical GetUser with each. The Active twin returns 200 (correct) and the Inactive twin ALSO returns HTTP 200 with the full User body — replay detail: `real AWS refuses this call, but it SUCCEEDED and returned {'User': {...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. So the emulator persists the Status field but never consults it when authenticating a signed request.
Why:      This is a credential-revocation bypass. Operators and incident-response tooling deactivate a key as the fast way to cut off a credential without deleting it; against this emulator the deactivated key keeps working, so any test or runbook that validates key deactivation passes while the credential is in fact still live. It also makes the Active/Inactive distinction untestable for every caller.
Fix:      the request authentication path that resolves an access key id to a principal (the emulator's SigV4 credential lookup / signer, shared across services) must reject keys whose stored Status is Inactive with InvalidClientTokenId before dispatching to the operation handler; the iam UpdateAccessKey handler's state write is already correct.
Traces:   iam.inactive-access-key-is-refused-while-its-active-twin-succeeds (mismatch). The check's Active-twin positive control passes, so key material, identity policy and endpoint are healthy and only Status is at issue.

=== Cause 4 of 4: IAM implicit-deny AccessDenied messages omit AWS's authorization-details sentence ===
Failing check: iam.op-contract-list-open-id-connect-providers
AWS:      recorded behavior only (the model carries no message text; ListOpenIDConnectProviders takes no input and models only ServiceFailure, so an implicit deny is the authorization boundary). For a principal with no identity-based policy allowing the action, AWS returns AccessDenied whose message is the fixed template `User: ` + the caller's ARN + ` is not authorized to perform: ` + the action + ` on resource: ` + the resource + ` because no identity-based policy allows the ` + the action + ` action`, FOLLOWED BY the fixed sentence `. Go to https://<region>.console.aws.amazon.com/iam/home?region=<region>#/authorization-details/<authorization id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization id>`. The ARN, action, resource and region come from the request; the authorization id is a per-request value (the recording shows the placeholder `<no authorization id>` where no id was minted) — the requirement is the sentence and its structure, not any one recorded id.
Emulator: emits only the first half and stops after `... action`. Replay detail: expected `'User: arn:aws:iam::000000000000:user/<denied-user> is not authorized to perform: iam:ListOpenIDConnectProviders on resource: arn:aws:iam::000000000000:oidc-provider/* because no identity-based policy allows the iam:ListOpenIDConnectProviders action. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<no authorization id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <no authorization id>'`; got the same text truncated at `... action`.
Why:      Callers that parse the denial for the authorization id (to then call GetRequestAuthorizationDetails) find nothing, and anything asserting on AWS's current denial wording sees a different message. The leading half matching makes this a wording defect in one shared formatter rather than a per-operation gap.
Fix:      the shared IAM implicit-deny AccessDenied message formatter used by the iam authorization layer (the component that builds the `is not authorized to perform` text), so every operation's implicit deny carries the trailing authorization-details sentence.
Traces:   iam.op-contract-list-open-id-connect-providers (mismatch); the check's positive half — a freshly created provider appearing in ListOpenIDConnectProviders — passes.

Related but NOT this issue: iam.e2e-service-role-setup fails because CreatePolicyVersion answers `NotImplemented` instead of `NoSuchEntity`/404; that cause is already carried by the open issue 20261025T000319Z-iam-createpolicyversion-stub.md and is deliberately not bundled here.

## Evidence
Cause 1 — iam.duplicate-create-returns-entityalreadyexists:
  expected: EntityAlreadyExists / 409 / message 'Provider with name <Name> already exists.'
  actual:   EntityAlreadyExists / 409 / message 'SAML Provider with name sim2real-behav-saml-f09f61d3e919 already exists.'
  (AssertionError: saml: unexpected message 'SAML Provider with name sim2real-behav-saml-f09f61d3e919 already exists.')

Cause 2 — iam.get-nonexistent-entity-returns-nosuchentity:
  expected: NoSuchEntity / 404 / 'Policy <PolicyArn> version <VersionId> does not exist or is not attachable.'
  actual:   NoSuchEntity / 404 / 'Policy arn:aws:iam::000000000000:policy/sim2real-behav-policy-b70e126675f0 version v9 does not exist.'
  (AssertionError: GetPolicyVersion: expected message '... does not exist or is not attachable.', got '... does not exist.')

Cause 3 — iam.inactive-access-key-is-refused-while-its-active-twin-succeeds:
  setup (all passing): one user, two access keys, second set Status=Inactive; emulator's ListAccessKeys reports {keyA: Active, keyB: Inactive}; GetUser signed with keyA returns 200.
  expected: GetUser signed with the Inactive key keyB -> InvalidClientTokenId, message 'The security token included in the request is invalid.'
  actual:   HTTP 200 with the full User body —
    AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'User': {'Path': '/', 'UserName': 'sim2real-keystate-user-...', 'UserId': 'AIDA...', 'Arn': 'arn:aws:iam::000000000000:user/...', ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 4 — iam.op-contract-list-open-id-connect-providers:
  expected: AccessDenied message 'User: <caller arn> is not authorized to perform: iam:ListOpenIDConnectProviders on resource: arn:aws:iam::<acct>:oidc-provider/* because no identity-based policy allows the iam:ListOpenIDConnectProviders action. Go to https://<region>.console.aws.amazon.com/iam/home?region=<region>#/authorization-details/<authorization id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization id>'
  actual:   same text truncated after '... allows the iam:ListOpenIDConnectProviders action' — the trailing authorization-details sentence is absent.

## Steps to Reproduce
Boot the emulator and replay these behavioral checks (they are the grounding set for this issue):
  1. iam.duplicate-create-returns-entityalreadyexists — CreateSAMLProvider(Name=N, SAMLMetadataDocument=doc), then CreateSAMLProvider again with the same Name; inspect the EntityAlreadyExists message.
  2. iam.get-nonexistent-entity-returns-nosuchentity — CreatePolicy, then GetPolicyVersion(PolicyArn=<that policy>, VersionId='v9'); inspect the NoSuchEntity message.
  3. iam.inactive-access-key-is-refused-while-its-active-twin-succeeds — create a user with an inline policy allowing iam:GetUser on itself, create two access keys, UpdateAccessKey(Status='Inactive') on the second, then issue the identical signed GetUser with each key; the Inactive one must be refused InvalidClientTokenId, and must succeed again after UpdateAccessKey(Status='Active').
  4. iam.op-contract-list-open-id-connect-providers — CreateOpenIDConnectProvider, confirm it is listed, then call ListOpenIDConnectProviders as a zero-permission IAM user and inspect the AccessDenied message.
All four must pass before submitting.
