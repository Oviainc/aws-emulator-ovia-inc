Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: signin.console-authorization-configuration-disabled-after-delete signin.console-authorization-configuration-full-lifecycle-workflow signin.get-console-authorization-configuration-not-found-when-unset signin.delete-resource-permission-statement-invalid-sid-rejected signin.get-resource-policy-not-found-without-statements signin.list-resource-permission-statements-baseline-empty-shape signin.put-console-authorization-configuration-double-put-conflict-rejected signin.put-console-authorization-configuration-nonexistent-org-rejected signin.put-resource-permission-statement-access-denied-without-permission signin.put-resource-permission-statement-concurrent-same-token-idempotent
Issue-Shape-Id: com.amazonaws.signin#Signin
Issue-Key: signin.console-authorization-configuration-disabled-after-delete|run#12|AssertionError|assert message == NOT_FOUND_MESSAGE, (
Issue-Key: signin.console-authorization-configuration-full-lifecycle-workflow|run#10|AssertionError|assert message == NOT_FOUND_MESSAGE, (
Issue-Key: signin.get-console-authorization-configuration-not-found-when-unset|run#13|AssertionError|assert message == NOT_FOUND_MESSAGE, (
Issue-Key: signin.delete-resource-permission-statement-invalid-sid-rejected|run#6|AssertionError|assert malformed_message == expected_malformed, (
Issue-Key: signin.get-resource-policy-not-found-without-statements|_error#2|AssertionError|raise AssertionError(f"expected ResourceNotFoundException, but the call SUCCEEDED and returned {result!r}")
Issue-Key: signin.list-resource-permission-statements-baseline-empty-shape|_error#2|AssertionError|raise AssertionError(f"expected ResourceNotFoundException, but the call SUCCEEDED and returned {result!r}")
Issue-Key: signin.put-console-authorization-configuration-double-put-conflict-rejected|_error_code#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: signin.put-console-authorization-configuration-nonexistent-org-rejected|_error#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: signin.put-resource-permission-statement-access-denied-without-permission|_error_code_and_message#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: signin.put-resource-permission-statement-concurrent-same-token-idempotent|_error#17|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Commit: 3f55d2ad9ebbeca04482102aa29f3c57523b0c22

# signin: console-authorization and resource-policy refusals are missing or misworded — absent policy reads as empty, double Put/foreign-org Put/denied Put all answer 200, and two refusal messages are not AWS's

