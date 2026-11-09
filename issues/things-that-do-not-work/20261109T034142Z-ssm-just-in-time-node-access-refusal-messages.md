Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.start-access-request-denied-without-permission ssm.start-access-request-nonexistent-resource-rejected ssm.get-access-token-nonexistent-request-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.start-access-request-denied-without-permission|run#9|AssertionError|assert message == (
Issue-Key: ssm.start-access-request-nonexistent-resource-rejected|run#18|AssertionError|assert message == (
Issue-Key: ssm.get-access-token-nonexistent-request-rejected|run#11|AssertionError|assert msg2 == (
Issue-Commit: daca2190f290d05b68edbc616a9a31f627d833fa

# SSM just-in-time node access refusal messages diverge: StartAccessRequest AccessDenied names resource "*" instead of the target instance ARN, and GetAccessToken's pattern ValidationException embeds the offending value (2 net-new causes)

> _Note: this issue was identified against the code base at commit `daca2190f290d05b68edbc616a9a31f627d833fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAccessRequest

## Description
Two net-new root causes in the SSM just-in-time node access surface. Both are refusal-message defects: the HTTP status and the error code are right, the message text is not, so a client that reads or matches the message gets the wrong answer. A fixer must close BOTH causes before submitting.

Cause 1 of 2 — StartAccessRequest AccessDeniedException names `resource: *` instead of the ARN of the requested target instance.
AWS: Smithy (com.amazonaws.ssm) models StartAccessRequest with required `Reason` (String1to256, @length 1..256) and required `Targets` (Targets, @length 0..5 of Target{Key,Values}) and the modeled error `AccessDeniedException` (client, httpError 400). The model does not state the message text, so: recorded behavior only — the recording shows AWS refusing a caller whose role carries no identity-based policy for the action with code `AccessDeniedException` and message of the fixed form `User: <caller principal ARN> is not authorized to perform: ssm:StartAccessRequest on resource: <resource ARN> because no identity-based policy allows the ssm:StayAccessRequest action`, where AWS fills `<resource ARN>` with the EC2 instance ARN derived from the request's own target — `arn:aws:ec2:<region>:<account-id>:instance/<instance-id>` for `Targets=[{Key: "InstanceIds", Values: [<instance-id>]}]`. Fixed text: `User: `, ` is not authorized to perform: ssm:StartAccessRequest on resource: `, ` because no identity-based policy allows the ssm:StartAccessRequest action`. Variable parts, taken from the request and the signing credentials: the assumed-role principal ARN (`arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name>`), the region and account id, and the instance id from `Targets[].Values`. AWS resolves the authorization target even when the named instance does not exist — the denial is reached before any existence check, and it still names that instance's ARN.
Emulator: returns the right status/code but substitutes a wildcard for the resource: `User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: ssm:StartAccessRequest on resource: * because no identity-based policy allows the ssm:StartAccessRequest action`. The `on resource: *` segment is emulator-authored; the request's target is never mapped to an instance ARN. Both failing checks diverge on exactly that segment, one with a target instance id and one with a non-existent instance id.
Why: a client (or an operator's runbook, or a policy-debugging tool) parses the denial to learn WHICH resource the missing permission applies to, then writes the matching `Resource` into the identity policy. Against the emulator every StartAccessRequest denial points at `*`, so the resource the grant must name is lost: tooling either widens the policy to `*` or reports "denied on all resources" where AWS pinpoints one instance. Tests asserting the exact AWS denial text also fail.
Fix: the StartAccessRequest handler's authorization/denial path for SSM — the component that formats AccessDeniedException must resolve the request's `Targets` entries into their resource ARNs (`InstanceIds` value -> `arn:aws:ec2:<region>:<account>:instance/<id>`) and render that ARN in the message, instead of hard-coding a wildcard; the resolution must happen before any target-existence check so a bogus instance id still yields the ARN-naming denial.
Traces: ssm.start-access-request-denied-without-permission (restricted role, existing-shaped target i-0000000000000000), ssm.start-access-request-nonexistent-resource-rejected (restricted role, non-existent target i-00000000000000000; its permitted-caller positive control shows the denial is permission-specific).

Cause 2 of 2 — GetAccessToken's pattern ValidationException embeds the rejected value, where AWS's message for `accessRequestId` does not.
AWS: Smithy: GetAccessToken.AccessRequestId -> AccessRequestId, required, @pattern `^(oi)-[0-9a-f]{12}$`; modeled error ValidationException (client, httpError 400). The model states the constraint but not the message, so for the wording: recorded behavior only — the recording shows AWS refusing a malformed id with code `ValidationException` and message `1 validation error detected: Value at 'accessRequestId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(oi)-[0-9a-f]{12}$`. Fixed text: `1 validation error detected: Value at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: `. Variable parts: the member name (`accessRequestId`) and the pattern from the model. Notably AWS does NOT interpolate the submitted value for this member, and this message is distinct from the account-level gate message AWS returns for a well-formed but unissued id (`Just-In-Time Node Access service is not enabled in this account.`), which the emulator does serve correctly.
Emulator: returns ValidationException with the value spliced in: `1 validation error detected: Value 'not-a-real-access-request-id' at 'accessRequestId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(oi)-[0-9a-f]{12}$`. The extra `'<submitted value>' ` after `Value` is the whole divergence (the quoted literal here is the one request's value, not part of the template).
Why: a client that distinguishes refusal classes by message equality — the shape/pattern rejection versus the just-in-time-node-access account gate — cannot match AWS's text, and any assertion or log normalization keyed on AWS's wording breaks; it also echoes caller-supplied input back into a message where AWS deliberately does not.
Fix: the GetAccessToken input validation / ValidationException message formatter for SSM — emit the no-value form of the constraint message for `accessRequestId` rather than the generic value-interpolating template.
Traces: ssm.get-access-token-nonexistent-request-rejected (first assertion, the well-formed-unissued account-gate message, already passes; the malformed-id assertion is the failure).

## Evidence
Cause 1 (both checks, verdict mismatch):
  Expected (AWS): `User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: ssm:StartAccessRequest on resource: arn:aws:ec2:<region>:<account-id>:instance/<instance-id> because no identity-based policy allows the ssm:StartAccessRequest action` (code AccessDeniedException, HTTP 400).
  Actual (emulator): `User: arn:aws:sts::000000000000:assumed-role/sim2real-ssm-restricted-start-access-request-<suffix>/sim2real-behavioral-check is not authorized to perform: ssm:StartAccessRequest on resource: * because no identity-based policy allows the ssm:StartAccessRequest action`
  and `User: arn:aws:sts::000000000000:assumed-role/sim2real-ssm-no-perms-access-req-<suffix>/sim2real-session-<suffix> is not authorized to perform: ssm:StartAccessRequest on resource: * because no identity-based policy allows the ssm:StartAccessRequest action`.
  Code and status match; only the resource segment diverges (`*` vs the target instance ARN). Principal/account/suffix values above are per-run and are not the requirement.

Cause 2 (verdict mismatch):
  Expected (AWS): `1 validation error detected: Value at 'accessRequestId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(oi)-[0-9a-f]{12}$` (code ValidationException, HTTP 400).
  Actual (emulator): `1 validation error detected: Value 'not-a-real-access-request-id' at 'accessRequestId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(oi)-[0-9a-f]{12}$`.
  Assertion raised at check_ssm_get_access_token_nonexistent_request_rejected.py line 39.

## Steps to Reproduce
Cause 1:
1. iam.CreateRole with a trust policy allowing the account root to sts:AssumeRole, and attach NO permissions policy.
2. sts.AssumeRole into it, build an SSM client from those credentials.
3. Call ssm.StartAccessRequest(Reason="<any 1..256 chars>", Targets=[{"Key": "InstanceIds", "Values": ["i-0000000000000000"]}]).
4. Observe code AccessDeniedException (correct) but the message's resource segment is `*`; AWS names `arn:aws:ec2:<region>:<account>:instance/i-0000000000000000`. Repeat with a non-existent instance id (i-00000000000000000) — AWS still names that instance ARN.

Cause 2:
1. Call ssm.GetAccessToken(AccessRequestId="oi-000000000000") — the account-gate message is served correctly.
2. Call ssm.GetAccessToken(AccessRequestId="not-a-real-access-request-id").
3. Observe ValidationException whose message interpolates the submitted value after `Value`, where AWS emits `Value at 'accessRequestId' failed to satisfy constraint: ...`.
