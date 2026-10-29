Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: uxc.update-denied-without-iam-permission-rejected
Issue-Shape-Id: com.amazonaws.uxc#AWSAccountUXSetting
Issue-Key: uxc.update-denied-without-iam-permission-rejected|_wait_for_denied#16|AssertionError|raise AssertionError(
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# uxc authorizes every sigv4 caller: UpdateAccountCustomizations succeeds for an IAM identity with no uxc: permissions instead of AccessDeniedException (403)

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccountCustomizations

## Description
One net-new root cause in the uxc (AWSAccountUXSetting, restJson1) control plane: the service performs NO identity-based authorization. Every request that presents valid sigv4 credentials is served as if it were fully privileged, so a mutating control-plane call from an unprivileged IAM identity is accepted and its effect persisted.

Cause 1 of 1: UpdateAccountCustomizations is not authorized against the calling identity's IAM permissions, so an identity with no attached policy mutates the account-singleton setting instead of being refused.

AWS: UpdateAccountCustomizations models AccessDeniedException as a client error with HTTP 403 and a REQUIRED `message` member (Smithy: UpdateAccountCustomizations errors = AccessDeniedException(403), InternalServerException(500), ThrottlingException(429), ValidationException(400); AccessDeniedException.message @required). The model documents the error as "You don't have sufficient access to perform this operation. Verify that your IAM policy includes the required `uxc:` permissions for the operation that you are calling." The service is sigv4-authenticated (service_info: authSchemes [sigv4], sigv4Name `uxc`, arnNamespace `uxc`), so the signing identity is the identity to authorize. The wire message AWS returns for the no-policy-at-all class is the standard IAM implicit-deny sentence; its FIXED text is `User: ` / ` is not authorized to perform: ` / ` on resource: ` / ` because no identity-based policy allows the ` / ` action`, and its VARIABLE parts come from the request and from state: the caller's principal ARN (`arn:aws:iam::<account-id>:user/<user-name>`, the account and user of the signing access key), the action (`uxc:UpdateAccountCustomizations`, the service's arnNamespace plus the operation name), the resource ARN of the account-singleton being written (`arn:aws:uxc::<account-id>:account-customizations`), and the action again in the trailing clause. No literal account id, user name or generated value from any one run is part of the requirement — only that template, filled from the request and the emulator's own IAM state.

Emulator: the call SUCCEEDS. The behavioral check creates a fresh IAM user with no attached policies, mints an access key for it, and calls UpdateAccountCustomizations(accountColor='teal') with those credentials repeatedly for 45s; every attempt returns 200 with the updated body and never any error — the replay reports `never observed AccessDeniedException within 45.0s; last observed='Success'`. The handler has no authorization step at all: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs::handle goes straight from `sent_as_null` reads to `crate::state::service_state` and `crate::state::calling_account_id()`, merges the supplied settings into the record and answers from persisted state; the only refusal it owns is the catalog-membership ValidationException. The caller's principal is never consulted, and the modeled `AccessDeniedException` variant on `UpdateAccountCustomizationsErrorModeled` is never constructed anywhere in the operation.

Why: this is an overly-permissive divergence, the most damaging direction for an emulator. A client, test suite or policy-verification pipeline that validates least-privilege behavior against AWS — asserting that an unprivileged role CANNOT change console customizations, or exercising its 403 handling and retry/escalation path — passes against the emulator and then fails or, worse, silently mutates shared account state against real AWS. Negative-path code (catching AccessDeniedException, surfacing the IAM remediation message, falling back to a privileged principal) is never exercised, and the effect-absence guarantee AWS gives (a denied mutation leaves the prior value intact) does not hold here: the setting is actually written by a principal that may not touch it.

