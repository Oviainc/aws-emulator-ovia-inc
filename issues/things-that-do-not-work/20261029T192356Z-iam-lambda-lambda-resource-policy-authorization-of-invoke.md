Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda.xsvc-iam-2-lambda-add-permission-alone-authorizes-invoke iam__lambda.xsvc-iam-2-lambda-invoke-without-permission-denied iam__lambda.xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied
Issue-Key: iam__lambda.xsvc-iam-2-lambda-add-permission-alone-authorizes-invoke|_settle#13|Timeout|time.sleep(interval_s)
Issue-Key: iam__lambda.xsvc-iam-2-lambda-invoke-without-permission-denied|_settle#11|Timeout|time.sleep(interval_s)
Issue-Key: iam__lambda.xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied|_settle#13|Timeout|time.sleep(interval_s)
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# iam__lambda: Lambda resource-policy authorization of Invoke is unobservable — all three AddPermission/RemovePermission invoke checks are killed at the 60 s cap

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: lambda:Invoke never produces an authorization verdict, so the function resource policy (AddPermission / RemovePermission) has no observable effect.
AWS: lambda:AddPermission alone authorizes a principal to Invoke the function (Invoke answers 200 with the function's payload); with no grant, Invoke is refused with AccessDeniedException (HTTP 403); after RemovePermission the same principal is refused again. Each decision is returned immediately on the call.
Emulator: all three checks make no progress and are killed at the harness's 60 s cap — the emulator returns no verdict for an authorized invoke, an unauthorized invoke or a revoked invoke. Recorded detail for each: 'Check timed out after 60.0s'. The related open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md records that Invoke refuses real invocations with NotImplemented; it does not carry these replay keys, and these three checks additionally show that NO decision (allow or deny) is ever reached, so the resource-policy authorization path itself is unexercised.
Why: a client cannot tell an allowed invoke from a denied one. Both the positive grant and the negative control are unverifiable, so a function whose resource policy grants nothing looks identical to one that grants everything — and a call that never returns is itself a divergence from AWS, which answers within one request.
Fix: the Invoke handler — it must evaluate the function's resource policy (and the caller's identity policy) and return either the function's response or AccessDeniedException, instead of leaving the call unresolved.
Traces: iam__lambda.xsvc-iam-2-lambda-add-permission-alone-authorizes-invoke, iam__lambda.xsvc-iam-2-lambda-invoke-without-permission-denied, iam__lambda.xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied

## Evidence
Expected: Invoke answers 200 with the function's payload when AddPermission granted lambda:InvokeFunction to the principal, and HTTP 403 AccessDeniedException when no grant exists or after RemovePermission. Actual: for all three checks the only recorded result is `Error: Check timed out after 60.0s` — the emulator produced no response the check could grade. (The 60 s value is the harness cap, not a requirement; the requirement is that Invoke answers at all, with the allow/deny AWS returns.)

## Steps to Reproduce
Create a Lambda function and a second principal (an IAM user/role). (1) Call lambda:AddPermission granting lambda:InvokeFunction to that principal, then Invoke as that principal — AWS answers 200. (2) Repeat with no AddPermission — AWS answers 403 AccessDeniedException. (3) Call lambda:RemovePermission for the statement, then Invoke again — AWS answers 403. In the emulator each leg never returns a gradeable result and the check is killed at the cap.
