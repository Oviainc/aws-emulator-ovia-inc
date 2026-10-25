Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.resource-policy-cross-account-access-enforced-denied
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.resource-policy-cross-account-access-enforced-denied|_error#29|AssertionError|raise AssertionError(f"expected the call to be denied, but it succeeded: {result!r}")
Issue-Commit: 0618b8792443df0dc6dd007b51a973b8896c970f

# bedrock GetGuardrail ignores identity-based IAM policy: an assumed role with an explicit Deny on bedrock:GetGuardrail is served the guardrail with HTTP 200 instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `0618b8792443df0dc6dd007b51a973b8896c970f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGuardrail

## Description
One net-new root cause found on the Bedrock control plane by the behavioral suite.

=== Cause 1 of 1: GetGuardrail performs no identity-based authorization evaluation, so a principal explicitly denied the action still reads the guardrail ===

AWS: Smithy (com.amazonaws.bedrock#AmazonBedrockControlPlaneService) models GetGuardrail as GET /guardrails/{guardrailIdentifier}, success code 200, with errors AccessDeniedException (httpError 403), ResourceNotFoundException (404), ValidationException (400), ThrottlingException (429), InternalServerException (500). AWS evaluates the caller's identity-based policy on every control-plane read: an explicit Deny in an identity-based policy is terminal and overrides every other statement, including any resource-based policy Allow. A guardrail resource policy that names the account root principal does NOT, by itself, grant a specific principal inside that account access — same-account access still requires an identity-based Allow and is defeated by an identity-based Deny. So a role whose inline policy carries {"Effect":"Deny","Action":"bedrock:GetGuardrail","Resource":"*"} must be refused with HTTP 403 / error code AccessDeniedException, and no guardrail body may be returned. The denial message is built from the request, so its rule is the requirement (not a recorded string): "User: <assumed-role principal ARN> is not authorized to perform: bedrock:GetGuardrail on resource: <guardrail ARN> with an explicit deny in an identity-based policy" — the principal ARN, the action and the resource ARN are taken from the request, and the trailing clause names the explicit-deny reason that distinguishes it from an implicit denial.

Emulator: the same GetGuardrail call, made with credentials from sts:AssumeRole on that explicitly-denied role, returns HTTP 200 and the complete guardrail representation (name, guardrailId, guardrailArn, version DRAFT, status READY, wordPolicy, createdAt/updatedAt, blockedInputMessaging/blockedOutputsMessaging). No AccessDeniedException is ever produced: the check polls for the code for 30s and every attempt succeeds. The replay reports: AssertionError: expected the call to be denied, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'guardrailId': ..., 'status': 'READY', ...}. Everything up to that point agrees with AWS — CreateGuardrail reaches READY, CreateGuardrailVersion publishes, PutResourcePolicy stores the documented account-root/{bedrock:ApplyGuardrail,bedrock:GetGuardrail} statement and GetResourcePolicy round-trips it byte-for-byte — so the divergence is isolated to authorization evaluation on the read, not to the resource-policy surface.

Why: a client relying on AWS's authorization semantics gets the opposite answer. Least-privilege validation and negative-authorization tests (the whole class of "this role must NOT be able to read this guardrail") pass against the emulator and fail in production; worse, an emulator that treats "a resource policy nominally covering this account exists" (or simply nothing at all) as sufficient hides a real over-permissiveness bug in the policy under test. Code that branches on AccessDeniedException — to fall back to a different credential, to surface a permissions error, or to assert an IAM boundary in CI — never takes that branch, and a guardrail's configuration (including its blocked-messaging strings and word policy, modeled as sensitive) is disclosed to a principal AWS would have refused.

Fix: the request-authorization layer in front of the bedrock control-plane handlers — the component that should evaluate the caller's identity (including sts:AssumeRole session principals) against identity-based policies before the operation runs — plus GetGuardrail's handler, so the modeled AccessDeniedException is returned with status 403 and no guardrail body when the caller's identity carries an explicit Deny (or lacks an Allow) for bedrock:GetGuardrail.

