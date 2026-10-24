Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.update-pipe-source-arn-immutable-rejected pipes.update-pipe-source-type-mismatch-rejected pipes.update-pipe-wrong-source-type-parameters-rejected pipes.update-pipe-unknown-name-rejected-not-created pipes.update-pipe-while-creating-rejected pipes.concurrent-update-pipe-no-corruption
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes.update-pipe-source-arn-immutable-rejected|run#10|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: pipes.update-pipe-source-type-mismatch-rejected|run#12|AssertionError|assert message == (
Issue-Key: pipes.update-pipe-wrong-source-type-parameters-rejected|run#11|AssertionError|assert message == (
Issue-Key: pipes.update-pipe-unknown-name-rejected-not-created|run#12|AssertionError|assert message == f"Pipe {ghost} does not exist.", f"unexpected UpdatePipe message {message!r}"
Issue-Key: pipes.update-pipe-while-creating-rejected|run#12|AssertionError|assert message == f"Pipe {name} is updating concurrently.", f"unexpected UpdatePipe message {message!r}"
Issue-Key: pipes.concurrent-update-pipe-no-corruption|run#18|AssertionError|assert message == expected_msg, f"expected UpdatePipe refusal {expected_msg!r}, got {message!r}"
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Pipes UpdatePipe refusals carry emulator-authored bodies: the ValidationErrorFields JSON is missing for a source-type mismatch, and the NotFound/Conflict message templates are not Pipes' (3 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePipe

## Description
This bundle has 3 causes; all three must be closed before submitting. All three are UpdatePipe refusals of the pipes service where the HTTP status and the error code are already correct and the response BODY is what diverges. UpdatePipe is now implemented (in the previous run of this suite it answered the non-modeled NotImplemented), so these are the defects the implementation left behind.

Relationship to what is already filed: the open issue 20261023T235727Z-pipes-control-plane-cross-resource-createpipe-validation-is.md lists pipes.update-pipe-source-arn-immutable-rejected, pipes.update-pipe-source-type-mismatch-rejected and pipes.update-pipe-wrong-source-type-parameters-rejected in its Issue-Trace-Ids, but its replay keys for them are 'expected ValidationException ... got NotImplemented' (its Cause 6, standing the operation up). That key no longer matches: UpdatePipe now returns the modeled ValidationException and the failure has moved to the message body, which is why this run did not mark these checks OWNED. Its Cause 2 (Pipes-style ValidationException bodies) and Cause 3 (the not-found template) describe the same CLASS for CreatePipe/Start/Stop/Delete/Describe but neither names UpdatePipe, and neither carries pipes.update-pipe-unknown-name-rejected-not-created, pipes.update-pipe-while-creating-rejected or pipes.concurrent-update-pipe-no-corruption. Fix this issue together with that one if the same code path produces all of them.

