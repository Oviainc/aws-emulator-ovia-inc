Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.fifo-source-preserves-message-order pipes.pipe-preserves-fifo-message-ordering pipes.concurrent-start-pipe-single-winner
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes.fifo-source-preserves-message-order|run#17|Timeout|resp = sqs.receive_message(QueueUrl=tgt_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: pipes.pipe-preserves-fifo-message-ordering|_poll_for_n_messages#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: pipes.concurrent-start-pipe-single-winner|run#14|AssertionError|assert len(successes) == 1, f"expected exactly one StartPipe success, got {results}"
Issue-Commit: 8e40a1008c33b06d6b244a457efc673d78e6bf69

# Pipes data plane: a FIFO-sourced RUNNING pipe never delivers (per-group ordering unobservable), and StartPipe never refuses a second concurrent start with ConflictException

> _Note: this issue was identified against the code base at commit `8e40a1008c33b06d6b244a457efc673d78e6bf69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
This bundle has 2 causes; both must be closed before submitting. Both are grounded in failing checks of this run that no filed issue carries: the open issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md describes the same two defect families, but neither of the FIFO-ordering checks appears in its trace list and the replay did not report pipes.concurrent-start-pipe-single-winner as owned by it, so these checks are unclaimed work items. A fixer closing that issue should re-replay this issue's trace_ids too.

Cause 1 of 2: a RUNNING pipe whose source is a FIFO SQS queue never moves any record to its FIFO target, so per-message-group ordering can never be observed.
AWS: recorded behavior only — the Smithy model (com.amazonaws.pipes) describes only the control plane. What the model does carry is the configuration this behavior is driven by: CreatePipe accepts SourceParameters -> PipeSourceParameters (PipeSourceSqsQueueParameters.BatchSize @range 1..10000, MaximumBatchingWindowInSeconds @range 0..300) and TargetParameters -> PipeTargetSqsQueueParameters (MessageGroupId), and DescribePipe reports CurrentState from PipeState, whose values include RUNNING. The recorded contract exercised by both checks: once the pipe reports RUNNING, the SERVICE polls the FIFO source with no client action and sends each record to the FIFO target under the configured MessageGroupId, preserving the order the messages were sent within a group.
Emulator: both checks reach the harness cap — 'Check timed out after 60.0s' — while polling the target queue for the first record. Nothing is received from the source and nothing is sent to the target. The crate says so itself: services/service-com-amazonaws-pipes-pipes/src/state.rs (module doc) states 'the control plane (the record, its ARN, its lifecycle states and its timestamps) is real and persisted here; nothing polls the source or delivers to the target', and the only time-driven machinery in the crate is ComAmazonawsPipesPipesState::settle_due_transitions, which flips CurrentState.
Why: a client that wires a FIFO queue through a pipe sees CurrentState RUNNING and then loses every message: nothing is consumed at the source, nothing arrives at the target, and the ordering guarantee the client chose FIFO for cannot be relied on or tested. The failure is silent — the pipe's own state reports health.
Fix: a pipes data-plane/dispatch engine in services/service-com-amazonaws-pipes-pipes (a per-pipe poller driven by the stored SourceParameters that calls the emulator's own sqs surface, sends to the target under TargetParameters.SqsQueueParameters.MessageGroupId, and serializes delivery per message group so order within a group is preserved), plus the in-flight-batch state it needs.
Traces: pipes.fifo-source-preserves-message-order, pipes.pipe-preserves-fifo-message-ordering.

Cause 2 of 2: two back-to-back StartPipe calls on the same STOPPED pipe BOTH succeed; the loser is never refused with ConflictException.
AWS: Smithy (com.amazonaws.pipes#StartPipe) models ConflictException (HTTP 409) with required members message, resourceId and resourceType, alongside NotFoundException (404), ValidationException (400), ThrottlingException (429) and InternalException (500). ConflictException is documented as 'An action you attempted resulted in an exception' and is what AWS returns when the requested transition is not legal for the pipe's CurrentState — so of two simultaneous StartPipe calls on a STOPPED pipe exactly one is accepted and the other gets 409 ConflictException while the pipe is STARTING, and the pipe still lands in RUNNING with no double transition. The requirement is the 409 status and the ConflictException code; the recorded message text is an example.
Emulator: both calls answer 200 — 'AssertionError: expected exactly one StartPipe success, got [(\'ok\', None, None), (\'ok\', None, None)]'. services/service-com-amazonaws-pipes-pipes/src/ops/start_pipe.rs sets desired_state and calls PipeRecord::begin_transition unconditionally after the name lookup; it never inspects pipe.current_state / pipe.transient_state(), so no transition is ever gated and ConflictException is never constructed.
Why: a client cannot use 409 to serialize control-plane actions. Two racing deploys both believe they drove the transition, and the ConflictException retry/backoff path SDK users write is dead code against this emulator.
Fix: the StartPipe handler and the pipe state machine in services/service-com-amazonaws-pipes-pipes — gate the transition on CurrentState under the state lock (TRANSIENT_PIPE_STATES / RUNNING already exist in state.rs) and return the modeled ConflictException (409) with resourceId = pipe name and resourceType = PIPE_RESOURCE_TYPE for an illegal or already-in-flight start.
Traces: pipes.concurrent-start-pipe-single-winner.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 30/107 passed (28.0%); pipes 30/94 with 64 failures. No bootstrap signal — the emulator was serving (CreatePipe/DescribePipe/StartPipe and the SQS/IAM calls in these checks answered 200, and 30 pipes checks passed). Service 'pipes' is bootstrapped.

Cause 1:
  pipes.fifo-source-preserves-message-order — Error: Check timed out after 60.0s. The check creates a FIFO source and FIFO target, a pipes role, a pipe with SourceParameters={SqsQueueParameters:{BatchSize:1}} and DesiredState RUNNING, sends an ordered sequence into one message group and then polls the target.
  pipes.pipe-preserves-fifo-message-ordering — Error: Check timed out after 60.0s (same shape: 'Messages sent in sequence to a FIFO source queue are delivered by the pipe to a FIFO target queue, within the same message group, in the same order').
  Expected: the records arrive at the FIFO target, in send order within the group. Actual: the target stays empty for the whole budget and the source message is never consumed.
  Code corroboration: services/service-com-amazonaws-pipes-pipes/src/state.rs module doc ('nothing polls the source or delivers to the target'); no poller/dispatcher exists in the crate.

Cause 2:
  pipes.concurrent-start-pipe-single-winner — File "check_pipes_concurrent_start_pipe_single_winner.py", line 69, in run: assert len(successes) == 1 -> AssertionError: expected exactly one StartPipe success, got [('ok', None, None), ('ok', None, None)].
  Expected: one call 200, the other HTTP 409 with error code ConflictException (message is an example only), and the pipe ends in RUNNING.
  Actual: HTTP 200 for both callers.
  Code: services/service-com-amazonaws-pipes-pipes/src/ops/start_pipe.rs:166-175 — name lookup, then begin_transition(STARTING, RUNNING) with no state gate.

Generated pipe/queue/role names, ARNs, hosts, ports, timestamps and request ids differ per run and are examples only. The requirements are: a RUNNING FIFO-sourced pipe moves records source->target preserving per-group order, and an illegal/concurrent StartPipe is HTTP 409 ConflictException.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Cause 1: create a FIFO source queue and a FIFO target queue (ContentBasedDeduplication=true) plus an IAM role allowing sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source and sqs:SendMessage on the target. Call CreatePipe(Name, RoleArn, Source=<fifo src arn>, SourceParameters={SqsQueueParameters:{BatchSize:1}}, Target=<fifo tgt arn>, TargetParameters={SqsQueueParameters:{MessageGroupId:'g1'}}, DesiredState='RUNNING'); wait for DescribePipe CurrentState=RUNNING; SendMessage three bodies in order into one MessageGroupId on the source; poll ReceiveMessage on the target. Observe nothing ever arrives (and nothing is consumed from the source).
3. Cause 2: create a pipe with DesiredState='STOPPED', wait for CurrentState=STOPPED, then issue two StartPipe calls on that name back-to-back from two threads. Observe both return HTTP 200 instead of exactly one 200 and one HTTP 409 ConflictException.
4. After fixing, re-replay the full trace_ids list of this issue (and the related open issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md, which describes the same families for other checks).
