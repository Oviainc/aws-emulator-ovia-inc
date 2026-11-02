Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol
Issue-Key: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol|_error_code_and_status#18|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED: {result!r}")
Issue-Commit: 1d2614296c9898201632b19b84efb1eb661245a1

# iam__secretsmanager: Secrets Manager performs no identity-based authorization — ListSecrets as a policy-less IAM user answers 200 with an empty SecretList instead of 400 AccessDeniedException

> _Note: this issue was identified against the code base at commit `1d2614296c9898201632b19b84efb1eb661245a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecrets

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: Secrets Manager never evaluates the caller's identity-based policy, so an unauthorized principal is served a successful response ===

AWS: Secrets Manager authorizes every API call against the calling principal's identity-based policy before the operation runs. An IAM user that holds no policy granting secretsmanager:ListSecrets is refused; because Secrets Manager speaks awsJson1_1, the denial is folded onto HTTP 400 (not 403) and carries the error code `AccessDeniedException`. Smithy: ListSecrets (com.amazonaws.secretsmanager#secretsmanager, awsJson1_1) models only InternalServiceError (500), InvalidNextTokenException (400), InvalidParameterException (400) and InvalidRequestException (400) — AccessDeniedException is the protocol/runtime denial error, not an operation-modelled one, so the requirement here is the code+status pair `AccessDeniedException` / HTTP 400. The denial message is request-derived (principal ARN + the action being denied), of the form `User: arn:aws:iam::<account>:user/<name> is not authorized to perform: secretsmanager:ListSecrets because no identity-based policy allows the secretsmanager:ListSecrets action`; that construction rule is the requirement, the exact recorded sentence is only an example. The same recording shows IAM's own awsQuery ListUsers denial for the identical principal arriving as `AccessDenied` at HTTP 403 — that half of the check passes, so the protocol-dependent status difference is observable only because Secrets Manager answers at all.

Emulator: the call SUCCEEDS. Signing ListSecrets(MaxResults=1) with the freshly minted IAM user's access key (a user created with no attached or inline policy) returns HTTP 200 with `{'SecretList': []}` and content-type application/x-amz-json-1.1. No authorization decision is taken at any point: the access key is accepted, the principal is resolved to nothing in particular, and the handler runs to completion. The check fails at its own guard `expected the call to be denied, but it SUCCEEDED`.

Why: a client relying on AWS's behavior gets a silent, total loss of the access-control boundary. An unauthorized caller cannot distinguish "you may not list secrets" from "this account holds no secrets" — both are 200 with an empty list — so least-privilege tests, deny-by-default assertions and negative IAM test suites all pass against an emulator that admits every principal. Error-handling code paths that branch on AccessDeniedException/400 are never exercised, and a cross-protocol expectation (403 from awsQuery services vs 400 from awsJson1_1 services) cannot be validated at all.

Fix: the Secrets Manager service's request-authorization layer — resolve the signing principal from the SigV4 credential, evaluate its identity-based policy for the requested `secretsmanager:<Action>` before dispatching to the operation handler, and when no statement allows the action, serialize an awsJson1_1 `AccessDeniedException` at HTTP 400 with the principal/action-derived message. This is the deny-by-default gate for every operation, not just ListSecrets; ListSecrets is simply where the suite observes it. Open issue 20261024T190718Z-secretsmanager-secrets-manager-enforces-no-authorization-and-never-consults.md (Cause 2) describes the neighbouring gap for Secrets Manager *condition keys* (aws:RequestTag, aws:ResourceTag, secretsmanager:VersionStage) and does not carry this check's replay key; this cause is the more basic one — no identity policy is consulted even when there is no condition to evaluate and no policy granting the action exists at all.

Traces: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol.

## Evidence
Check iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol [mismatch]:

Expected (AWS): ListSecrets signed by an IAM user with no identity-based policy is refused with Error.Code == 'AccessDeniedException' at HTTPStatusCode 400, message of the form 'User: arn:aws:iam::<account>:user/<name> is not authorized to perform: secretsmanager:ListSecrets because no identity-based policy allows the secretsmanager:ListSecrets action'.

Actual (emulator): the call returned successfully —
  AssertionError: expected the call to be denied, but it SUCCEEDED: {'SecretList': [], 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '17', ...}, 'RetryAttempts': 0}}
raised at check line 117 -> _error_code_and_status(lambda: as_denied_sm.list_secrets(MaxResults=1)).

The IAM leg of the same check, with the identical principal and credentials, did reach its assertions (iam:ListUsers denial as 'AccessDenied' at HTTP 403), so the failure is isolated to Secrets Manager not taking an authorization decision — not to credential propagation or to the minted user.

## Steps to Reproduce
1. With the account's own credentials, confirm the positive controls answer: iam ListUsers(MaxItems=1) and secretsmanager ListSecrets(MaxResults=1) both succeed.
2. iam CreateUser(UserName=<fresh name>) — attach NO policy, inline or managed.
3. iam CreateAccessKey(UserName=<that user>) and build a secretsmanager client signing with that access key id / secret access key (plain access key, no session token).
4. As that principal, call secretsmanager ListSecrets(MaxResults=1).
   Expected: ClientError with Error.Code 'AccessDeniedException' and ResponseMetadata.HTTPStatusCode 400.
   Observed: HTTP 200 with body {'SecretList': []}.
5. For contrast, as the same principal call iam ListUsers(MaxItems=1): this is correctly refused with 'AccessDenied' at HTTP 403, which is what makes the Secrets Manager 200 a divergence rather than a credential problem.