Cause 1 of 3: UpdatePipe refuses SourceParameters belonging to a source type other than the pipe's own, but the ValidationException message is prose instead of Pipes' ValidationErrorFields JSON document, and it names the wrong reason.
AWS: Smithy (com.amazonaws.pipes#UpdatePipe) models ValidationException with errorKind client and httpError 400, whose members are message and fieldList (ValidationExceptionFieldList). UpdatePipe exposes NO Source member at all (Source is @required on CreatePipe with cfnMutability create-and-read), and UpdatePipeSourceParameters has exactly one member per source type (SqsQueueParameters, DynamoDBStreamParameters, KinesisStreamParameters, ActiveMQBrokerParameters, ...), so the source type is immutable and supplying another type's member is a type mismatch. The recorded refusal is HTTP 400 / ValidationException whose message is a JSON document with one ValidationErrorFields entry: Name = the SourceParameters member that was supplied, Message = 'Invalid source parameter provided for source <the pipe's resolved source type>.' The recorded instances are '{"ValidationErrorFields":[{"Name":"DynamoDBStreamParameters","Message":"Invalid source parameter provided for source sqs."}]}' and the identical body with "KinesisStreamParameters". The rule is built from the request (the member supplied, interpolated with the pipe's actual source type) so the ENVELOPE AND THAT RULE are the requirement; the two literals above are examples of it.
Emulator: status 400 and code ValidationException are correct (both assertions pass), but the message is the emulator's own sentence — 'SourceParameters.DynamoDBStreamParameters cannot be updated: this pipe has no DynamoDBStreamParameters to update.' and 'SourceParameters.KinesisStreamParameters cannot be updated: this pipe has no KinesisStreamParameters to update.'. It is not JSON, the member name is reported dotted-prefixed ('SourceParameters.X' rather than 'X'), ValidationException.fieldList is not populated, and the stated reason is a missing-value one ('no X to update') rather than the type mismatch AWS reports.
Why: clients, the console and CLIs parse ValidationErrorFields to map a refusal back to the offending request field; against this emulator that parse finds no JSON and no fieldList, so the failure cannot be attributed to a field. Worse, a client that reads the reason concludes its pipe merely lacks those parameters and retries after 'adding' them, when AWS is saying the source type can never accept them.
Fix: the UpdatePipe handler's source-parameter validation and the pipes ValidationException construction/serialization in services/service-com-amazonaws-pipes-pipes — compare the supplied UpdatePipeSourceParameters member against the pipe's stored source type, and emit the Pipes-style ValidationErrorFields body (plus fieldList) naming the supplied member and the resolved source type.
Traces: pipes.update-pipe-source-arn-immutable-rejected, pipes.update-pipe-source-type-mismatch-rejected, pipes.update-pipe-wrong-source-type-parameters-rejected.

Cause 2 of 3: UpdatePipe on a name that does not exist uses the wrong NotFoundException message template.
AWS: Smithy: UpdatePipe models NotFoundException with errorKind client and httpError 404, carrying a message member. The message is built from the request — the requested name interpolated into a fixed sentence, 'Pipe <name> does not exist.' — which is the same template this suite records for DescribePipe/StartPipe/StopPipe/DeletePipe. That rule is the requirement, not the recorded literal. UpdatePipe is not an upsert: nothing is created by the refused call.
Emulator: the 404 and NotFoundException are correct and no pipe is created, but the message is 'Pipe not found: <name>'.
Why: this is the narrowest cause in the bundle — classification and absence are both right. Clients and test suites that assert on the documented sentence, and operators reading logs, see a different message built from the same request, and the emulator's template differs from the one AWS uses on every other pipes operation.
Fix: the shared not-found construction used by the pipes handlers in services/service-com-amazonaws-pipes-pipes — one template, 'Pipe <name> does not exist.', for UpdatePipe as well.
Traces: pipes.update-pipe-unknown-name-rejected-not-created.

Cause 3 of 3: the ConflictException raised when UpdatePipe lands during an in-flight transition reports the pipe's CurrentState instead of Pipes' single concurrent-update sentence.
AWS: Smithy: UpdatePipe models ConflictException with errorKind client and httpError 409, whose message, resourceId and resourceType are all @required. The recorded message is built from the request name and is ONE sentence regardless of which transition is in flight — 'Pipe <name> is updating concurrently.' — recorded both for an UpdatePipe issued while the pipe is still CREATING from CreatePipe and for the loser of two concurrent UpdatePipe calls on a RUNNING pipe. The rule (one fixed sentence naming the pipe, no state leaked) is the requirement.
Emulator: the 409 and ConflictException are correct, but the message is state-specific prose: 'Pipe <name> is in state CREATING and cannot be updated.' and 'Pipe <name> is in state UPDATING and cannot be updated.'
Why: classification is right, so clients retry correctly; but the emulator exposes the internal CurrentState in the refusal and uses a different sentence per state, so a client cannot distinguish 'another update is in flight' the way AWS words it, and assertions on the documented sentence fail.
Fix: the conflict branch of the UpdatePipe handler in services/service-com-amazonaws-pipes-pipes — one message template for a transition already in flight, with resourceId/resourceType populated as the model requires.
Traces: pipes.update-pipe-while-creating-rejected, pipes.concurrent-update-pipe-no-corruption.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1), this run: Total 14/107 passed (13.1%); pipes 14/94 with 80 failures; Mismatched 76, Errored 17, Skipped 0, Unloadable 0. No bootstrap signal — the emulator was serving: CreatePipe, DescribePipe, UpdatePipe (positive control), DeletePipe, SQS and IAM calls all answered 200 in these checks and 14 pipes checks passed. None of the six checks below was marked ALREADY FILED or OWNED in this run's report.

