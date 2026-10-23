Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.create-pipe-batch-size-above-sqs-target-cap-rejected pipes.pipe-filter-blocks-nonmatching-events pipes.create-pipe-malformed-source-arn-rejected pipes.create-pipe-name-outside-the-allowed-characters-rejected pipes.description-max-length-exceeded-rejected pipes.tag-resource-exceeding-max-tags-rejected pipes.untag-resource-malformed-empty-key-rejected pipes.start-nonexistent-pipe-not-found pipes.start-pipe-nonexistent-not-found pipes.stop-nonexistent-pipe-not-found pipes.stop-pipe-nonexistent-not-found pipes.delete-running-pipe-auto-stops pipes.filter-criteria-nested-json-preserved pipes.create-pipe-duplicate-name-rejected pipes.pipes-create-pipe-duplicate-name-rejected-without-overwriting pipes.empty-vs-absent-description-distinguished pipes.pipe-description-empty-normalized-to-absent pipes.tag-resource-nonexistent-pipe-not-found pipes.tag-resource-invalid-key-validation-rejected pipes.tag-resource-on-non-pipe-arn-rejected pipes.tag-resource-with-aws-reserved-prefix-rejected pipes.untag-resource-nonexistent-pipe-not-found pipes.list-tags-for-resource-nonexistent-pipe-not-found pipes.update-pipe-source-arn-immutable-rejected pipes.update-pipe-source-type-mismatch-rejected pipes.update-pipe-wrong-source-type-parameters-rejected
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes.create-pipe-batch-size-above-sqs-target-cap-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: pipes.pipe-filter-blocks-nonmatching-events|run#6|AssertionError|raise AssertionError("CreatePipe accepted a malformed (non-JSON) FilterCriteria pattern")
Issue-Key: pipes.create-pipe-malformed-source-arn-rejected|run#12|AssertionError|assert message == f'[ECMA 262 regex "{_SOURCE_PATTERN}" does not match input string "{src_url}"]', (
Issue-Key: pipes.create-pipe-name-outside-the-allowed-characters-rejected|run#13|AssertionError|assert message == expected, f"refusal message was {message!r}, expected {expected!r}"
Issue-Key: pipes.description-max-length-exceeded-rejected|run#18|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: pipes.tag-resource-exceeding-max-tags-rejected|run#6|AssertionError|assert message == "[object has too many properties (found 51 but schema requires at most 50)]", (
Issue-Key: pipes.untag-resource-malformed-empty-key-rejected|run#6|AssertionError|assert message == "[tagKeys]", f"unexpected UntagResource message {message!r}"
Issue-Key: pipes.start-nonexistent-pipe-not-found|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: pipes.start-pipe-nonexistent-not-found|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: pipes.stop-nonexistent-pipe-not-found|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: pipes.stop-pipe-nonexistent-not-found|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: pipes.delete-running-pipe-auto-stops|run#19|AssertionError|assert message == expected_message, (
Issue-Key: pipes.create-pipe-duplicate-name-rejected|run#15|AssertionError|assert message == expected_message, (
Issue-Key: pipes.pipes-create-pipe-duplicate-name-rejected-without-overwriting|run#14|AssertionError|assert message == expected_message, (
Issue-Key: pipes.empty-vs-absent-description-distinguished|run#21|AssertionError|assert "Description" not in described_empty, (
Issue-Key: pipes.pipe-description-empty-normalized-to-absent|run#15|AssertionError|assert "Description" not in desc_empty, f"expected Description=\"\" to normalize to absent, got {desc_empty.get('Description')!r}"
Issue-Key: pipes.tag-resource-nonexistent-pipe-not-found|run#4|AssertionError|assert code == "NotFoundException", f"expected NotFoundException tagging a non-existent pipe, got {code!r}"
Issue-Key: pipes.tag-resource-invalid-key-validation-rejected|run#5|AssertionError|assert code == "BadRequestException", f"code={code!r}"
Issue-Key: pipes.tag-resource-on-non-pipe-arn-rejected|run#7|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: pipes.tag-resource-with-aws-reserved-prefix-rejected|run#4|AssertionError|assert code == "BadRequestException", f"code={code!r}"
Issue-Key: pipes.untag-resource-nonexistent-pipe-not-found|run#4|AssertionError|assert code == "NotFoundException", f"expected NotFoundException untagging a ghost pipe ARN, got {code}"
Issue-Key: pipes.list-tags-for-resource-nonexistent-pipe-not-found|run#4|AssertionError|assert code == "NotFoundException", f"expected NotFoundException for a never-created pipe ARN, got {code!r}"
Issue-Key: pipes.update-pipe-source-arn-immutable-rejected|run#7|AssertionError|assert code == "ValidationException", f"expected a rejection, got {code}"
Issue-Key: pipes.update-pipe-source-type-mismatch-rejected|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException for a source-type mismatch, got {code}"
Issue-Key: pipes.update-pipe-wrong-source-type-parameters-rejected|run#10|AssertionError|assert code == "ValidationException", (
Issue-Related: 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md
Issue-Commit: cfd0674e2ddfe499cbab1f11b70f1bc18d331a71

