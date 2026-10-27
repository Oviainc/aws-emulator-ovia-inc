Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.batch-partial-failure-envelope-fields sqs.batch-send-partial-mixed sqs.sqsbatch-send-full-width-one-invalid-entry-rejected sqs.sqsbatch-send-partial-failure-preserves-survivor-attributes sqs.sqsbatch-send-request-level-error-preempts-entry-failures-rejected sqs.create-queue-existing-name-different-policy-not-conflicting sqs.create-queue-existing-name-different-tags-rejected sqs.list-queues-pagination-bad-input-rejected sqs.list-message-move-tasks-max-results-out-of-range-rejected sqs.op-tag-and-permission-validation-rejected sqs.tag-queue-system-prefix-and-length-boundaries-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.batch-partial-failure-envelope-fields|run#19|AssertionError|assert bad.get("Code") == ERR_EMPTY_VALUE, f"Failed Code: {bad.get('Code')!r}"
Issue-Key: sqs.batch-send-partial-mixed|run#10|AssertionError|raise AssertionError(
Issue-Key: sqs.sqsbatch-send-full-width-one-invalid-entry-rejected|run#25|AssertionError|assert bad.get("Code") == ERR_EMPTY_VALUE, (
Issue-Key: sqs.sqsbatch-send-partial-failure-preserves-survivor-attributes|run#14|AssertionError|assert fail[ids[BAD_INDEX]].get("Code") == ERR_EMPTY_VALUE, (
Issue-Key: sqs.sqsbatch-send-request-level-error-preempts-entry-failures-rejected|run#25|AssertionError|assert fail[distinct_ids[BAD_BODY_INDEX]].get("Code") == ERR_EMPTY_VALUE, (
Issue-Key: sqs.create-queue-existing-name-different-policy-not-conflicting|_err#14|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.create-queue-existing-name-different-tags-rejected|run#9|AssertionError|assert msg == MSG_TAGS, f"a different tag value gave message {msg!r}"
Issue-Key: sqs.list-queues-pagination-bad-input-rejected|run#7|AssertionError|assert msg == MSG_MAX, f"MaxResults=1001 gave message {msg!r}"
Issue-Key: sqs.list-message-move-tasks-max-results-out-of-range-rejected|run#19|AssertionError|assert msg == MSG_RANGE, f"MaxResults={bad} gave message {msg!r}"
Issue-Key: sqs.op-tag-and-permission-validation-rejected|run#9|AssertionError|assert err_tag.get("Message") == MSG_TAG, f"got {err_tag!r}"
Issue-Key: sqs.tag-queue-system-prefix-and-length-boundaries-rejected|run#9|AssertionError|assert (err.get("Code"), err.get("Message")) == ("InvalidParameterValue", msg), (
Issue-Commit: 8bf24a185681652e6c0ae734129bf03e6b79d2f1

# sqs: 5 net-new behavioral root causes — SendMessageBatch per-entry code for an empty body, CreateQueue Policy/tags conflict detection, and three emulator-authored refusal-message templates

