Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.op-contract-get-account-configuration acm.op-contract-put-account-configuration
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.op-contract-get-account-configuration|_error#9|AssertionError|raise AssertionError(f"expected AccessDeniedException, but the call SUCCEEDED and returned {result!r}")
Issue-Key: acm.op-contract-put-account-configuration|_error#11|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# ACM serves calls from a principal with no permissions: GetAccountConfiguration/PutAccountConfiguration answer 200 where AWS answers AccessDeniedException

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountConfiguration

## Description
One root cause, grounded in two checks. Both operations answer correctly for the ambient credentials (the positive legs pass: GetAccountConfiguration is a stable read returning ExpiryEvents.DaysBeforeExpiry=45, and PutAccountConfiguration is accepted), so this is not a stub; what is missing is authorization of the caller.

Cause 1 of 1: identity-based authorization is not enforced on the ACM account-configuration operations.
AWS: Smithy models AccessDeniedException (HTTP 403) on both GetAccountConfiguration (input Unit -> {ExpiryEvents}) and PutAccountConfiguration ({ExpiryEvents, IdempotencyToken required len 1..32 pattern ^\\w+$} -> Unit). A session for a role with no attached policies hits IAM's implicit deny, so either call made with that session's credentials is refused with 403 AccessDeniedException, message (built from the request's principal and action) 'User: <assumed-role arn> is not authorized to perform: acm:GetAccountConfiguration because no identity-based policy allows the acm:GetAccountConfiguration action'. The caller's own credentials are unaffected.
Emulator: the same calls, signed with credentials from sts:AssumeRole on a freshly created role that has no policies, SUCCEED — GetAccountConfiguration returns HTTP 200 {'ExpiryEvents': {'DaysBeforeExpiry': 45}} and PutAccountConfiguration returns 200. The emulator serves every signed request as if it were the account's root principal.
Why: overly permissive. Every negative authorization contract is unobservable, so a deployment whose role is missing acm:GetAccountConfiguration / acm:PutAccountConfiguration passes in test and is denied in production — the most expensive class of false green. It also means least-privilege policy work cannot be validated against the emulator at all.
Fix: the request-authorization layer the ACM handlers are invoked through (evaluate the signing principal's IAM identity-based policies before dispatch and raise the modeled AccessDeniedException), not the two operations' business logic — services/service-com-amazonaws-acm-certificatemanager/src/ops/get_account_configuration.rs and put_account_configuration.rs only need to surface the denial.
Traces: acm.op-contract-get-account-configuration, acm.op-contract-put-account-configuration.

## Evidence
acm.op-contract-get-account-configuration — AssertionError: expected AccessDeniedException, but the call SUCCEEDED and returned {'ExpiryEvents': {'DaysBeforeExpiry': 45}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. The denied client is built by the check's _acm_client_for_permissionless_role: iam.create_role(<no policies attached>) then sts.assume_role(RoleArn=<that role>), and the ACM client is constructed with those temporary credentials.
acm.op-contract-put-account-configuration — AssertionError: expected the call to be rejected, but it succeeded, for denied_acm.put_account_configuration(ExpiryEvents={'DaysBeforeExpiry': 15}, IdempotencyToken=<24 hex>).
Smithy citations: GetAccountConfiguration errors [AccessDeniedException(403, awsQueryError AccessDenied), ThrottlingException(400)]; PutAccountConfiguration errors [AccessDeniedException(403), ConflictException(400), ThrottlingException(400), ValidationException(400)].
The exact message text is quoted as an example; the requirement is HTTP 403 with Error.Code 'AccessDeniedException' for a principal with no identity-based policy allowing the action.

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. iam.create_role(RoleName='x', AssumeRolePolicyDocument=<trust the account root>) and attach NO policies; creds = sts.assume_role(RoleArn=<that role>, RoleSessionName='s')['Credentials'].
3. Build an ACM client from those credentials and call get_account_configuration() — HTTP 200 with ExpiryEvents instead of 403 AccessDeniedException.
4. With the same client call put_account_configuration(ExpiryEvents={'DaysBeforeExpiry': 15}, IdempotencyToken=<hex>) — HTTP 200 instead of 403 AccessDeniedException.
5. Re-replay the two checks in trace_ids.
