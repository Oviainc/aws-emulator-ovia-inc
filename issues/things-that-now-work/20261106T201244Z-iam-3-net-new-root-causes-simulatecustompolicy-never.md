Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.simulate-condition-key-context-entries iam.simulate-custom-policy-uses-context-keys iam.op-contract-simulate-custom-policy iam.simulate-invalid-input-rejected iam.sep-policy-composition-session-policy-narrows-never-widens-denied-by-simulator-and-real-call
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.simulate-condition-key-context-entries|run#6|AssertionError|assert inside["EvalDecision"] == "allowed", (
Issue-Key: iam.simulate-custom-policy-uses-context-keys|run#10|AssertionError|assert allowed[0]["EvalDecision"] == "allowed", (
Issue-Key: iam.op-contract-simulate-custom-policy|run#10|AssertionError|assert msg == want, f"InvalidInput message: wanted {want!r}, got {msg!r}"
Issue-Key: iam.simulate-invalid-input-rejected|run#6|AssertionError|assert msg_doc == want, f"InvalidInput message: wanted {want!r}, got {msg_doc!r}"
Issue-Key: iam.sep-policy-composition-session-policy-narrows-never-widens-denied-by-simulator-and-real-call|run#27|AssertionError|assert narrowed["iam:ListRoleTags"] == "AccessDenied", (
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# iam: 3 net-new root causes — SimulateCustomPolicy never satisfies an IpAddress/aws:SourceIp condition from ContextEntries, its InvalidInput message is generic, and an AssumeRole session policy does not narrow the session

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulateCustomPolicy

## Description
Three distinct net-new root causes on IAM's policy-simulation and session-policy surface. Each must be closed before this issue is submitted.

Cause 1 of 3: SimulateCustomPolicy returns implicitDeny for a Condition-guarded Allow even when a SATISFYING value for the referenced condition key is supplied in ContextEntries — specifically an IpAddress operator on aws:SourceIp with a ContextKeyType of 'ip'.
AWS: Smithy: SimulateCustomPolicy takes ContextEntries (ContextEntryListType of ContextEntry{ContextKeyName, ContextKeyValues, ContextKeyType}); ContextEntry.ContextKeyValues is documented as "The value (or values...) to provide to the simulation when the key is referenced by a Condition element in an input policy", ContextKeyTypeEnum includes 'ip', and EvalDecision is PolicyEvaluationDecisionType (allowed|explicitDeny|implicitDeny). For a policy whose only statement is Allow s3:ListBucket on "*" guarded by {"IpAddress": {"aws:SourceIp": "<cidr>"}}, a simulation that supplies aws:SourceIp with an address INSIDE that CIDR returns EvalDecision 'allowed', an empty MissingContextValues, and MatchedStatements attributed to SourcePolicyId 'PolicyInputList.1'; an address outside the CIDR returns implicitDeny with no matched statement; withholding the key returns implicitDeny with the key listed in MissingContextValues.
Emulator: the satisfying case also evaluates 'implicitDeny'. The withheld-key and violating-value branches "pass" only because implicitDeny is what the emulator returns in every case, so the simulator never discriminates. The same policy shape with two keys in one Condition block ({"StringEquals": {"s3:x-amz-server-side-encryption": "aws:kms"}, "IpAddress": {"aws:SourceIp": "<cidr>"}}), with BOTH keys supplied satisfyingly, also evaluates 'implicitDeny'. The IpAddress/aws:SourceIp term is the only element common to both failures. Hypothesis: typed ('ip') ContextEntries values are not fed into the evaluator's condition context, or the IpAddress operator is not implemented, so any statement guarded by it can never match.
Why: the simulator is the API callers use to pre-flight a conditional grant before deploying it. A false implicitDeny makes every CIDR-scoped (and, by the second trace, every multi-key-condition) Allow look broken, so a correct least-privilege policy is reported as denying and a client cannot distinguish "condition not satisfied" from "condition not evaluated".
Fix: the condition-evaluation path of the simulate_custom_policy handler — how ContextEntries populate the request context and how the IpAddress operator compares an 'ip'-typed value against the policy's CIDR.
Traces: iam.simulate-condition-key-context-entries, iam.simulate-custom-policy-uses-context-keys

Cause 2 of 3: SimulateCustomPolicy's refusal of an unparseable PolicyInputList entry carries a generic message that names no entry.
AWS: an entry in PolicyInputList that is not a parseable policy document is refused with error code InvalidInput (Smithy: InvalidInputException, awsQueryError code InvalidInput, HTTP 400) and the message 'Policy input list item <n> has invalid content' — fixed text "Policy input list item " + the 1-BASED INDEX of the offending PolicyInputList entry + " has invalid content". The index is the only variable part; it comes from the request.
Emulator: the code InvalidInput is right, but the message is 'Syntax errors in policy.' — a sentence that identifies no entry and is not AWS's wording.
Why: a caller submitting several documents in one simulation cannot tell which one was rejected, and any client matching AWS's message text diverges.
Fix: the InvalidInput message construction in the simulate_custom_policy handler's input validation. The identical wording defect is already filed for a DIFFERENT operation — Cause 8 of 20261029T192356Z-iam-9-net-new-authorization-and-policy-evaluation.md covers GetContextKeysForCustomPolicy — so that issue does not cover the simulate handler; both message builders need AWS's text.
Traces: iam.op-contract-simulate-custom-policy, iam.simulate-invalid-input-rejected