Cause 1 (HTTP 400 and code ValidationException both asserted and both correct; only the message body diverges):
  pipes.update-pipe-source-arn-immutable-rejected — the check asserts code == 'ValidationException' (passes) and status == 400 (passes), then: AssertionError: expected the message '{"ValidationErrorFields":[{"Name":"DynamoDBStreamParameters","Message":"Invalid source parameter provided for source sqs."}]}', got 'SourceParameters.DynamoDBStreamParameters cannot be updated: this pipe has no DynamoDBStreamParameters to update.'
  pipes.update-pipe-source-type-mismatch-rejected — AssertionError: unexpected UpdatePipe message 'SourceParameters.DynamoDBStreamParameters cannot be updated: this pipe has no DynamoDBStreamParameters to update.' (expected the same ValidationErrorFields body).
  pipes.update-pipe-wrong-source-type-parameters-rejected — AssertionError: unexpected UpdatePipe message 'SourceParameters.KinesisStreamParameters cannot be updated: this pipe has no KinesisStreamParameters to update.' (expected '{"ValidationErrorFields":[{"Name":"KinesisStreamParameters","Message":"Invalid source parameter provided for source sqs."}]}').
  Request in all three: UpdatePipe(Name=<sqs-sourced pipe>, RoleArn=<its role>, SourceParameters={DynamoDBStreamParameters:{BatchSize:5}}) or the Kinesis equivalent. The checks' later legs — Source and SqsQueueParameters survive the rejection, the wrong member is not applied, and a same-type SourceParameters update still succeeds — were not reached.

Cause 2 (HTTP 404 / NotFoundException correct, message template wrong):
  pipes.update-pipe-unknown-name-rejected-not-created — AssertionError: unexpected UpdatePipe message 'Pipe not found: sim2real-pipes-updnf-ghost-<id>'; expected 'Pipe <that name> does not exist.'

Cause 3 (HTTP 409 / ConflictException correct, message wrong):
  pipes.update-pipe-while-creating-rejected — AssertionError: unexpected UpdatePipe message 'Pipe sim2real-pipes-updc-<id> is in state CREATING and cannot be updated.'; expected 'Pipe <name> is updating concurrently.'
  pipes.concurrent-update-pipe-no-corruption — AssertionError: expected UpdatePipe refusal 'Pipe sim2real-pipes-concupd-<id> is updating concurrently.', got 'Pipe sim2real-pipes-concupd-<id> is in state UPDATING and cannot be updated.'

Generated pipe names, queue URLs/ARNs, role ARNs, hosts, ports, timestamps and request ids differ per run and are examples only. The requirements are: HTTP 400 + ValidationException carrying the Pipes ValidationErrorFields envelope (and fieldList) built from the supplied SourceParameters member and the pipe's resolved source type; HTTP 404 + NotFoundException with the 'Pipe <name> does not exist.' template; HTTP 409 + ConflictException with one concurrent-update template naming the pipe and no CurrentState leaked.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Create two SQS queues and a role pipes can assume, then CreatePipe(Name, RoleArn, Source=<src queue arn>, Target=<tgt queue arn>, SourceParameters={SqsQueueParameters:{BatchSize:1}}) and wait for a settled CurrentState.
3. Cause 1: call UpdatePipe(Name=<that pipe>, RoleArn=<its role>, SourceParameters={DynamoDBStreamParameters:{BatchSize:5}}) and then the same with {KinesisStreamParameters:{BatchSize:5}}. Both correctly answer HTTP 400 / ValidationException, but the message is 'SourceParameters.<Member> cannot be updated: this pipe has no <Member> to update.' instead of the ValidationErrorFields JSON naming <Member> and source 'sqs', and ValidationException.fieldList is empty.
4. Cause 2: call UpdatePipe with a Name that was never created. Observe HTTP 404 / NotFoundException with message 'Pipe not found: <name>' instead of 'Pipe <name> does not exist.' (and confirm no pipe is created, which already holds).
5. Cause 3: call CreatePipe and, while CurrentState is still CREATING, call UpdatePipe on the same name; separately, fire two UpdatePipe calls with different SourceParameters at a settled pipe from two threads. The refusals are HTTP 409 / ConflictException with 'Pipe <name> is in state CREATING|UPDATING and cannot be updated.' instead of 'Pipe <name> is updating concurrently.'
6. After fixing, re-replay the full trace_ids list of this issue.