Fix: an identity-based authorization step in the uxc service's request path — the service front door (services/service-com-amazonaws-uxc-awsaccountuxsetting/src/service.rs, the same layer that already refuses shape violations before the handler runs) or an equivalent pre-handler check in the operation, resolving the sigv4 signing identity to its IAM principal and permissions and returning the already-modeled AccessDeniedException (HTTP 403) with the implicit-deny message template above when no identity-based policy allows `uxc:<Operation>`. The refusal must be taken BEFORE the state write so the persisted setting is unchanged, and it belongs to all three uxc operations (GetAccountCustomizations and ListServices model the same error), not only to the one the check exercises. Not a serializer change: the error shape and status are already modeled correctly and the privileged positive control replays clean.

This cause is not covered by either open uxc issue: 20261029T184328Z-uxc-response-bodies-diverge-from-the-recorded-service is about 200-status array content (ListServices catalog membership and the visibleServices echo order), and 20261027T134113Z-uxc-updateaccountcustomizations-maintainability is about duplicated front-door helpers. Neither carries an authorization/403 replay key.

Traces: uxc.update-denied-without-iam-permission-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 18/19 passed (94.7%), 1 mismatched, 0 errored, 0 skipped, bootstrap_signal=false. The single failure is service `uxc`, verdict `mismatch`, not marked ALREADY FILED and not OWNED by any open issue.

[mismatch] uxc.update-denied-without-iam-permission-rejected (group `overly-permissive`, tags axis:rejection, concern:negative-classes, plane:control, resource:account-singleton:uxc-account-customizations):
  File "check_uxc_update_denied_without_iam_permission_rejected.py", line 41, in run
    code, message = _wait_for_denied(...)
  File "check_uxc_update_denied_without_iam_permission_rejected.py", line 85, in _wait_for_denied
    raise AssertionError(
  AssertionError: never observed AccessDeniedException within 45.0s; last observed='Success'

Expected (AWS): UpdateAccountCustomizations(accountColor='teal') signed by the freshly created, policy-less IAM user fails with HTTP 403, Error.Code == 'AccessDeniedException', and Error.Message ==
  "User: arn:aws:iam::<account-id>:user/<user-name> is not authorized to perform: uxc:UpdateAccountCustomizations on resource: arn:aws:uxc::<account-id>:account-customizations because no identity-based policy allows the uxc:UpdateAccountCustomizations action"
where the bracketed parts are the signing identity's account and user name from the request/IAM state and everything else is the fixed text of this refusal class. GetAccountCustomizations after the denial still reports the baseline colour 'yellow' (effect absent).

Actual (emulator): every attempt over a 45s / 3s-interval poll returns HTTP 200 with the mutated body; no ClientError is ever raised, so the loop's last observation stays 'Success'. The unprivileged write is persisted. The privileged positive control (same call on the default credentials, then GetAccountCustomizations) passes, so the operation itself works — only the authorization decision is missing.

Emulator source: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs, `impl Operation::handle` (lines 186-303) — no principal/permission check between entry and the `state.lock()` write; `UpdateAccountCustomizationsErrorModeled::AccessDeniedException` (declared line 134) is never constructed. The IAM calls the check uses to set up the unprivileged principal (CreateUser, CreateAccessKey) all succeed, so the identity exists in the emulator's IAM state and is simply never authorized against.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator (uxc is bootstrapped: service-com-amazonaws-uxc-awsaccountuxsetting is in get_bootstrap_status().bootstrapped).
2. With default/privileged credentials: uxc.update_account_customizations(accountColor='yellow'), then uxc.get_account_customizations() -> accountColor 'yellow' (baseline).
3. iam.create_user(UserName=<fresh name>) and iam.create_access_key(UserName=<that name>); attach NO policies to the user.
4. Build a uxc client from that access key id / secret and call update_account_customizations(accountColor='teal').
5. Observed: HTTP 200 and a body echoing accountColor 'teal'; repeating the call for 45s never yields an error. Expected: HTTP 403 AccessDeniedException with the implicit-deny message template above, and uxc.get_account_customizations() still reporting the baseline 'yellow'.
6. Positive control (must keep passing): the same update from the privileged identity returns accountColor 'teal' and GetAccountCustomizations reports 'teal'.
7. Equivalent replay: run the behavioral suite and re-run check uxc.update-denied-without-iam-permission-rejected.
