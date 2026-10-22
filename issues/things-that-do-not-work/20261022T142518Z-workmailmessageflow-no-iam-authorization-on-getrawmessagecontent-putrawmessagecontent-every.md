Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workmailmessageflow.get-raw-message-content-action-level-permission-granularity-rejected workmailmessageflow.get-raw-message-content-explicit-deny-overrides-allow-rejected workmailmessageflow.get-raw-message-content-permissions-boundary-restriction-rejected workmailmessageflow.get-raw-message-content-resource-arn-account-mismatch-rejected workmailmessageflow.get-raw-message-content-resource-arn-messageid-mismatch-rejected workmailmessageflow.get-raw-message-content-resource-arn-region-mismatch-rejected workmailmessageflow.put-raw-message-content-session-policy-restriction-rejected
Issue-Shape-Id: com.amazonaws.workmailmessageflow#GiraffeMessageInTransitService
Issue-Key: workmailmessageflow.get-raw-message-content-action-level-permission-granularity-rejected|_wait_for_error#8|AssertionError|raise AssertionError(f"expected error code {expected_code!r}, last observed {last!r}")
Issue-Key: workmailmessageflow.get-raw-message-content-explicit-deny-overrides-allow-rejected|_wait_for_error#6|AssertionError|raise AssertionError(f"expected error code {expected_code!r}, last observed {last!r}")
Issue-Key: workmailmessageflow.get-raw-message-content-permissions-boundary-restriction-rejected|_wait_for_error#13|AssertionError|raise AssertionError(f"expected error code {expected_code!r}, last observed {last!r}")
Issue-Key: workmailmessageflow.get-raw-message-content-resource-arn-account-mismatch-rejected|_wait_for_error#11|AssertionError|raise AssertionError(f"expected error code {expected_code!r}, last observed {last!r}")
Issue-Key: workmailmessageflow.get-raw-message-content-resource-arn-messageid-mismatch-rejected|_wait_for_error#10|AssertionError|raise AssertionError(f"expected error code {expected_code!r}, last observed {last!r}")
Issue-Key: workmailmessageflow.get-raw-message-content-resource-arn-region-mismatch-rejected|_wait_for_error#11|AssertionError|raise AssertionError(f"expected error code {expected_code!r}, last observed {last!r}")
Issue-Key: workmailmessageflow.put-raw-message-content-session-policy-restriction-rejected|_wait_for_error#8|AssertionError|raise AssertionError(f"expected error code {expected_code!r}, last observed {last!r}")
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# workmailmessageflow: no IAM authorization on GetRawMessageContent/PutRawMessageContent — every denial case returns ResourceNotFoundException instead of AccessDeniedException (7 causes)

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRawMessageContent

## Description
The emulator never evaluates the caller's IAM policy for the two WorkMailMessageFlow data-plane operations. Every signed request — whatever identity policy, explicit deny, permissions boundary, session policy, or resource-ARN scoping the assumed-role session carries — is passed straight to message resolution, so the only error the service can ever produce is ResourceNotFoundException. Seven independent IAM evaluation dimensions are therefore all un-enforced; a fixer must close all seven before submitting.

Smithy reference used for all causes: services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/smithy-model/com-amazonaws-workmailmessageflow-giraffemessageintransitservice.json — sdkId WorkMailMessageFlow, protocol restJson1, authSchemes [sigv4], 2 operations: GetRawMessageContent (GET /messages/{messageId}) and PutRawMessageContent (POST /messages/{messageId}); service-common error ResourceNotFoundException. The model does NOT model AccessDeniedException, so the authorization behavior below is 'recorded behavior only': the recordings show error code 'AccessDeniedException' (the sigv4/IAM denial code, HTTP 403 as recorded) with a message built from the request — 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: workmailmessageflow:<Action> on resource: <messageId> <reason-clause>'. Because that message is composed from the caller's session ARN, the requested action and the requested messageId, the composition rule (not the literal recorded text) is the requirement; the reason clause recorded per denial kind is: identity-policy gap -> 'because no identity-based policy allows the workmailmessageflow:<Action> action'; explicit deny -> 'with an explicit deny in an identity-based policy'; permissions boundary -> 'because no permissions boundary allows the workmailmessageflow:<Action> action'; session policy -> 'because no session policy allows the workmailmessageflow:<Action> action'.