> _Note: this issue was identified against the code base at commit `3f55d2ad9ebbeca04482102aa29f3c57523b0c22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConsoleAuthorizationConfiguration,GetConsoleAuthorizationConfiguration,DeleteConsoleAuthorizationConfiguration,GetResourcePolicy,ListResourcePermissionStatements,PutResourcePermissionStatement,DeleteResourcePermissionStatement

## Description
This issue bundles 7 distinct root causes in the signin emulator, all reached now that the resource-policy and console-authorization operations answer at all. NOTE: the open issues 20261029T191246Z-signin-resource-policy-and-console-authorization-operations-are.md and 20261102T032217Z-signin-resource-policy-and-console-authorization-operations-are-0875c7.md report those operations as UNREACHABLE (wrong announced host). They do not cover anything below: in this run every operation below was reached and answered a 2xx or a 4xx body, and the divergence is in that answer. A fixer must close all 7 causes before submitting.

=== Cause 1 of 7: ResourceNotFoundException message for a missing console-authorization configuration is written by the emulator instead of AWS's fixed text ===
AWS: Smithy: GetConsoleAuthorizationConfiguration / DeleteConsoleAuthorizationConfiguration both model ResourceNotFoundException (errorKind client, httpError 404, required members `error` (OAuth2ErrorCode, RESOURCE_NOT_FOUND) and `message`). Recorded behavior: reading or deleting a target that has no configuration answers 404 ResourceNotFoundException with the message `Console authorization configuration not found` — fixed text in full, with no variable part (the targetId is NOT interpolated).
Emulator: answers ResourceNotFoundException (code correct) but with its own sentence `no console authorization configuration for targetId <targetId>` — lowercase, differently worded, and interpolating the request's targetId where AWS interpolates nothing.
Why: a client that branches on, logs or asserts the refusal text (and any human reading it) sees a message AWS never emits; message-matching error handling and golden-output tests pass against AWS and fail against the emulator, for both the Get-after-Delete and the second-Delete paths.
Fix: the signin crate's console-authorization not-found error construction (shared by the GetConsoleAuthorizationConfiguration and DeleteConsoleAuthorizationConfiguration handlers) must emit the fixed AWS text and stop interpolating targetId.
Traces: signin.console-authorization-configuration-disabled-after-delete, signin.console-authorization-configuration-full-lifecycle-workflow, signin.get-console-authorization-configuration-not-found-when-unset.

=== Cause 2 of 7: the constraint-violation ValidationException message quotes the anchored regex source instead of AWS's pattern text ===
AWS: Smithy: DeleteResourcePermissionStatement.statementId targets StatementId with @pattern `^[A-Za-z0-9+/]{64}=?$`. AWS's constraint message renders the pattern WITHOUT the `^`/`$` anchors: `1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9+/]{64}=?`. Fixed text: `1 validation error detected: Value '` … `' at '` … `' failed to satisfy constraint: Member must satisfy regular expression pattern: ` plus the unanchored pattern; variable parts: the rejected value from the request and the member name.
Emulator: emits the same sentence but with the raw anchored pattern `^[A-Za-z0-9+/]{64}=?$`.
Why: every pattern-constrained member of this service reports a constraint AWS does not publish; clients and tests that compare the documented pattern or parse the message to recover the accepted form get a regex that differs from AWS's for all such members, not just statementId.
Fix: the signin crate's input-constraint validation / message formatter must strip the leading `^` and trailing `$` when rendering a @pattern into a validation message.
Traces: signin.delete-resource-permission-statement-invalid-sid-rejected.

=== Cause 3 of 7: GetResourcePolicy answers 200 with an empty policy where AWS refuses with ResourceNotFoundException ===
AWS: Smithy: GetResourcePolicy models exactly one operation error, ResourceNotFoundException (client, httpError 404), and its output member signinResourceBasedPolicy is @required. Recorded behavior: an account with no AWS Sign-In resource-based policy gets 404 ResourceNotFoundException with message `Requested resource not found` — fixed text, no variable part. The absence of a policy is a missing resource, not an empty one.
Emulator: answers HTTP 200 with `{"signinResourceBasedPolicy": {"version": "2012-10-17", "statement": []}}`, synthesising a policy document that was never created.
Why: a client that uses GetResourcePolicy to decide whether a Sign-In policy exists concludes one exists (and that it is an explicit allow-nothing policy) when none does; code that only handles ResourceNotFoundException for the unconfigured case never takes that branch, and policy-diff/drift logic sees a phantom empty policy.
Fix: the GetResourcePolicy handler plus the signin resource-policy state lookup — a missing policy record must raise the 404 ResourceNotFoundException rather than defaulting to an empty SigninResourceBasedPolicy.
Traces: signin.get-resource-policy-not-found-without-statements.

=== Cause 4 of 7: ListResourcePermissionStatements answers 200 with an empty page where AWS refuses with ResourceNotFoundException ===
AWS: Smithy: ListResourcePermissionStatements models ResourceNotFoundException (client, httpError 404) and ValidationException; its output member permissionStatements is @required. Recorded behavior: with no resource-based policy on the account the call is refused 404 ResourceNotFoundException, message `Requested resource not found` (fixed text, no variable part), rather than returning a page.
Emulator: answers HTTP 200 with `{"permissionStatements": []}`.
Why: the list operation is indistinguishable between 'policy exists and has no statements' and 'no policy at all', so a client cannot tell whether to create a policy; a caller that treats the 404 as the empty-state signal loops or reports configured-but-empty forever. It also makes the emulator unable to express the pre-creation state at all.
Fix: the ListResourcePermissionStatements handler plus the shared resource-policy state lookup — refuse with the 404 when no policy record exists.
Traces: signin.list-resource-permission-statements-baseline-empty-shape.

=== Cause 5 of 7: a second PutConsoleAuthorizationConfiguration on an already-enabled target succeeds instead of conflicting ===
AWS: Smithy: PutConsoleAuthorizationConfiguration models ConflictException (client, httpError 409, doc: 'Error thrown when request conflicts with current state'). Recorded behavior: re-Putting a target whose configuration is already enabled is refused 409 ConflictException with message `Configuration already exists` — fixed text, no variable part — and the existing configuration stays enabled; after a Delete clears that state, the identical Put succeeds again, so the conflict tracks the enabled STATE and not the targetId.
Emulator: the second Put answers HTTP 200 with `{targetId, scope: "ACCOUNT", consoleAuthorizationEnabled: true}`, i.e. it is a plain upsert and ConflictException is never raised.
Why: a client that relies on Put as a create-once guard (and on ConflictException to detect that another actor already enabled the target) silently overwrites the existing configuration and loses the signal; retry logic that distinguishes 'I enabled it' from 'it was already enabled' can no longer do so.
Fix: the PutConsoleAuthorizationConfiguration handler must consult the console-authorization state and refuse with ConflictException when an enabled record already exists for the target.
Traces: signin.put-console-authorization-configuration-double-put-conflict-rejected.

=== Cause 6 of 7: Put/Get ConsoleAuthorizationConfiguration for an organization the caller does not own are accepted instead of denied ===
AWS: Smithy: targetId accepts `^(\\d{12}|o-[a-z0-9]{10}|r-[0-9a-z]{4,32})$`, so an organization id is well-formed input; AccessDeniedException is a service-common error (client, INSUFFICIENT_PERMISSIONS). Recorded behavior: a well-formed but foreign/nonexistent organization id (`o-` form) is refused on Put AND on the subsequent Get with AccessDeniedException and message `Caller is not authorized for this organization` — fixed text, no variable part — and no configuration is created for it.
Emulator: the Put answers HTTP 200 with `{targetId: "<org id>", scope: "ORGANIZATION", consoleAuthorizationEnabled: true}`, creating a record for an organization the caller has no authority over; the authorization check is absent on both the write and the read path.
Why: the emulator cannot reproduce the cross-tenant boundary at all. Code paths and tests that exist to prove a caller cannot enable console authorization for someone else's organization pass vacuously, and a client sees a configuration that AWS would never have created.
Fix: the PutConsoleAuthorizationConfiguration and GetConsoleAuthorizationConfiguration handlers need the organization-ownership authorization check ahead of the state write/read, emitting AccessDeniedException for an `o-`/`r-` target the caller is not authorized for.
Traces: signin.put-console-authorization-configuration-nonexistent-org-rejected.

=== Cause 7 of 7: PutResourcePermissionStatement is never denied — it creates a statement where AWS refuses the action outright ===
AWS: Smithy: PutResourcePermissionStatement models ConflictException, ServiceQuotaExceededException and ValidationException, with AccessDeniedException as a service-common error. Recorded behavior: the call is refused with AccessDeniedException and message `No Access to PutResourcePermissionStatement API` — fixed text, no variable part — and NO statement is created; the refusal is specific to this action, since ListResourcePermissionStatements for the same caller is not denied, and it still applies to a principal holding an explicit IAM Allow on signin:PutResourcePermissionStatement (proven live by a reaching List call) and when a clientToken is supplied.
Emulator: answers HTTP 200 with `{"statementId": "<64-char id>"}` and persists the statement, under both the default caller and the explicitly-allowed IAM user.
Why: a client gets a resource-policy statement that AWS would never have written, so the emulator's Sign-In resource policy diverges from the real one from the first write onward, and every downstream read (GetResourcePolicy, ListResourcePermissionStatements) reports a statement that cannot exist. Callers that handle the documented denial never exercise it.
Fix: the PutResourcePermissionStatement handler must refuse with AccessDeniedException and that exact message before touching the resource-policy state, independently of the caller's IAM grants and of clientToken.
Traces: signin.put-resource-permission-statement-access-denied-without-permission, signin.put-resource-permission-statement-concurrent-same-token-idempotent.

## Evidence
Suite: 9/19 signin checks passed; all 10 failures are verdict 'mismatch' (the emulator answered; nothing errored, skipped or was unloadable). None was marked ALREADY FILED or OWNED by.

Cause 1 — expected message 'Console authorization configuration not found' reading a deleted configuration, got 'no console authorization configuration for targetId 000000000000' (same assertion in all three checks; code ResourceNotFoundException was correct).

Cause 2 — expected "1 validation error detected: Value 'not-a-valid-sid!!' at 'statementId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9+/]{64}=?", got "... regular expression pattern: ^[A-Za-z0-9+/]{64}=?$" (anchors leaked into the message).

Cause 3 — expected 404 ResourceNotFoundException / 'Requested resource not found'; GetResourcePolicy SUCCEEDED: HTTPStatusCode 200, body {'signinResourceBasedPolicy': {'version': '2012-10-17', 'statement': []}}.

Cause 4 — expected 404 ResourceNotFoundException / 'Requested resource not found'; ListResourcePermissionStatements(maxResults=25) SUCCEEDED: HTTPStatusCode 200, body {'permissionStatements': []}.

Cause 5 — expected 409 ConflictException / 'Configuration already exists' on the second Put of an already-enabled target; it SUCCEEDED: HTTPStatusCode 200, body {'targetId': <account id>, 'scope': 'ACCOUNT', 'consoleAuthorizationEnabled': True}.

Cause 6 — expected AccessDeniedException / 'Caller is not authorized for this organization' for a well-formed foreign org id in the o- form; Put SUCCEEDED: HTTPStatusCode 200, body {'targetId': '<o-xxxxxxxxxx>', 'scope': 'ORGANIZATION', 'consoleAuthorizationEnabled': True}.

Cause 7 — expected AccessDeniedException / 'No Access to PutResourcePermissionStatement API'; Put SUCCEEDED: HTTPStatusCode 200, body {'statementId': '<64-char base64-alphabet id>'}, both for the ambient caller and for a fresh IAM user carrying an explicit Allow on signin:PutResourcePermissionStatement plus a clientToken.

(Account ids, org ids, statement ids, dates and request ids above are this run's values filling the template; only the status codes, error codes and the fixed message texts named in the description are requirements.)

## Steps to Reproduce
Replay the signin behavioral checks against the emulator (run_behavioral_suite), or by hand against the emulator's signin endpoint:
1. Cause 1: PutConsoleAuthorizationConfiguration(targetId=<caller account id>) in us-east-1, then DeleteConsoleAuthorizationConfiguration(targetId=same), then GetConsoleAuthorizationConfiguration(targetId=same) and a second Delete — inspect the ResourceNotFoundException message of each.
2. Cause 2: DeleteResourcePermissionStatement(statementId='not-a-valid-sid!!') — inspect the ValidationException message's pattern text.
3. Cause 3: on an account with no Sign-In resource-based policy, GetResourcePolicy() — observe 200 instead of 404.
4. Cause 4: same precondition, ListResourcePermissionStatements(maxResults=25) — observe 200 with an empty list instead of 404.
5. Cause 5: PutConsoleAuthorizationConfiguration(targetId=<account id>) twice in us-east-1 — observe the second returns 200 instead of 409 ConflictException.
6. Cause 6: PutConsoleAuthorizationConfiguration(targetId='o-00000000zz') in us-east-1, then GetConsoleAuthorizationConfiguration for the same id — observe 200 and a created ORGANIZATION-scope record instead of AccessDeniedException.
7. Cause 7: PutResourcePermissionStatement(excludedPrincipal=<caller arn>) — observe 200 with a statementId instead of AccessDeniedException; repeat as a fresh IAM user holding an explicit Allow on signin:PutResourcePermissionStatement with a clientToken and observe the same.