Related open issue, which does NOT cover this: 20261024T025841Z-bedrock-control-plane-no-iam-authorization-gate-guardrail.md. Its Cause 1 and Cause 2 name DeleteModelInvocationLoggingConfiguration and CreateCustomModel only, its Issue-Trace-Ids do not include this check, and this failure was not reported as owned by it. It is plausible that one shared authorization layer underlies both, so a fixer should check that issue's causes too — but this check's replay key (GetGuardrail on an assumed role with an explicit Deny, with a guardrail resource policy present) is carried by no open issue.

Traces: bedrock.resource-policy-cross-account-access-enforced-denied; handler services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/get_guardrail.rs.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1), bedrock 21/76 passed. Check bedrock.resource-policy-cross-account-access-enforced-denied [mismatch].

Expected (AWS): HTTP 403, error code AccessDeniedException, no guardrail body. Message per AWS's rule: "User: arn:aws:sts::<account>:assumed-role/<role-name>/sim2real-rbp-session is not authorized to perform: bedrock:GetGuardrail on resource: <guardrail ARN> with an explicit deny in an identity-based policy".

Actual (emulator): HTTP 200 with the full guardrail document. Verbatim from the replay:

  File "check_bedrock_resource_policy_cross_account_access_enforced_denied.py", line 206, in _error
    raise AssertionError(f"expected the call to be denied, but it succeeded: {result!r}")
  AssertionError: expected the call to be denied, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '515', ...}, 'RetryAttempts': 0}, 'name': 'sim2real-rbp-gr-<suffix>', 'guardrailId': '<id>', 'guardrailArn': 'arn:aws:bedrock:<region>:<account>:guardrail/<id>', 'version': 'DRAFT', 'status': 'READY', 'wordPolicy': {'words': [{'text': '<bannedword>'}], 'managedWordLists': []}, 'createdAt': datetime(...), 'updatedAt': datetime(...), 'statusReasons': [], 'failureRecommendations': [], 'blockedInputMessaging': 'Input blocked by test guardrail.', 'blockedOutputsMessaging': 'Output blocked by test guardrail.'}

(Generated names, ids, timestamps and the account id above are per-run values, not requirements; the requirement is status 403 + code AccessDeniedException instead of 200 + a body.)

The denial is polled for up to 30s (to absorb IAM propagation lag) and the call succeeds on every attempt, so this is a missing check rather than a propagation race. The preceding legs of the same check pass against the emulator: CreateGuardrail -> READY, CreateGuardrailVersion -> READY, PutResourcePolicy accepted, and GetResourcePolicy returned the stored document equal to what was put.

## Steps to Reproduce
Boot the emulator from this checkout and replay bedrock.resource-policy-cross-account-access-enforced-denied (suite simparity-behavioral-ga-prod-v1). Manually:

1. sts:GetCallerIdentity to learn <account>.
2. bedrock:CreateGuardrail with blockedInputMessaging/blockedOutputsMessaging and wordPolicyConfig.wordsConfig=[{text:"<word>"}]; poll GetGuardrail until status == READY; CreateGuardrailVersion and poll that version to READY.
3. bedrock:PutResourcePolicy on the guardrail ARN with {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":["bedrock:ApplyGuardrail","bedrock:GetGuardrail"],"Resource":"<guardrail ARN>"}]} (accepted today), then GetResourcePolicy to confirm the round trip.
4. iam:CreateRole with a trust policy allowing arn:aws:iam::<account>:root to sts:AssumeRole, then iam:PutRolePolicy with {"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"bedrock:GetGuardrail","Resource":"*"}]}.
5. sts:AssumeRole into that role and call bedrock:GetGuardrail(guardrailIdentifier=<guardrailId>) with the session credentials.

Expect HTTP 403 with error code AccessDeniedException and no guardrail body. Observe HTTP 200 with the full guardrail document. Positive control for the fix: the identical call from a second role whose inline policy Allows bedrock:GetGuardrail must still succeed with 200.
