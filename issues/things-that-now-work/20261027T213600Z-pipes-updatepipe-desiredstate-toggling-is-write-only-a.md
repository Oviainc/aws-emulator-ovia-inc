Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.update-pipe-desired-state-halts-then-resumes-delivery
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes.update-pipe-desired-state-halts-then-resumes-delivery|_receive_all#19|Timeout|resp = sqs.receive_message(
Issue-Commit: f143463d502a212e51b38689c0495a5f574ac69f

# pipes: UpdatePipe DesiredState toggling is write-only — a RUNNING pipe never consumes its source, so STOPPED withholds nothing and RUNNING never drains the backlog

> _Note: this issue was identified against the code base at commit `f143463d502a212e51b38689c0495a5f574ac69f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePipe

## Description
This bundle has 1 cause; it must be closed before submitting.

Cause 1 of 1: UpdatePipe accepts and persists DesiredState (STOPPED, then RUNNING) and DescribePipe reports the matching CurrentState, but the toggle has no effect on any data movement because the pipes emulator has no source poller and no target dispatcher: a pipe whose CurrentState is RUNNING never receives from its SQS source and never sends to its target. Both halves of the DesiredState contract are therefore unobservable — the 'STOPPED withholds the message' half is vacuously true (nothing is ever delivered under any state) and the 'RUNNING then delivers the withheld message' half can never be satisfied. Failing check: pipes.update-pipe-desired-state-halts-then-resumes-delivery.

AWS: Smithy (com.amazonaws.pipes#UpdatePipe), PUT /v1/pipes/{Name}, 200; input Name (PipeName, @length 1..64, @pattern ^[\.\-_A-Za-z0-9]+$, httpLabel, required), RoleArn (RoleArn, @length 1..1600, @pattern ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z0-9+=,.@\-_/]+$, required), DesiredState (RequestedPipeState: RUNNING | STOPPED), plus SourceParameters/Target/TargetParameters/Enrichment; output echoes DesiredState and CurrentState; modeled errors ConflictException 409, NotFoundException 404, ThrottlingException 429, ValidationException 400, InternalException 500. Smithy (com.amazonaws.pipes#DescribePipe), GET /v1/pipes/{Name}, 200, returns DesiredState, CurrentState (PipeState: RUNNING, STOPPED, CREATING, UPDATING, DELETING, STARTING, STOPPING, CREATE_FAILED, UPDATE_FAILED, START_FAILED, ...) and StateReason (PipeStateReason, @length 0..512). SourceParameters.SqsQueueParameters is PipeSourceSqsQueueParameters (BatchSize @range 1..10000, MaximumBatchingWindowInSeconds @range 0..300). For the data plane the model says nothing, so: recorded behavior only — the recording shows that after UpdatePipe(Name, RoleArn, DesiredState="STOPPED") returns 200 and DescribePipe settles on DesiredState/CurrentState STOPPED and HOLDS there, a message sent to the source is NOT delivered to the target and remains counted on the source (SQS GetQueueAttributes ApproximateNumberOfMessages reads 1), and that a subsequent UpdatePipe(Name, RoleArn, DesiredState="RUNNING") makes that same withheld message arrive at the target rendered by the pipe's TargetParameters.InputTemplate ("<$.body>" yields the raw source body). The IaC-relevant requirement is that this works through UpdatePipe's DesiredState member, not only through StopPipe/StartPipe.

Emulator: the check is cut at the harness cap — 'Check timed out after 60.0s' — on its FIRST wait, the positive control: after CreatePipe(..., SourceParameters={SqsQueueParameters:{BatchSize:1}}, TargetParameters={InputTemplate:"<$.body>"}, DesiredState=RUNNING) reaches CurrentState RUNNING, SendMessage on the source is followed by ReceiveMessage polling on the target that never returns the body, so the UpdatePipe(DesiredState=STOPPED) leg, the hold, the withheld-message assertion, the ApproximateNumberOfMessages==1 assertion and the resume leg are never reached. The hang is not a budget artifact: there is no source-poll or target-dispatch path in the pipes crate at all (services/service-com-amazonaws-pipes-pipes/src/ops/create_pipe.rs stores a crate::state::PipeRecord and returns; the only time-based machinery is crate::state::settle_due_transitions(), which only flips CurrentState), so the wait is unsatisfiable at any timeout. services/service-com-amazonaws-pipes-pipes/src/ops/update_pipe.rs is a record mutator: it writes DesiredState onto the stored record and answers from what it wrote, with nothing to pause or resume.

Relation to already-open issues (neither covers this check today): 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md is the same family — it documents the missing data plane — and 20261025T132558Z-pipes-updatepipe-rolearn-switch-is-never-exercised-a.md covers the RoleArn-switch observable of the same operation. This run's replay report does NOT mark pipes.update-pipe-desired-state-halts-then-resumes-delivery as owned by either, so its replay key is not carried by an open issue and the DesiredState observable would be closed unverified. The distinct requirement this check adds beyond the no-runtime issue is that the pipe's run/stop gate is re-read from the stored DesiredState on every poll cycle: a pipe moved to STOPPED must stop RECEIVING from the source (the message stays visible and counted, not consumed-and-dropped), must hold that state across outstanding long polls, and must deliver the accumulated backlog when DesiredState goes back to RUNNING — i.e. stopping must not lose or silently consume the records it withholds.

Why: a client that toggles a pipe with UpdatePipe (what CDK/SAM/Terraform emit, since they declare DesiredState rather than calling StopPipe/StartPipe) gets 200 and a DescribePipe that confirms the requested state, yet learns nothing about whether traffic actually stopped or restarted. A paused pipe that silently drains and discards its source and a correctly paused pipe are indistinguishable here, and the resume path — the one that must replay the backlog — is dead. Drain/cutover runbooks and maintenance windows cannot be validated against this emulator in either direction.

Fix: the pipes data-plane/dispatch engine in services/service-com-amazonaws-pipes-pipes must exist and must consult the pipe record's current DesiredState/CurrentState on each poll cycle, suspending source ReceiveMessage while STOPPED (leaving messages visible on the source) and resuming consumption and target delivery when it returns to RUNNING; services/service-com-amazonaws-pipes-pipes/src/ops/update_pipe.rs must drive that suspend/resume through crate::state's pipe state machine (STOPPING -> STOPPED, STARTING -> RUNNING) instead of only writing the field.

Traces: pipes.update-pipe-desired-state-halts-then-resumes-delivery.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 71/106 passed (67.0%); Mismatched 32, Errored 3, Skipped 0, Unloadable 0. No bootstrap signal — the emulator was serving: SQS CreateQueue/GetQueueAttributes/SendMessage/ReceiveMessage, IAM CreateRole/PutRolePolicy and pipes CreatePipe/DescribePipe/UpdatePipe all answered 200 in this check, and 69/93 pipes checks passed. Per-service pass rates: pipes 69/93 (74%).

Failing check: [mismatch] pipes.update-pipe-desired-state-halts-then-resumes-delivery
  Error: Check timed out after 60.0s

What the check drives (one source queue, one target queue, one role allowing sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source and sqs:SendMessage on the target):
  1. CreatePipe(Name, RoleArn, Source=<src arn>, SourceParameters={"SqsQueueParameters": {"BatchSize": 1}}, Target=<tgt arn>, TargetParameters={"InputTemplate": "<$.body>"}, DesiredState="RUNNING"), poll DescribePipe until CurrentState RUNNING.
  2. Positive control: SendMessage(source, body="control-<hex>"), then ReceiveMessage(target) until that body arrives; asserts got["Body"] == the sent body.
  3. UpdatePipe(Name, RoleArn, DesiredState="STOPPED"); poll DescribePipe to CurrentState STOPPED; assert DescribePipe DesiredState == "STOPPED"; hold 40 s re-reading CurrentState, which must stay "STOPPED".
  4. SendMessage(source, body="held-<hex>"); drain the target for 45 s and assert that body is absent; poll SQS GetQueueAttributes(ApproximateNumberOfMessages) on the SOURCE until it reads 1 — the withheld message must still be there.
  5. UpdatePipe(Name, RoleArn, DesiredState="RUNNING"); poll DescribePipe to CurrentState RUNNING; assert DesiredState == "RUNNING"; ReceiveMessage(target) until the "held-<hex>" body arrives.
  6. DeletePipe, then poll DescribePipe until HTTP 404 NotFoundException.

Expected (recorded): every wait completes — control body at the target under RUNNING; nothing at the target while STOPPED and ApproximateNumberOfMessages == 1 on the source; the held body at the target after the resume. The check's own assertion text names the contract: "a message sent after UpdatePipe(DesiredState=STOPPED) settled was delivered", "source queue should still hold the withheld message, ApproximateNumberOfMessages=...", "resumed delivery body ... != ...".

Actual: the run never completes step 2 and is cut at the harness cap after 60 s. No message is ever received from the target and the source message is never consumed, so steps 3-6 are never reached and both DesiredState halves are unobservable.

Status/error-code requirements asserted by this check: UpdatePipe -> HTTP 200 with the requested DesiredState echoed (no error code); DescribePipe on the live pipe -> HTTP 200 with DesiredState/CurrentState equal to the requested state; DescribePipe after DeletePipe -> HTTP 404, error code NotFoundException; DeletePipe mid-transition may be HTTP 409 ConflictException (retried, bounded 45 s) and CreatePipe may answer ValidationException/AccessDeniedException only while a fresh role propagates (bounded 90 s) — either code persisting past those windows is also a divergence.

Per-run values are examples only and are NOT requirements: the generated pipe name (fixed prefix "sim2real-pipes-upds-", per-run suffix), the generated queue names/URLs/ARNs, the generated role name/ARN, the account id, host and port, the message bodies (the fixed text is the "control-"/"held-" prefix; the uuid4 hex is per-run), CreationTime/LastModifiedTime and request ids. The requirements are general: a RUNNING pipe moves records source->target under its stored SourceParameters/TargetParameters; UpdatePipe(DesiredState=STOPPED) suspends source consumption so the record stays visible and counted on the source; UpdatePipe(DesiredState=RUNNING) resumes and delivers that withheld record; a deleted pipe answers 404 NotFoundException.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Create two SQS queues (source, target) and an IAM role whose trust policy names Principal Service pipes.amazonaws.com, with an inline policy allowing sqs:ReceiveMessage/sqs:DeleteMessage/sqs:GetQueueAttributes on the source and sqs:SendMessage on the target.
3. CreatePipe(Name=<name>, RoleArn=<role arn>, Source=<source arn>, SourceParameters={"SqsQueueParameters": {"BatchSize": 1}}, Target=<target arn>, TargetParameters={"InputTemplate": "<$.body>"}, DesiredState="RUNNING"). Poll DescribePipe until CurrentState is RUNNING.
4. SendMessage to the source, then poll ReceiveMessage on the target. Observe that nothing ever arrives and the source message is never consumed — this is where the check is cut at the harness cap, before any UpdatePipe is issued.
5. Call UpdatePipe(Name=<name>, RoleArn=<role arn>, DesiredState="STOPPED"). Observe 200 and DescribePipe reporting DesiredState/CurrentState STOPPED, with the field written even though nothing consumes it. Hold ~40 s re-reading DescribePipe and confirm CurrentState stays STOPPED.
6. SendMessage to the source again and drain the target for ~45 s: the body is absent, but vacuously so. Then call SQS GetQueueAttributes(ApproximateNumberOfMessages) on the SOURCE — once a data plane exists this must read 1, showing the record was withheld rather than consumed and dropped.
7. Call UpdatePipe(Name=<name>, RoleArn=<role arn>, DesiredState="RUNNING"), poll DescribePipe to CurrentState RUNNING, then poll ReceiveMessage on the target for the body sent in step 6. Observe it never arrives: the resume-and-drain-backlog path does not exist.
8. DeletePipe and poll DescribePipe until it answers HTTP 404 NotFoundException.
9. After fixing, re-replay pipes.update-pipe-desired-state-halts-then-resumes-delivery.