Cause 1 of 7 — action-level granularity is not enforced (the two sibling actions are not authorized independently).
AWS: a role granted only workmailmessageflow:GetRawMessageContent reads through to ResourceNotFoundException, but its PutRawMessageContent call is rejected with AccessDeniedException ('...not authorized to perform: workmailmessageflow:PutRawMessageContent on resource: <messageId> because no identity-based policy allows...'). A twin role granted both actions reaches resource resolution. Recorded behavior only (model has no AccessDeniedException).
Emulator: PutRawMessageContent from the get-only role returns ResourceNotFoundException ('Unknown messageId: <messageId>') — the missing grant is ignored.
Why: a client validating least-privilege policies sees a write accepted/resolved under a read-only grant; negative authorization tests pass against the emulator and fail in AWS, and the emulator cannot be used to catch an over-broad policy.
Fix: the request authorization step shared by the two operation handlers (authorize the specific workmailmessageflow action before resource resolution), in the workmailmessageflow service crate.
Traces: workmailmessageflow.get-raw-message-content-action-level-permission-granularity-rejected.

Cause 2 of 7 — explicit Deny does not override Allow.
AWS: IAM evaluation precedence — an explicit Deny for GetRawMessageContent beats an Allow for the same action/resource in the same policy; the call is rejected with AccessDeniedException and the 'with an explicit deny in an identity-based policy' reason, while an Allow-only twin role reads through to ResourceNotFoundException. Recorded behavior only.
Emulator: the deny-carrying role's GetRawMessageContent returns ResourceNotFoundException.
Why: a client relying on a deny guardrail (e.g. a break-glass or data-protection deny) observes the operation as reachable, so the guardrail is silently absent.
Fix: policy evaluation order in the authorization step (deny statements must be evaluated and must short-circuit).
Traces: workmailmessageflow.get-raw-message-content-explicit-deny-overrides-allow-rejected.

Cause 3 of 7 — permissions boundaries are not intersected with the identity policy.
AWS: effective permissions are the intersection of the identity policy and the attached permissions boundary. A role whose identity policy allows GetRawMessageContent but whose boundary allows only an unrelated action is denied with AccessDeniedException and the 'because no permissions boundary allows the workmailmessageflow:GetRawMessageContent action' reason; detaching the boundary restores the grant. Recorded behavior only.
Emulator: the bounded role's GetRawMessageContent returns ResourceNotFoundException — the boundary has no effect.
Why: delegated-administration setups that rely on boundaries to cap a role cannot be tested; the emulator reports a role as more capable than AWS allows.
Fix: include the principal's permissions boundary in the authorization step's evaluation inputs.
Traces: workmailmessageflow.get-raw-message-content-permissions-boundary-restriction-rejected.

Cause 4 of 7 — resource-ARN scoping on an unresolvable message does not fail closed (account segment).
AWS: a 'message/...'-scoped Allow can never be matched for a messageId that does not correspond to a real in-transit message — the service has no organization context to resolve the resource ARN against, so it fails closed with AccessDeniedException regardless of whether the pattern's account segment is the caller's own or a foreign account; only an unscoped Resource="*" statement reaches the existence check and yields ResourceNotFoundException. Recorded behavior only.
Emulator: both the foreign-account-scoped and the real-account-scoped roles return ResourceNotFoundException, i.e. resource-scoped statements are treated as matching.
Why: a client tightening a policy from Resource="*" to a message ARN pattern sees identical behavior in the emulator and a hard AccessDeniedException in AWS.
Fix: resource-ARN matching in the authorization step must fail closed for message-scoped statements when the message cannot be resolved.
Traces: workmailmessageflow.get-raw-message-content-resource-arn-account-mismatch-rejected.

Cause 5 of 7 — resource-ARN scoping on an unresolvable message does not fail closed (region segment).
AWS: same fail-closed rule as cause 4 along the region segment — a message-scoped Allow naming the caller's own request region is denied just like a foreign-region one; only Resource="*" reads through. Recorded behavior only.
Emulator: both region-scoped roles return ResourceNotFoundException.
Why: region-scoped policies appear to grant access in the emulator that AWS denies.
Fix: same resource-ARN matching path as cause 4 (region segment).
Traces: workmailmessageflow.get-raw-message-content-resource-arn-region-mismatch-rejected.

Cause 6 of 7 — resource-ARN scoping on an unresolvable message does not fail closed (messageId segment).
AWS: a 'message/.../{messageId}'-scoped Allow naming the exact messageId in the request is still denied with AccessDeniedException when that messageId never corresponded to a real in-transit message, as is an unrelated messageId pattern; only Resource="*" reads through to ResourceNotFoundException. Recorded behavior only.
Emulator: the exactly-matching-messageId-scoped role returns ResourceNotFoundException.
Why: the emulator reports per-message resource scoping as working when AWS rejects it, so policy tests scoped to a single message are meaningless.
Fix: same resource-ARN matching path as cause 4 (messageId segment).
Traces: workmailmessageflow.get-raw-message-content-resource-arn-messageid-mismatch-rejected.