# Pipes control plane: cross-resource CreatePipe validation is missing, Description="" is not normalized, and the ValidationException/NotFoundException/ConflictException bodies are emulator-authored rather than Pipes' own (6 causes)

> _Note: this issue was identified against the code base at commit `cfd0674e2ddfe499cbab1f11b70f1bc18d331a71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
This bundle has 6 causes; all six must be closed before submitting. They are all control-plane fidelity defects of the pipes service, separate from the missing pipes runtime filed in the companion issue of this run ("EventBridge Pipes has no runtime ... (5 causes)").

Cause 1 of 6: CreatePipe accepts configurations AWS refuses because they violate a rule about the resource the ARN names, not the request shape.
AWS: Smithy (com.amazonaws.pipes#CreatePipe) models ValidationException (HTTP 400, with message and fieldList). The model's own range for PipeSourceSqsQueueParameters.BatchSize is @range 1..10000, so the real cap is set by the TARGET type: with an SQS target, AWS refuses BatchSize above 10 with ValidationException, message (recorded) '{"ValidationErrorFields":[{"Name":"BatchSize","Message":"The configured batch size 50 is greater than the max supported batch size 10 for target sqs."}]}'. FilterCriteria.Filters[].Pattern is modeled as a plain string, but AWS parses it as an event pattern and refuses a non-JSON pattern with ValidationException. (A third instance of the same class — a FIFO target with no TargetParameters.SqsQueueParameters.MessageGroupId, which AWS refuses with ValidationException — is exercised by pipes__sqs.pipes-create-pipe-fifo-target-without-message-group-id-rejected, a check listed in the runtime issue because its positive control hangs before it reaches the negative leg.)
Emulator: CreatePipe with SourceParameters={SqsQueueParameters:{BatchSize:50,MaximumBatchingWindowInSeconds:5}} and an SQS target returns HTTP 200 with CurrentState CREATING, and a malformed non-JSON FilterCriteria pattern is likewise accepted. services/service-com-amazonaws-pipes-pipes/src/ops/create_pipe.rs validates only DesiredState and the name's uniqueness/quota; it stores SourceParameters/TargetParameters verbatim and never resolves the target ARN or parses the filter pattern.
Why: a client gets a pipe that AWS would never have created. The misconfiguration is accepted silently, so the error surfaces (in real AWS) only at deploy time against the real service — exactly the class of bug an emulator exists to catch — and a template that the emulator validates cannot be trusted.
Fix: CreatePipe/UpdatePipe validation in the pipes handlers — resolve the target ARN's service and queue type, enforce the per-target batch-size cap and the FIFO MessageGroupId requirement, and parse FilterCriteria patterns as JSON event patterns, returning the modeled ValidationException (400).
Traces: pipes.create-pipe-batch-size-above-sqs-target-cap-rejected, pipes.pipe-filter-blocks-nonmatching-events.

Cause 2 of 6: constraint violations are reported with the generic boundary-validator template instead of the Pipes-specific ValidationException body.
AWS: Smithy: ValidationException (400) carries message plus fieldList (ValidationExceptionFieldList). Pipes builds that message from the request and the violated constraint, and the recorded forms are service-specific: a Source that does not match ArnOrUrl's pattern gives '[ECMA 262 regex "<the modeled pattern>" does not match input string "<value>"]'; a Name outside PipeName's pattern ^[\.\-_A-Za-z0-9]+$ gives '{"ValidationErrorFields":[{"Name":"PipeName","Message":"Invalid pattern for pipe name. Pipe name should match pattern ^[\\.\\-_A-Za-z0-9]+$."}]}'; a Description over PipeDescription's @length max 512 gives '[string "<value>" is too long (length: 513, maximum allowed: 512)]'; an over-50-entry TagMap and an empty tag key in UntagResource are reported in the same Pipes style. The rule is known (the member and its modeled constraint build the message), so the FORM is the requirement, not the recorded literal.
Emulator: every one of these returns the emulator's generic template, e.g. "1 validation error detected: Value '<value>' at 'source' failed to satisfy constraint: Member must satisfy regular expression pattern: ^smk://...", "... at 'name' ...", "... at 'description' failed to satisfy constraint: Member must have length less than or equal to 512", "Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50", "Value '' at 'tagKeys.1.member' failed to satisfy constraint: Member must have length greater than or equal to 1". The error code and 400 status are right; the body is not Pipes'.
Why: clients and CLIs surface these messages to users and some parse the ValidationErrorFields JSON to map a failure back to a form field; against this emulator that parse finds nothing and the field name is reported under a different casing ('name', 'source') than Pipes uses ('PipeName').
Fix: the pipes validation/serialization layer — emit the Pipes-style message and populate ValidationException.fieldList for constraint violations on Name, Source, Description and the tag members, instead of the shared boundary-validator text.
Traces: pipes.create-pipe-malformed-source-arn-rejected, pipes.create-pipe-name-outside-the-allowed-characters-rejected, pipes.description-max-length-exceeded-rejected, pipes.tag-resource-exceeding-max-tags-rejected, pipes.untag-resource-malformed-empty-key-rejected.

Cause 3 of 6: NotFoundException for an unknown pipe uses the wrong message template.
AWS: Smithy: NotFoundException (HTTP 404) with a message member, modeled on DescribePipe, StartPipe, StopPipe, DeletePipe and the tagging operations. The message is built from the request and the recorded template is 'Pipe <name> does not exist.' — a known rule (the requested name interpolated into a fixed sentence), so it is a requirement rather than an incidental string.
Emulator: the same 404/NotFoundException is returned with 'Pipe not found: <name>' on StartPipe, StopPipe, DeletePipe and DescribePipe of a never-created or already-deleted pipe.
Why: status and code are right, so this is the narrowest cause in this bundle — but clients and test suites that assert on the documented sentence (and operators reading logs) see a different message from the same request.
Fix: the shared not-found construction in the pipes handlers (services/service-com-amazonaws-pipes-pipes) — one template, 'Pipe <name> does not exist.'.
Traces: pipes.start-nonexistent-pipe-not-found, pipes.start-pipe-nonexistent-not-found, pipes.stop-nonexistent-pipe-not-found, pipes.stop-pipe-nonexistent-not-found, pipes.delete-running-pipe-auto-stops, pipes.filter-criteria-nested-json-preserved.

Cause 4 of 6: the duplicate-name ConflictException message is not the one AWS builds.
AWS: Smithy: CreatePipe models ConflictException (HTTP 409) with required message, resourceId and resourceType. The message is built from the request: recorded form 'Pipe <name> already exists.'
Emulator: the 409/ConflictException is raised correctly (create_pipe.rs:223-230) but with 'Pipe named <name> already exists.'
Why: as with Cause 3 the classification is right; the divergence is in the message rule, which clients assert on and which AWS derives from the request name.
Fix: the duplicate-name branch of the CreatePipe handler.
Traces: pipes.create-pipe-duplicate-name-rejected, pipes.pipes-create-pipe-duplicate-name-rejected-without-overwriting.

Cause 5 of 6: an explicit empty-string Description is round-tripped as "" instead of being normalized to absent.
AWS: Smithy: PipeDescription @length 0..512, so Description="" is model-valid; recorded behavior (stated in the check, confirmed against real AWS) is that CreatePipe with Description="" is normalized exactly like an omitted Description — DescribePipe returns NO Description key at all — while a non-empty Description is preserved.
Emulator: DescribePipe returns Description='' for the empty-string case ("explicit empty-string description should be normalized to absent (real AWS behavior), got ''"), so empty and absent are distinguishable where AWS makes them identical.
Why: clients that branch on the presence of Description (diffing a pipe against a desired configuration, or rendering a description only when present) see a spurious difference, and idempotent re-applies of the same template keep reporting drift.
Fix: the CreatePipe/UpdatePipe handler and the pipe record in services/service-com-amazonaws-pipes-pipes — store None for an empty Description so DescribePipe omits the member.
Traces: pipes.empty-vs-absent-description-distinguished, pipes.pipe-description-empty-normalized-to-absent.

Cause 6 of 6: the tagging surface and UpdatePipe answer the non-modeled NotImplemented, so none of their not-found, validation or immutability semantics can be observed.
AWS: Smithy: TagResource, UntagResource and ListTagsForResource each model NotFoundException (404) and ValidationException (400) only — tagging a non-existent pipe ARN is 404 NotFoundException, an ARN that is not a pipe ARN is refused (BadRequestException/ValidationException per the recorded response), a reserved aws: key prefix and an invalid tag key are ValidationException, and ListTagsForResource on a ghost ARN is 404. UpdatePipe models ValidationException for an immutable Source change and for SourceParameters that do not match the source type. NotImplemented is not a modeled error of any of them.
Emulator: these calls fail with 'An error occurred (NotImplemented) ... operation <X> is scaffolded but not implemented in this emulator' (observed as code='NotImplemented' where NotFoundException/ValidationException/BadRequestException was required). Note the suite already filed scaffold-stub issues for pipes TagResource (src/ops/tag_resource.rs), ListTagsForResource (src/ops/list_tags_for_resource.rs) and UpdatePipe (src/ops/update_pipe.rs) — those cover standing the operations up; the checks listed here were NOT marked as covered, UntagResource has no stub issue at all, and they additionally pin the error classification each operation owes, which is what must be true after the stubs are implemented.
Why: a client cannot tag, untag or read the tags of a pipe, and gets an error code no SDK models, so retry/classification logic treats a permanent client error as unknown. Nothing refuses an immutable Source change on UpdatePipe either.
Fix: implement pipes TagResource, UntagResource, ListTagsForResource and UpdatePipe over the pipe record's tag map, with the modeled 404/400 classification and the immutability rules for UpdatePipe.
Traces: pipes.tag-resource-nonexistent-pipe-not-found, pipes.tag-resource-invalid-key-validation-rejected, pipes.tag-resource-on-non-pipe-arn-rejected, pipes.tag-resource-with-aws-reserved-prefix-rejected, pipes.untag-resource-nonexistent-pipe-not-found, pipes.list-tags-for-resource-nonexistent-pipe-not-found, pipes.update-pipe-source-arn-immutable-rejected, pipes.update-pipe-source-type-mismatch-rejected, pipes.update-pipe-wrong-source-type-parameters-rejected.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 12/107 passed (11.2%); pipes 12/94 with 82 failures. The emulator was serving (CreatePipe/DescribePipe/DeletePipe answered 200; no bootstrap signal).

Cause 1:
  pipes.create-pipe-batch-size-above-sqs-target-cap-rejected — AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Name': 'sim2real-pipes-bsz-bad-...', 'DesiredState': 'STOPPED', 'CurrentState': 'CREATING', ...} for SourceParameters={SqsQueueParameters:{BatchSize:50,MaximumBatchingWindowInSeconds:5}} with an SQS target.
    Expected: HTTP 400 ValidationException, message (example) '{"ValidationErrorFields":[{"Name":"BatchSize","Message":"The configured batch size 50 is greater than the max supported batch size 10 for target sqs."}]}'. The same wiring with BatchSize 10 is accepted and reads back.
  pipes.pipe-filter-blocks-nonmatching-events — AssertionError: CreatePipe accepted a malformed (non-JSON) FilterCriteria pattern. Expected HTTP 400 ValidationException.

Cause 2 (status 400 / code ValidationException are correct; the body diverges):
  pipes.create-pipe-malformed-source-arn-rejected — expected '[ECMA 262 regex "<ArnOrUrl pattern>" does not match input string "<value>"]', got "1 validation error detected: Value 'https://127.0.0.1:<port>/000000000000/<queue>' at 'source' failed to satisfy constraint: Member must satisfy regular expression pattern: ^smk://..."
  pipes.create-pipe-name-outside-the-allowed-characters-rejected — expected '{"ValidationErrorFields":[{"Name":"PipeName","Message":"Invalid pattern for pipe name. Pipe name should match pattern ^[\\.\\-_A-Za-z0-9]+$."}]}', got "1 validation error detected: Value '<name>@x' at 'name' failed to satisfy constraint: ..."
  pipes.description-max-length-exceeded-rejected — expected '[string "xxx..." is too long (length: 513, maximum allowed: 512)]', got "1 validation error detected: Value 'xxx... [513 bytes]' at 'description' failed to satisfy constraint: Member must have length less than or equal to 512"
  pipes.tag-resource-exceeding-max-tags-rejected — unexpected message "1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50"
  pipes.untag-resource-malformed-empty-key-rejected — unexpected message "1 validation error detected: Value '' at 'tagKeys.1.member' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 3 (404/NotFoundException correct, message template wrong) — five checks report the same shape:
  expected message 'Pipe <name> does not exist.', got 'Pipe not found: <name>'  (pipes.start-nonexistent-pipe-not-found, pipes.start-pipe-nonexistent-not-found, pipes.stop-nonexistent-pipe-not-found, pipes.stop-pipe-nonexistent-not-found, pipes.delete-running-pipe-auto-stops on DeletePipe of an already-deleted pipe, pipes.filter-criteria-nested-json-preserved on DescribePipe after DeletePipe).

Cause 4 (409/ConflictException correct, message wrong):
  pipes.create-pipe-duplicate-name-rejected — expected message 'Pipe <name> already exists.', got 'Pipe named <name> already exists.'
  pipes.pipes-create-pipe-duplicate-name-rejected-without-overwriting — same.
  Code: services/service-com-amazonaws-pipes-pipes/src/ops/create_pipe.rs:225.

Cause 5:
  pipes.empty-vs-absent-description-distinguished — AssertionError: explicit empty-string description should be normalized to absent (real AWS behavior), got ''
  pipes.pipe-description-empty-normalized-to-absent — AssertionError: expected Description="" to normalize to absent, got ''
  Positive control in the same checks: a non-empty Description must still round-trip.

Cause 6 (expected modeled 404/400, got a non-modeled error code):
  pipes.tag-resource-nonexistent-pipe-not-found — AssertionError: expected NotFoundException tagging a non-existent pipe, got 'NotImplemented'
  pipes.list-tags-for-resource-nonexistent-pipe-not-found — AssertionError: expected NotFoundException for a never-created pipe ARN, got 'NotImplemented'
  pipes.untag-resource-nonexistent-pipe-not-found — AssertionError: expected NotFoundException untagging a ghost pipe ARN, got NotImplemented
  pipes.tag-resource-on-non-pipe-arn-rejected — AssertionError: expected BadRequestException, got NotImplemented
  pipes.tag-resource-invalid-key-validation-rejected / pipes.tag-resource-with-aws-reserved-prefix-rejected — AssertionError: code='NotImplemented'
  pipes.update-pipe-source-arn-immutable-rejected — AssertionError: expected a rejection, got NotImplemented
  pipes.update-pipe-source-type-mismatch-rejected — AssertionError: expected ValidationException for a source-type mismatch, got NotImplemented
  pipes.update-pipe-wrong-source-type-parameters-rejected — AssertionError: expected ValidationException when SourceParameters don't match the SQS source type, got NotImplemented

Generated pipe names, queue URLs, hosts, ports and request ids are examples only; the requirements are the status codes, the error codes, the message-building rules named above, and the normalization of an empty Description.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Cause 1: with two standard SQS queues and a pipes role, call CreatePipe(..., SourceParameters={SqsQueueParameters:{BatchSize:50,MaximumBatchingWindowInSeconds:5}}, Target=<sqs queue arn>) and observe HTTP 200 instead of 400 ValidationException; repeat with FilterCriteria={Filters:[{Pattern:'not-json'}]} and observe acceptance.
3. Cause 2: call CreatePipe with Source set to a queue URL (not an ARN), then with Name='<valid>@x', then with a 513-character Description; call TagResource with 51 tags and UntagResource with an empty tag key. Each returns 400 ValidationException with the generic '<N> validation error detected: Value ... at '<member>' failed to satisfy constraint ...' message instead of the Pipes-style body.
4. Cause 3: call StartPipe / StopPipe / DeletePipe / DescribePipe with a name that was never created. Observe 404 NotFoundException with message 'Pipe not found: <name>' instead of 'Pipe <name> does not exist.'
5. Cause 4: call CreatePipe twice with the same Name. The second returns 409 ConflictException with 'Pipe named <name> already exists.' instead of 'Pipe <name> already exists.'
6. Cause 5: call CreatePipe with Description="", then DescribePipe. Observe Description: '' in the response instead of the member being absent (and confirm a non-empty Description still round-trips).
7. Cause 6: call TagResource / UntagResource / ListTagsForResource against a ghost pipe ARN, a non-pipe ARN, an aws:-prefixed tag key and an invalid tag key, and call UpdatePipe changing Source or passing SourceParameters of the wrong source type. Each answers the non-modeled NotImplemented instead of the modeled 404 NotFoundException / 400 ValidationException.
8. After fixing, re-replay the full trace_ids list of this issue.