> _Note: this issue was identified against the code base at commit `8bf24a185681652e6c0ae734129bf03e6b79d2f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessageBatch

## Description
Five independent net-new root causes in the SQS emulator, all grounded in failing behavioral checks that the replay report marked neither ALREADY FILED nor OWNED. None of them is covered by the open sqs issues the report does cite (20261025T065401Z-sqs-refusal-taxonomy-input-validation-refusals-answer-validationexception.md, 20261025T182751Z-sqs-7-net-new-batch-message-move-root.md, 20261025T122517Z-sqs-deletemessagebatch-s-receipthandleisinvalid-entry-message-is-re.md): those claim ValidationException-vs-InvalidParameterValue taxonomy, the batch/message-move causes already listed there, and DeleteMessageBatch's entry message respectively, while the causes below are a different per-entry code, a missing attribute comparison, and three distinct message templates. Fix all five before submitting.

=== Cause 1 of 5: SendMessageBatch reports an empty MessageBody entry as `MissingParameter`, not `EmptyValue` ===
AWS: Smithy declares SendMessageBatch's output member `Failed` (BatchResultErrorEntryList) and the request-level errors only; BatchResultErrorEntry.Code is not an enum in the model, so this is recorded behavior only. Recorded against real SQS: a batch entry whose MessageBody is empty (or absent) fails ALONE, under its own request `Id`, with BatchResultErrorEntry.Code = "EmptyValue", SenderFault = true and a non-empty Message; the other entries in the same batch are enqueued and the request is HTTP 200. The variable part of that entry is its `Id`, echoed verbatim from the request; the Code is fixed text for this fault class.
Emulator: the same entry comes back with Code = 'MissingParameter' (message "The request must contain the parameter MessageBody."). Id echoing, SenderFault, the 200 status and the all-but-this-entry enqueue are all correct — only the code is wrong.
Why: a client that classifies batch outcomes by BatchResultErrorEntry.Code (the documented way to decide which entries to repair vs. retry) sees a code AWS never emits for this fault. Code-driven retry wrappers, SDK batch helpers and dead-letter routers either fall through to a default branch or mis-treat a permanently invalid entry, and the distinction between "you forgot a parameter" and "you sent an empty value" is lost.
Fix: the per-entry judgement in SendMessageBatch's handler — the empty-body arm of its entry-preparation function must use the EmptyValue code for this fault class instead of the generic missing-parameter code.
Traces: services/service-com-amazonaws-sqs-amazonsqs/src/ops/send_message_batch.rs lines 490-500 (the `entry.message_body.is_empty()` arm returns MISSING_PARAMETER). Failing checks: sqs.batch-partial-failure-envelope-fields, sqs.batch-send-partial-mixed, sqs.sqsbatch-send-full-width-one-invalid-entry-rejected, sqs.sqsbatch-send-partial-failure-preserves-survivor-attributes, sqs.sqsbatch-send-request-level-error-preempts-entry-failures-rejected.

=== Cause 2 of 5: CreateQueue's existing-name idempotency check leaves the Policy attribute out entirely, so a genuinely conflicting Policy re-create is ACCEPTED ===
AWS: Smithy: CreateQueue models QueueNameExists with awsQueryError code "QueueAlreadyExists" (HTTP 400), documented as "A queue with this name already exists. Amazon SQS returns this error only if the request includes attributes whose values differ from those of the existing queue." Recorded behavior: the Policy attribute IS part of that comparison, with one exclusion — a statement's `Sid`. Re-creating an existing queue with a Policy that differs only in Sid is accepted as identical, returns the SAME QueueUrl and does not overwrite the stored Sid; re-creating it with a Policy whose Effect, Action, Resource or Principal actually differs is refused QueueAlreadyExists with message "A queue already exists with the same name and a different value for attribute Policy" (fixed text; the trailing word is the conflicting attribute's name, taken from the request), and the stored Policy is left untouched.
Emulator: Policy is unconditionally excluded from the comparison, so a re-create whose Policy changes Action, Resource or Principal is answered HTTP 200 with the existing QueueUrl instead of 400 QueueAlreadyExists. (The positive control in the same check — a differing VisibilityTimeout — is refused correctly, so the general mechanism works; Policy is simply skipped.)
Why: a caller that re-asserts a queue through CreateQueue and changes the grant is told its new policy was accepted when nothing was applied, so the queue silently keeps the OLD grant. That is a security-relevant divergence in both directions: a tightening never lands, and a widening is reported as successful.
Fix: the re-create comparison in the SQS state module — Policy must be compared on its semantic fields rather than excluded, with Sid specifically ignored, and the refusal must carry the modelled QueueNameExists/QueueAlreadyExists channel.
Traces: services/service-com-amazonaws-sqs-amazonsqs/src/state.rs line 439 (`ATTRIBUTES_NOT_COMPARED_ON_RECREATE: &[&str] = &[POLICY]`) and lines 1892-1904 (`recreate_refusal`). Failing check: sqs.create-queue-existing-name-different-policy-not-conflicting.

=== Cause 3 of 5: CreateQueue's tags-conflict refusal message is worded as an attribute conflict ===
AWS: recorded behavior only (the model gives QueueNameExists no message template). Real SQS, when a re-create of an existing name supplies a different tag set, answers QueueAlreadyExists with the message "A queue already exists with the same name and a different value for tags " — all fixed text, lower-case "tags", no "attribute" keyword, and a trailing space; it carries no request- or state-derived values. The stored tags are left intact and an identical re-create returns the same URL.
Emulator: the same refusal carries "A queue already exists with the same name and a different value for attribute Tags" — it reuses the per-ATTRIBUTE template with the literal name "Tags", which is not an attribute name at all.
Why: the code is right but the message is not AWS's. IaC and test harnesses that re-run CreateQueue to change tags match on this message to distinguish a tag conflict from an attribute conflict (the remedy differs: TagQueue vs. SetQueueAttributes), and the two are indistinguishable here.
Fix: the tags branch of the re-create comparison in the SQS state module needs its own message template, distinct from the attribute template.
Traces: services/service-com-amazonaws-sqs-amazonsqs/src/state.rs lines 1905-1910. Failing check: sqs.create-queue-existing-name-different-tags-rejected.

=== Cause 4 of 5: one emulator-authored MaxResults range message is shared by two listings that word it differently on AWS ===
AWS: Smithy documents the ranges (ListQueuesRequest$MaxResults "Value range is 1 to 1000"; ListMessageMoveTasksRequest$MaxResults "The upper limit is 10"), but not the refusal text — recorded behavior only. Recorded: each listing refuses an out-of-range MaxResults with code InvalidParameterValue and its OWN message, and neither echoes the offending value back:
  * ListQueues: "Value for parameter MaxResults is invalid. Reason: MaxResults must be an integer between 1 and 1000."
  * ListMessageMoveTasks: "Value for parameter MaxResults is invalid. Reason: You must enter a number that's between 1 and 10."
In both, everything is fixed text except the bounds, which are that parameter's documented range.
Emulator: both operations share a single template that interpolates the supplied value and a generic reason — 'Value 1001 for parameter MaxResults is invalid. Reason: must be between 1 and 1000.' for ListQueues and 'Value 0 for parameter MaxResults is invalid. Reason: must be between 1 and 10.' for ListMessageMoveTasks. Code and status are right; the text is the emulator's own, and the deliberate "one template so two operations cannot drift" design is itself the divergence, because AWS's two messages DO differ.
Why: a client or test asserting on the documented refusal text does not match, and the emulator's extra echo of the rejected value changes the message shape for every page-size refusal in the service.
Fix: per-operation refusal message templates for the page-size parameter in the SQS state module's page-size validation helper (and its two call sites).
Traces: services/service-com-amazonaws-sqs-amazonsqs/src/state.rs lines 515-531 (`max_results_refusal`, the single shared template). Failing checks: sqs.list-queues-pagination-bad-input-rejected, sqs.list-message-move-tasks-max-results-out-of-range-rejected.

=== Cause 5 of 5: queue-tag validation refusals carry emulator-authored messages instead of the AWS-wide tag-rule messages ===
AWS: recorded behavior only (TagQueue models no InvalidParameterValue shape). Real SQS refuses the three tag-rule violations with code InvalidParameterValue and these messages, each entirely fixed text with no request- or state-derived parts — notably the rejected key/value is NOT quoted back, and the limits are stated as the rule rather than as a measurement:
  * a tag value longer than 256 characters -> "Tag values must be between 0 and 256 characters in length."
  * an empty key, or a key longer than 128 characters (TagQueue or UntagQueue) -> "Tag keys must be between 1 and 128 characters in length."
  * a key in the reserved aws: namespace -> "Caller is an end user and not allowed to mutate system tags"
A refused TagQueue adds none of its tags and a refused UntagQueue removes nothing.
Emulator: the shared tag-refusal builder produces its own wording that names the parameter and measures the input: 'Value for parameter TagValue is invalid. Reason: the value of tag key Environment must be at most 256 characters long, but was 300.' for the oversized value, 'Value aws:owner for parameter TagKey is invalid. Reason: must not start with the reserved prefix aws:.' for the reserved prefix, and the analogous 'Value for parameter TagKey is invalid. Reason: must be between 1 and 128 characters long, but was N.' for key length. The codes are right; every message is wrong, and the aws:-prefix case is additionally mis-classed as a parameter-format problem rather than as the system-tag refusal AWS gives it.
Why: tagging modules surface these messages to operators verbatim and some assert on them to tell a user-fixable tag-rule violation from a permissions problem; the emulator's text also leaks the rejected key back into the message, which AWS does not do.
Fix: the tag-validation refusal messages in the SQS state module (used by CreateQueue, TagQueue and UntagQueue) must emit AWS's three fixed texts, with the reserved-prefix case worded as the system-tag refusal.
Traces: services/service-com-amazonaws-sqs-amazonsqs/src/state.rs lines 1824-1856 (`tag_refusal`), reached from create_queue.rs lines 229-234. Failing checks: sqs.tag-queue-system-prefix-and-length-boundaries-rejected, sqs.op-tag-and-permission-validation-rejected (this one stops at its first assertion, the oversized tag value; its later RemovePermission/AddPermission assertions were never reached and are not claimed here — AddPermission is covered by its own already-filed stub issue).

## Evidence
Cause 1 — sqs.batch-partial-failure-envelope-fields: AssertionError: Failed Code: 'MissingParameter' (expected 'EmptyValue'). sqs.batch-send-partial-mixed: AssertionError: expected Failed code EmptyValue, got 'MissingParameter'. sqs.sqsbatch-send-full-width-one-invalid-entry-rejected: AssertionError: Failed['Entry_04-eCHO'].Code is 'MissingParameter', expected 'EmptyValue'. sqs.sqsbatch-send-partial-failure-preserves-survivor-attributes: AssertionError: Failed Code is 'MissingParameter', expected 'EmptyValue'. sqs.sqsbatch-send-request-level-error-preempts-entry-failures-rejected: AssertionError: per-entry Code is 'MissingParameter', expected 'EmptyValue'.

Cause 2 — sqs.create-queue-existing-name-different-policy-not-conflicting: AssertionError: expected the call to be refused, but it succeeded: {'QueueUrl': '<existing queue url>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. Expected: 400, Code 'QueueAlreadyExists', Message 'A queue already exists with the same name and a different value for attribute Policy', stored Policy unchanged. The same check's VisibilityTimeout positive control IS refused correctly, so only Policy is skipped.

Cause 3 — sqs.create-queue-existing-name-different-tags-rejected: AssertionError: a different tag value gave message 'A queue already exists with the same name and a different value for attribute Tags'. Expected (measured on real SQS us-west-2): 'A queue already exists with the same name and a different value for tags ' (note lower-case 'tags' and the trailing space). Code 'QueueAlreadyExists' matched.

Cause 4 — sqs.list-queues-pagination-bad-input-rejected: AssertionError: MaxResults=1001 gave message 'Value 1001 for parameter MaxResults is invalid. Reason: must be between 1 and 1000.'; expected 'Value for parameter MaxResults is invalid. Reason: MaxResults must be an integer between 1 and 1000.'. sqs.list-message-move-tasks-max-results-out-of-range-rejected: AssertionError: MaxResults=0 gave message 'Value 0 for parameter MaxResults is invalid. Reason: must be between 1 and 10.'; expected 'Value for parameter MaxResults is invalid. Reason: You must enter a number that's between 1 and 10.'. Code InvalidParameterValue matched in both.

Cause 5 — sqs.op-tag-and-permission-validation-rejected: AssertionError: got {'Message': 'Value for parameter TagValue is invalid. Reason: the value of tag key Environment must be at most 256 characters long, but was 300.', 'Code': 'InvalidParameterValue'}; expected Message 'Tag values must be between 0 and 256 characters in length.'. sqs.tag-queue-system-prefix-and-length-boundaries-rejected: AssertionError: TagQueue with a aws: key answered {'Message': 'Value aws:owner for parameter TagKey is invalid. Reason: must not start with the reserved prefix aws:.', 'Code': 'InvalidParameterValue'}; expected Message 'Caller is an end user and not allowed to mutate system tags'.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, then re-run exactly the checks listed in trace_ids.

Cause 1: CreateQueue, then SendMessageBatch with Entries=[{Id:'valid-1',MessageBody:'a'},{Id:'empty-2',MessageBody:''},{Id:'valid-3',MessageBody:'b'}]. Inspect Failed[0].Code — it must be 'EmptyValue' (SenderFault true, 200, the two siblings enqueued).

Cause 2: CreateQueue(name, Attributes={VisibilityTimeout:'40', Policy:<statement with Sid 'a'>}). Re-create with a Policy differing only in Sid -> must return the same QueueUrl and leave the stored Sid at 'a'. Re-create with a Policy whose Action (or Resource, or Principal) differs -> must be refused 400 QueueAlreadyExists with 'A queue already exists with the same name and a different value for attribute Policy', stored Policy unchanged. Today that last call returns 200.

Cause 3: CreateQueue(name, Attributes={VisibilityTimeout:'40'}, tags={a:'1'}), then CreateQueue with the same name/attributes but tags={a:'2'}. Inspect the refusal message.

Cause 4: ListQueues(QueueNamePrefix=p, MaxResults=1001) and ListMessageMoveTasks(SourceArn=arn, MaxResults=0). Inspect each message.

Cause 5: TagQueue(url, Tags={'Environment': 'v'*300}); TagQueue(url, Tags={'aws:owner':'me'}); TagQueue(url, Tags={'':'v'}); UntagQueue(url, TagKeys=['k'*129]). Inspect each message, and confirm no tag was added or removed by the refused call.