Cause 3 of 3: the inline session policy passed to AssumeRole does not narrow the resulting session — an action the role policy allows but the session policy omits still succeeds.
AWS: a session policy can only narrow, never widen: the effective permissions of a session created with AssumeRole's Policy parameter are the intersection of the role's identity policies and the session policy. An action the role allows and the session policy omits is refused with exactly AccessDenied, message 'User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: <action> on resource: role <role-name> because no session policy allows the <action> action' (plus AWS's authorization-detail suffix); an action the session policy grants but the role does not is likewise AccessDenied, with 'because no identity-based policy allows the <action> action'. Recorded behavior only for the exact suffix; the account id, role name, session name, action and resource are all values taken from the request and the caller's state, not fixed text.
Emulator: under the narrowed session, iam:ListRoleTags — granted by the role policy, absent from the session policy — SUCCEEDS (the check records 'OK' where AWS refuses). The session therefore carries the role's full permissions and the scoping is silently discarded.
Why: this is a silent over-permit. Scoped-down sessions are the standard way a caller hands out least-privilege credentials; a session that ignores its session policy grants more than the issuer asked for, and every negative control built on session policies cannot fail.
Fix: STS AssumeRole must persist the session Policy with the minted session, and IAM's authorization layer must intersect it with the role's identity policies when evaluating a call made under that session. A sibling trace of this same cause is already filed as Cause 3 of 20261029T192356Z-iam-9-net-new-authorization-and-policy-evaluation.md (trace iam.session-policy-intersects-the-role-policy); the check listed below is NOT carried by that issue's replay keys, so it is filed here and should be re-replayed alongside it.
Traces: iam.sep-policy-composition-session-policy-narrows-never-widens-denied-by-simulator-and-real-call

## Evidence
Cause 1 — iam.simulate-condition-key-context-entries: AssertionError "203.0.113.5 is inside 203.0.113.0/24 so the conditional Allow must fire; got 'implicitDeny'" (policy: Allow s3:ListBucket on "*" with Condition {"IpAddress": {"aws:SourceIp": "203.0.113.0/24"}}; ContextEntries: aws:SourceIp, ContextKeyType 'ip', value inside the CIDR). iam.simulate-custom-policy-uses-context-keys: AssertionError "a context satisfying every referenced Condition key must grant; expected 'allowed', got 'implicitDeny'" (Condition block StringEquals s3:x-amz-server-side-encryption=aws:kms AND IpAddress aws:SourceIp=10.0.0.0/24, both keys supplied satisfyingly). Expected EvalDecision 'allowed'; actual 'implicitDeny'.

Cause 2 — iam.op-contract-simulate-custom-policy and iam.simulate-invalid-input-rejected both fail with: AssertionError "InvalidInput message: wanted 'Policy input list item 1 has invalid content', got 'Syntax errors in policy.'". The error CODE InvalidInput and the HTTP 400 are correct; only the message diverges. ('1' here is the 1-based index of the single bad entry submitted — the index is a request-derived value, not fixed text.)

Cause 3 — iam.sep-policy-composition-session-policy-narrows-never-widens-denied-by-simulator-and-real-call: AssertionError "the role allows ListRoleTags but the session policy omits it, so the session must be refused with exactly AccessDenied (narrowing); got 'OK'". The role's inline policy allows iam:GetRole and iam:ListRoleTags on the role ARN; the session policy passed to AssumeRole allows iam:GetRole and iam:ListRolePolicies. Expected the intersection ({GetRole: allowed, ListRoleTags: AccessDenied, ListRolePolicies: AccessDenied}); actual ListRoleTags returned HTTP 200.

## Steps to Reproduce
Cause 1: call SimulateCustomPolicy with PolicyInputList=[a policy whose only statement is Allow s3:ListBucket on "*" with Condition {"IpAddress": {"aws:SourceIp": "203.0.113.0/24"}}], ActionNames=["s3:ListBucket"] and ContextEntries=[{ContextKeyName: "aws:SourceIp", ContextKeyType: "ip", ContextKeyValues: ["203.0.113.5"]}]; read EvaluationResults[0].EvalDecision (expect 'allowed', MissingContextValues empty, MatchedStatements attributed to 'PolicyInputList.1'). Repeat with an address outside the CIDR (expect implicitDeny) and with the key withheld (expect implicitDeny plus aws:SourceIp in MissingContextValues) to confirm the three-way discrimination.

Cause 2: call SimulateCustomPolicy with PolicyInputList=['{not valid json'] (or '{"Version": "2012-10-17", "Statement": ['), ActionNames=["s3:ListBucket"]; inspect Error.Code and Error.Message.

Cause 3: create a role trusting the caller, put an inline role policy allowing iam:GetRole and iam:ListRoleTags on the role ARN, AssumeRole with Policy=<a session policy allowing iam:GetRole and iam:ListRolePolicies on that ARN>, then call ListRoleTags and ListRolePolicies with the returned session credentials: both must be AccessDenied with AWS's session / identity-based wording, while GetRole succeeds. A control session assumed with no Policy must still succeed at ListRoleTags.
