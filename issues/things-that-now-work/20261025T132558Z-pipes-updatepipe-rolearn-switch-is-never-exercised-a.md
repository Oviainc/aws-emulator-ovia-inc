Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.update-pipe-role-arn-switches-the-execution-role-and-delivery-continues
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes.update-pipe-role-arn-switches-the-execution-role-and-delivery-continues|_receive_until#15|Timeout|resp = sqs.receive_message(
Issue-Commit: 4051ebf28f0bd776eb1ce6243425fd1600c846dd

# Pipes UpdatePipe RoleArn switch is never exercised: a RUNNING SQS-to-SQS pipe delivers nothing before or after the re-role, so the new execution role and redirected target are unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `4051ebf28f0bd776eb1ce6243425fd1600c846dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePipe

## Description
This bundle has 1 cause; it must be closed before submitting.

Cause 1 of 1: UpdatePipe accepts a new RoleArn (and a new Target) and persists it, but no execution path ever uses the pipe's execution role, because a pipe whose CurrentState is RUNNING never polls its SQS source and never invokes its target. The re-role is therefore write-only: the one observable that distinguishes the old role from the new one — a record arriving in a queue that ONLY the new role is permitted to send to — never occurs, before or after the update.

AWS: Smithy (com.amazonaws.pipes#UpdatePipe), PUT /v1/pipes/{Name}, 200; input members Name (PipeName, @length 1..64, @pattern ^[\.\-_A-Za-z0-9]+$, httpLabel, required), RoleArn (RoleArn, @length 1..1600, @pattern ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z0-9+=,.@\-_/]+$, required), Target (Arn, @length 1..1600), TargetParameters (PipeTargetParameters), DesiredState (RequestedPipeState: RUNNING | STOPPED); output Arn/Name/DesiredState/CurrentState/CreationTime/LastModifiedTime; modeled errors ConflictException 409, NotFoundException 404, ThrottlingException 429, ValidationException 400, InternalException 500. Smithy (com.amazonaws.pipes#DescribePipe), GET /v1/pipes/{Name}, 200, returns RoleArn, Source, Target, TargetParameters, CurrentState (PipeState) and StateReason (PipeStateReason, @length 0..512). The operation's own modeled documentation states that UpdatePipe updates only the fields the request specifies and that service-specific Parameters objects are replaced atomically. For the data plane the model says nothing, so: recorded behavior only — the recording shows that after UpdatePipe(Name, RoleArn=<new role>, Target=<new queue>, TargetParameters={InputTemplate: "<$.body>"}) returns 200 with DesiredState=RUNNING, DescribePipe reports RoleArn=<new role> and Source/Target=<source>,<new queue> with CurrentState=RUNNING, and a message then sent to the source arrives in the new target queue under the new role — while the old role, which has sqs:SendMessage only on the old target, could not have delivered it. The recording likewise shows the pre-update leg delivering into the old target queue under the old role.

Emulator: the check reaches the harness cap — "Check timed out after 60.0s" — while polling for the record the pipe should have moved; the wait cannot complete at any budget because the pipes crate has no source poller and no target dispatcher at all (the already-open issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md documents that code state: services/service-com-amazonaws-pipes-pipes/src/ops/create_pipe.rs stores a crate::state::PipeRecord and returns, and the only time-based machinery in the crate is crate::state::settle_due_transitions(), which flips CurrentState). services/service-com-amazonaws-pipes-pipes/src/ops/update_pipe.rs is implemented as a record mutator only: it applies the request's members to the persisted pipe record and answers from what it wrote, with no re-resolution of the new RoleArn against iam, no re-resolution of the new Target, and no rebinding of any execution machinery — there is none to rebind. So RoleArn and Target are storage-only fields.

That open issue is the same family of defect but does NOT cover this check: its Issue-Trace-Ids and its Cause 1 Traces list pipes.update-pipe-target-redirects-delivery-to-the-new-queue, pipes.update-pipe-desired-state-halts-then-resumes-delivery and pipes.update-pipe-filter-criteria-takes-effect-on-a-running-pipe-and-reverts, and never names pipes.update-pipe-role-arn-switches-the-execution-role-and-delivery-continues, so the RoleArn-switch observable carries no replay key today and would be closed unverified. The distinct requirement this check adds beyond that issue is that the pipe's execution identity, not just its target address or its filter, is re-read from the record on every invocation: delivery after the update must be authorized by the NEW role, and a pipe left on the old role must be unable to reach the new target.

Why: a client that rotates a pipe's execution role — the standard remedy for an over-broad or compromised role, and the standard step when a pipe is re-pointed at a queue the old role may not write — gets a 200 from UpdatePipe and a DescribePipe that confirms the new RoleArn, and has no way to discover that the role is inert. Nothing is delivered under either role, so a correct rotation and a rotation onto a role with no permissions at all are indistinguishable, and the pipe reports CurrentState RUNNING with no StateReason throughout. Least-privilege role rotation on a pipe cannot be validated against this emulator in either direction: a passing rotation proves nothing and a broken one raises nothing.

Fix: the pipes data-plane/dispatch engine in services/service-com-amazonaws-pipes-pipes must exist and must read the execution role and target from the current pipe record on each poll/invoke cycle (not captured once at create time), and services/service-com-amazonaws-pipes-pipes/src/ops/update_pipe.rs must re-resolve the incoming RoleArn and Target and rebind the pipe's running execution context so the next cycle authorizes against the new role. The authorization of the target invocation against the role's policy belongs in that dispatcher's target stage, and the terminal UPDATE_FAILED CurrentState plus StateReason belongs in crate::state's pipe state machine for the case where the new role cannot be assumed or cannot reach the new target.

Traces: pipes.update-pipe-role-arn-switches-the-execution-role-and-delivery-continues.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 30/108 passed (27.8%); Mismatched 72, Errored 6, Skipped 0, Unloadable 0. No bootstrap signal — the emulator was serving: SQS CreateQueue/GetQueueAttributes/SendMessage, IAM CreateRole/PutRolePolicy and pipes CreatePipe/DescribePipe/UpdatePipe all answered 200 in this check, and 30 pipes checks passed. Per-service pass rates: pipes 30/95 (32%).

Failing check: [mismatch] pipes.update-pipe-role-arn-switches-the-execution-role-and-delivery-continues
  Error: Check timed out after 60.0s

What the check drives (one source queue, two target queues A and B, two IAM roles — role A may send only to queue A, role B only to queue B):
  1. CreatePipe(Name, RoleArn=<role A>, Source=<source arn>, Target=<queue A arn>, TargetParameters={"InputTemplate": "<$.body>"}, DesiredState="RUNNING")
  2. DescribePipe -> expects CurrentState "RUNNING" and RoleArn == <role A>
  3. SendMessage(source, body="before-update-<hex>") then ReceiveMessage(queue A) until that body arrives
  4. UpdatePipe(Name, RoleArn=<role B>, Target=<queue B arn>, TargetParameters={"InputTemplate": "<$.body>"}) -> expects DesiredState "RUNNING" echoed
  5. DescribePipe -> expects CurrentState "RUNNING", RoleArn == <role B>, (Source, Target) == (<source arn>, <queue B arn>)
  6. SendMessage(source, body="after-update-<hex>") then ReceiveMessage(queue B) until that body arrives — reaching queue B is only possible under role B
  7. DeletePipe and poll DescribePipe until NotFoundException

Expected (recorded): both polls complete — the pre-update body is received from queue A and the post-update body from queue B; the assertion messages the check would raise name the contract exactly: "queue A body {got} != sent {first}", "DescribePipe RoleArn {desc['RoleArn']} != updated {new_arn}", "queue B body {got} != sent {second}".

Actual: the run never completes a delivery poll and is cut at the harness cap. No message is received from either target queue and the source message is never consumed, so step 3 cannot finish and steps 4-7 are never reached. The hang is not a budget artifact: there is no source-poll or target-dispatch path in the pipes crate, so the wait is unsatisfiable at any timeout.

Status/error-code requirements asserted by this check: UpdatePipe -> HTTP 200 (no error code); DescribePipe on the live pipe -> HTTP 200 with RoleArn equal to the role just supplied; DescribePipe after DeletePipe -> HTTP 404, error code NotFoundException. The check tolerates ValidationException and AccessDeniedException ONLY as transient retries while a freshly created role propagates (bounded at 90s), so either code persisting past that window is also a divergence.

Per-run values are examples only and are not requirements: the generated pipe name (prefix "sim2real-pipes-rerole-"), the three generated queue names/URLs/ARNs, the two generated role names and ARNs, the account id, the host and port, the "before-update-<hex>"/"after-update-<hex>" message bodies (the uuid4 hex is per-run; the fixed text is the "before-update-"/"after-update-" prefix), CreationTime/LastModifiedTime and request ids. The requirements are the general ones: a RUNNING pipe moves records source->target under its stored parameters; UpdatePipe's RoleArn and Target take effect on the running pipe; delivery after the update is authorized by the new role; and a deleted pipe is 404 NotFoundException.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Create three SQS queues: a source, target A and target B.
3. Create IAM role A with a trust policy for Principal Service pipes.amazonaws.com and an inline policy allowing sqs:ReceiveMessage/sqs:DeleteMessage/sqs:GetQueueAttributes on the source and sqs:SendMessage on target A ONLY. Create role B identically except that sqs:SendMessage is allowed on target B ONLY.
4. CreatePipe(Name=<name>, RoleArn=<role A arn>, Source=<source arn>, Target=<target A arn>, TargetParameters={"InputTemplate": "<$.body>"}, DesiredState="RUNNING"). Poll DescribePipe until CurrentState is RUNNING and confirm RoleArn is role A.
5. SendMessage to the source, then poll ReceiveMessage on target A. Observe that no message ever arrives and the source message is never consumed — this is where the check is cut at the harness cap.
6. Call UpdatePipe(Name=<name>, RoleArn=<role B arn>, Target=<target B arn>, TargetParameters={"InputTemplate": "<$.body>"}). Observe it returns 200 and DescribePipe reports RoleArn=<role B arn> and Target=<target B arn> with CurrentState RUNNING — the record is updated even though nothing consumes it.
7. SendMessage to the source again and poll ReceiveMessage on target B. Observe nothing arrives, so the claim "delivery after the re-role is authorized by the new role" is unobservable in both directions.
8. Negative control once a data plane exists: leave the pipe on role A but UpdatePipe only the Target to target B. The pipe must then fail to deliver (role A has no sqs:SendMessage on target B) and report it, rather than delivering anyway.
9. DeletePipe and poll DescribePipe until it answers HTTP 404 NotFoundException.
10. After fixing, re-replay pipes.update-pipe-role-arn-switches-the-execution-role-and-delivery-continues.