Cause 7 of 7 — STS session policies are not intersected with the role's permissions.
AWS: a session policy supplied at AssumeRole time further restricts the session. A role allowing both actions, assumed with a session policy granting only GetRawMessageContent, can read but is rejected on PutRawMessageContent with AccessDeniedException and the 'because no session policy allows the workmailmessageflow:PutRawMessageContent action' reason. Recorded behavior only.
Emulator: the session-restricted PutRawMessageContent returns ResourceNotFoundException — the session policy is dropped.
Why: callers that down-scope credentials before handing them to untrusted code get no down-scoping at all in the emulator, which is the inverse of the security property they are testing.
Fix: carry the session policy on the assumed-role credentials into the authorization step and intersect it with the role's policy.
Traces: workmailmessageflow.put-raw-message-content-session-policy-restriction-rejected.

Note: no existing open issue mentions workmailmessageflow (list_existing_issues query 'workmailmessageflow' returned none), and none of these checks was marked ALREADY FILED or OWNED by in the replay report.

## Evidence
Behavioral suite: 8/15 passed; all 7 failures are workmailmessageflow and all show the same shape — the expected AccessDeniedException never appears and the observed error is ResourceNotFoundException.

Expected (AWS, recorded): error code 'AccessDeniedException' with message 'User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: workmailmessageflow:<Action> on resource: <messageId> <reason clause>' (reason clause per denial kind, listed in the description).

Actual (emulator), from the replay output of all 7 checks:
  AssertionError: expected error code 'AccessDeniedException', last observed ('ResourceNotFoundException', 'Unknown messageId: <messageId>')
per check:
  - get-raw-message-content-action-level-permission-granularity-rejected (line 68, PutRawMessageContent under a GetRawMessageContent-only role)
  - get-raw-message-content-explicit-deny-overrides-allow-rejected (line 61, GetRawMessageContent under Allow+explicit Deny)
  - get-raw-message-content-permissions-boundary-restriction-rejected (line 62, GetRawMessageContent under a non-matching permissions boundary)
  - get-raw-message-content-resource-arn-account-mismatch-rejected (line 77, message-ARN-scoped Allow)
  - get-raw-message-content-resource-arn-messageid-mismatch-rejected (line 64, Allow scoped to the exact messageId requested)
  - get-raw-message-content-resource-arn-region-mismatch-rejected (line 79, region-scoped message ARN)
  - put-raw-message-content-session-policy-restriction-rejected (line 66, PutRawMessageContent under a read-only session policy)

Each check also includes a positive control (an unscoped Resource="*" / fully granted twin role) that is expected to reach ResourceNotFoundException; those controls pass, confirming the emulator resolves messages but performs no authorization at all — the denial path is simply never taken.

## Steps to Reproduce
1. Point the AWS SDK/botocore at the emulator.
2. Create an IAM role with an identity policy allowing only workmailmessageflow:GetRawMessageContent on Resource "*", with a trust policy permitting sts:AssumeRole by the caller.
3. AssumeRole into it (session name of your choice) and build a workmailmessageflow client from the returned credentials.
4. Call get_raw_message_content(messageId=<random unused id>) — expect ResourceNotFoundException 'Unknown messageId: <id>' (this passes today, the positive control).
5. Call put_raw_message_content(messageId=<same id>, content={'s3Reference': {'bucket': <b>, 'key': <k>}}).
   Expected: ClientError with Code 'AccessDeniedException' and a message naming the assumed-role session, workmailmessageflow:PutRawMessageContent and the messageId.
   Actual: ClientError with Code 'ResourceNotFoundException', message 'Unknown messageId: <id>'.
6. Repeat with (a) a policy carrying both an Allow and an explicit Deny for GetRawMessageContent, (b) a role with a permissions boundary allowing only sts:GetCallerIdentity, (c) roles whose Allow is scoped to 'arn:aws:workmailmessageflow:<region>:<account>:message/*/*/<messageId>' with the caller's own and with a foreign account/region, and (d) AssumeRole with a session Policy granting only GetRawMessageContent. In every case the emulator returns ResourceNotFoundException where AWS returns AccessDeniedException.
