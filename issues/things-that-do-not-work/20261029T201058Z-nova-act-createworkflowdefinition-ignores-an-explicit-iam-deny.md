Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: nova-act.access-denied-without-permission-rejected nova-act.create-workflow-definition-duplicate-name-rejected
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act.access-denied-without-permission-rejected|_error_code#18|AssertionError|raise AssertionError(f"expected a ClientError, call unexpectedly succeeded with {resp!r}")
Issue-Key: nova-act.create-workflow-definition-duplicate-name-rejected|run#7|AssertionError|assert err.get("Message") == "Workflow definition already exists", (
Issue-Commit: e0f3b342c2995e24bb81f3397837b3776f1c553b

# nova-act CreateWorkflowDefinition ignores an explicit IAM Deny and renders the duplicate-name ConflictException message wrongly

> _Note: this issue was identified against the code base at commit `e0f3b342c2995e24bb81f3397837b3776f1c553b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflowDefinition

## Description
CreateWorkflowDefinition diverges from AWS on two independent refusal paths. TWO causes must be closed before submitting; each has its own failing behavioral check listed under its Traces line.

Cause 1 of 2: a principal with an explicit Deny on nova-act:* is allowed to create a workflow definition (no authorization evaluation at all).
AWS: Smithy (services/service-com-amazonaws-novaact-amazonnovaagentsdataplane) models CreateWorkflowDefinition as PUT /workflow-definitions, success code 201, with AccessDeniedException among its errors (errorKind client, httpError 403, required member `message` targeting NonBlankString; doc: "You don't have sufficient permissions to perform this action. Verify your IAM permissions and try again."). AWS evaluates the signing identity's identity-based policies before any resource is created: a caller whose role policy contains Effect=Deny on Action nova-act:* gets HTTP 403 AccessDeniedException and NO definition is created, so a subsequent GetWorkflowDefinition for that name answers ResourceNotFoundException (message "Workflow definition not found"). The AccessDeniedException message is the standard IAM explicit-deny sentence; its FIXED text is "User: " / " is not authorized to perform: " / " on resource: " / " with an explicit deny in an identity-based policy", and its VARIABLE parts all come from the request and state: the caller ARN of the signing credentials (an assumed-role ARN of the form arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>), the denied action "nova-act:CreateWorkflowDefinition", and the target resource ARN arn:aws:nova-act:<region>:<account>:workflow-definition/<name> built from the request's `name`. The account id, role name, session name, region and definition name in any one recording are values filled into that template, not part of it.
Emulator: the denied call SUCCEEDS. It answers HTTP 201 with body {"status": "ACTIVE"} (content-type application/json, content-length 19) — no 403, no AccessDeniedException, no `message` — and the definition is actually created, so the check's follow-up read finds a resource that must not exist. Polling for 30s never produces AccessDeniedException, which shows the authorization decision is not made at all rather than made late.
Why: the emulator is over-permissive on this operation. Any client, test suite or guardrail that relies on an explicit Deny being enforced (least-privilege regression tests, deny-list policy verification, blast-radius checks before a production rollout) passes against the emulator and fails against AWS. Worse, the denied principal leaves a real resource behind, so cleanup and ownership assumptions built on "a denied create creates nothing" break.
Fix: the authorization stage in front of nova-act operation dispatch (or the CreateWorkflowDefinition handler's pre-dispatch check), evaluated against the signing principal's policies BEFORE the handler reaches state insertion; it must raise AccessDeniedException (403) with the explicit-deny message template above and must not mutate state.
Traces: behavioral check nova-act.access-denied-without-permission-rejected.

Cause 2 of 2: the duplicate-name ConflictException carries a non-AWS message.
AWS: Smithy models ConflictException for CreateWorkflowDefinition (errorKind client, httpError 409, required members `message`, `resourceId`, `resourceType`). Recorded behavior: re-creating an existing definition name with a FRESH clientToken (so this is not an idempotent replay) answers 409 ConflictException whose `message` is the fixed sentence "Workflow definition already exists" — it has NO variable parts: neither the requested name nor the existing definition's status appears in it. resourceType is "workflow-definition" and resourceId is the existing definition's ARN.
Emulator: returns the right status/code (409 ConflictException) but a different message: "workflow definition nwfdup330fda6c39cf already exists with status ACTIVE" — lower-cased leading text, the requested name interpolated, and the existing definition's status appended. (The name and status shown are this run's values; the defect is that the message is templated from request/state at all.)
Why: the message is part of the contract for this error class. A client, CLI or test that matches on AWS's text to distinguish "name taken" from other conflicts sees a string AWS never emits; interpolating the submitted workflow name also leaks request content into error text AWS deliberately omits, breaking log redaction and golden-message assertions.
Fix: the ConflictException construction inside CreateWorkflowDefinition's duplicate-name precondition check (the handler/state uniqueness path) — emit the fixed message text and stop interpolating the name and status.
Note on an existing open issue: 20261023T043407Z-nova-act-createworkflowdefinition-skips-precondition-checks-and-returns.md covers CreateWorkflowDefinition answering 201 where AWS answers 409/400 (no precondition checks at all). That is NOT this cause: the emulator now does answer 409 ConflictException for a duplicate name, so what remains here is purely the message text, which that issue explicitly treats as 'an example' rather than a requirement. It therefore does not cover this check.
Traces: behavioral check nova-act.create-workflow-definition-duplicate-name-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (nova-act 3/22 passed).

Cause 1 — [mismatch] nova-act.access-denied-without-permission-rejected:
  The check creates an IAM role with an inline policy {Effect: Deny, Action: "nova-act:*", Resource: "*"}, assumes it via sts:AssumeRole, and calls CreateWorkflowDefinition with those session credentials.
  Expected: ClientError AccessDeniedException (HTTP 403), message "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: nova-act:CreateWorkflowDefinition on resource: arn:aws:nova-act:<region>:<account>:workflow-definition/<name> with an explicit deny in an identity-based policy"; and GetWorkflowDefinition for that name then raising ResourceNotFoundException "Workflow definition not found".
  Actual: the call returned successfully. Harness output:
    AssertionError: expected a ClientError, call unexpectedly succeeded with {'ResponseMetadata': {'HTTPStatusCode': 201, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '19', ...}, 'RetryAttempts': 0}, 'status': 'ACTIVE'}
  i.e. 201 + {"status":"ACTIVE"} where AWS gives 403 AccessDeniedException. The expected assertion was retried for 30s at 3s intervals and never saw the deny.

Cause 2 — [mismatch] nova-act.create-workflow-definition-duplicate-name-rejected:
  Step 1 creates definition <name> with description "original". Step 2 re-issues CreateWorkflowDefinition with the SAME name, a different clientToken and description "clobber-attempt".
  Error Code: ConflictException — matches AWS.
  Expected $.message: "Workflow definition already exists"
  Actual   $.message: "workflow definition nwfdup330fda6c39cf already exists with status ACTIVE"
  Harness output: AssertionError: expected the exact duplicate-name message, got 'workflow definition nwfdup330fda6c39cf already exists with status ACTIVE'
  Fixed text of the AWS message: the whole sentence "Workflow definition already exists" (no variable parts). The emulator's extra content — the definition name and the word ACTIVE — is drawn from the request and from stored state and does not belong in the message.

Both causes land in the same operation handler but in different stages (authorization vs. conflict-error construction), so neither fix implies the other.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite simparity-behavioral-aws-v1 (service nova-act, client pinned to region us-east-1).

2. Cause 1 — replay check nova-act.access-denied-without-permission-rejected:
   a. iam:CreateRole with a trust policy allowing arn:aws:iam::<account>:root to sts:AssumeRole.
   b. iam:PutRolePolicy with {"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"nova-act:*","Resource":"*"}]}.
   c. sts:AssumeRole for that role; build a nova-act client from the returned session credentials.
   d. Call CreateWorkflowDefinition(name=<fresh name>, clientToken=<fresh token>) with those denied credentials.
   Expect HTTP 403 AccessDeniedException with the explicit-deny message; the emulator returns HTTP 201 {"status":"ACTIVE"}.
   e. With normal (un-denied) credentials call GetWorkflowDefinition(workflowDefinitionName=<that name>). Expect ResourceNotFoundException "Workflow definition not found" (the denied create must have created nothing); the emulator has the definition.

3. Cause 2 — replay check nova-act.create-workflow-definition-duplicate-name-rejected:
   a. CreateWorkflowDefinition(name=<N>, description="original", clientToken=<token A>).
   b. CreateWorkflowDefinition(name=<N>, description="clobber-attempt", clientToken=<token B>, i.e. a different token so this is not an idempotent replay).
   Expect 409 ConflictException with $.message exactly "Workflow definition already exists" (plus resourceType "workflow-definition" and resourceId the definition ARN); the emulator returns 409 ConflictException with $.message "workflow definition <N> already exists with status <status>".

4. Re-replay both check ids (nova-act.access-denied-without-permission-rejected, nova-act.create-workflow-definition-duplicate-name-rejected) to confirm the fix.
