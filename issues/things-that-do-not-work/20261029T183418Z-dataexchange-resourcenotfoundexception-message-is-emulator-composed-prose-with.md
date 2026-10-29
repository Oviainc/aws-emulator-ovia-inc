Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dataexchange.dataset-lifecycle-create-get-delete dataexchange.get-job-unknown-id-not-found dataexchange.event-action-notfound-error-semantics dataexchange.event-action-requires-entitled-dataset-rejected dataexchange.send-data-set-notification-error-semantics
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange.dataset-lifecycle-create-get-delete|run#17|AssertionError|assert get_message == "The requested resource was not found.", f"expected the exact refusal message, got {get_message!r}"
Issue-Key: dataexchange.get-job-unknown-id-not-found|run#12|AssertionError|assert message == "The requested resource was not found.", f"expected the exact refusal message, got {message!r}"
Issue-Key: dataexchange.event-action-notfound-error-semantics|run#5|AssertionError|assert code_get_message == "The requested resource was not found.", f"expected the exact refusal message, got {code_get_message!r}"
Issue-Key: dataexchange.event-action-requires-entitled-dataset-rejected|run#12|AssertionError|assert message_missing == "The requested resource was not found.", message_missing
Issue-Key: dataexchange.send-data-set-notification-error-semantics|run#10|AssertionError|assert code_missing_message == "The requested resource was not found.", (
Issue-Commit: b2028dd6393016a9896c0e35ff905bdd884f0327

# DataExchange ResourceNotFoundException Message is emulator-composed prose with the requested id interpolated instead of AWS's fixed "The requested resource was not found."

> _Note: this issue was identified against the code base at commit `b2028dd6393016a9896c0e35ff905bdd884f0327`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataSet

## Description
Five causes, one shared rule. AWS's ResourceNotFoundException Message is FIXED text with no variable part — 'The requested resource was not found.' — and the identity of the missing thing travels in the exception's own ResourceId and ResourceType members, not in the sentence (Smithy: ResourceNotFoundException { Message: __string required, ResourceId: __string "The unique identifier for the resource that couldn't be found.", ResourceType: ResourceType } @error client @httpError 404). In every case below the emulator returns the correct 404 and the correct ResourceNotFoundException code, and diverges only in Message, which it composes as "The <resource kind> <id> couldn't be found." — a sentence AWS never emits, with a request value interpolated into it. The causes are separated by operation because each is a distinct lookup site that must be made to raise the shared error.

Cause 1 of 5: GetDataSet on a data set that was deleted (or never created). AWS: 404 ResourceNotFoundException, Message 'The requested resource was not found.' (Smithy: GetDataSet GET /v1/data-sets/{DataSetId}, errors include ResourceNotFoundException @httpError 404). Emulator: 404 ResourceNotFoundException but Message "The data set <DataSetId> couldn't be found." Fix: the dataexchange crate's shared not-found error constructor, reached from GetDataSet's lookup. Traces: dataexchange.dataset-lifecycle-create-get-delete.

Cause 2 of 5: GetJob, UpdateEventAction and DeleteEventAction... specifically GetJob on a well-formed but never-created JobId. AWS: 404 ResourceNotFoundException with the same fixed Message (Smithy: GetJob GET /v1/jobs/{JobId}, errors include ResourceNotFoundException @httpError 404). Emulator: Message "The job <JobId> couldn't be found." Fix: same constructor, job lookup path. Traces: dataexchange.get-job-unknown-id-not-found.

Cause 3 of 5: GetEventAction, UpdateEventAction and DeleteEventAction on a well-formed but never-created EventActionId (the check exercises all three in turn and stops at the first). AWS: 404 ResourceNotFoundException with the same fixed Message for each (Smithy: GetEventAction GET /v1/event-actions/{EventActionId}, errors include ResourceNotFoundException @httpError 404; Update/DeleteEventAction likewise). Emulator: Message "The event action <EventActionId> couldn't be found." Fix: same constructor, event-action lookup path in all three handlers. Traces: dataexchange.event-action-notfound-error-semantics.

Cause 4 of 5: CreateEventAction whose Event.RevisionPublished.DataSetId is well-formed but never created. Recorded behavior only for the classification — the model does not list ResourceNotFoundException among CreateEventAction's errors (Smithy: AccessDeniedException 403, InternalServerException 500, ServiceLimitExceededException 402, ThrottlingException 429, ValidationException 400) yet AWS answers 404 ResourceNotFoundException here, telling 'does not exist' apart from the ValidationException it raises for an existing-but-not-ENTITLED data set. The emulator already gets that split right (its ValidationException 'Event actions are only compatible with Data Sets where the value for Origin is ENTITLED.' matches) and diverges only in the not-found Message: "The data set <DataSetId> couldn't be found." instead of 'The requested resource was not found.' Fix: same constructor, CreateEventAction's event-source data-set lookup. Traces: dataexchange.event-action-requires-entitled-dataset-rejected.

Cause 5 of 5: SendDataSetNotification on a well-formed but never-created DataSetId. AWS: 404 ResourceNotFoundException with the same fixed Message (Smithy: SendDataSetNotification POST /v1/data-sets/{DataSetId}/notification code 202, errors include ResourceNotFoundException @httpError 404), kept distinct from the ValidationException 'Data set not configured for AWS Marketplace Data Product.' it raises for a data set that exists but was never published — a split the emulator also already gets right. Emulator: Message "The data set <DataSetId> couldn't be found." Fix: same constructor, SendDataSetNotification's data-set lookup. Traces: dataexchange.send-data-set-notification-error-semantics.

Related open issue: 20261029T172356Z-dataexchange-client-error-messages-are-emulator-prose-instead.md describes the same wording rule from the WIRE corpus and names GetDataSet, GetEventAction, UpdateEventAction, DeleteEventAction, GetJob and CreateJob. It carries wire replay keys only, so none of the five behavioral checks above are bound to it; causes 4 and 5 (CreateEventAction, SendDataSetNotification) are lookup sites it does not name at all. Fixing the shared constructor should clear that issue and this one together, but this issue is what the behavioral checks listed in trace_ids re-replay against.

## Evidence
All five: expected HTTP 404 / actual HTTP 404, expected code ResourceNotFoundException / actual ResourceNotFoundException. Only $.Message diverges. Required text is wholly fixed: 'The requested resource was not found.' — no part of it is taken from the request or from state. The emulator's template is 'The ' + <resource kind word> + ' ' + <the id from the request> + " couldn't be found."; the ids quoted below are one run's request values and are not part of any required text.
Cause 1 — GetDataSet after DeleteDataSet: got "The data set 9daff2d24a0f8cfd4e37c83d34bc25b1 couldn't be found." (AssertionError: expected the exact refusal message).
Cause 2 — GetJob(JobId='z'*32): got "The job zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz couldn't be found."
Cause 3 — GetEventAction(EventActionId='b'*38): got "The event action bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb couldn't be found." (the check's UpdateEventAction and DeleteEventAction assertions on the same id are not reached because the first one fails).
Cause 4 — CreateEventAction with Event.RevisionPublished.DataSetId='a'*38: got "The data set aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa couldn't be found."
Cause 5 — SendDataSetNotification(DataSetId='c'*38, Type=DATA_UPDATE): got "The data set cccccccccccccccccccccccccccccccccccccc couldn't be found."
Smithy: ResourceNotFoundException @httpError 404, members Message (required), ResourceId, ResourceType — the members the resource identity belongs in.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Cause 1: CreateDataSet, DeleteDataSet, then GetDataSet on the same DataSetId (dataexchange.dataset-lifecycle-create-get-delete). 404 ResourceNotFoundException is returned, but Message is the interpolated sentence.
3. Cause 2: GetJob with a syntactically valid, never-created JobId (dataexchange.get-job-unknown-id-not-found).
4. Cause 3: GetEventAction — then UpdateEventAction and DeleteEventAction — with a syntactically valid, never-created EventActionId (dataexchange.event-action-notfound-error-semantics).
5. Cause 4: CreateEventAction with Event={RevisionPublished:{DataSetId: a never-created 38-char id}} and Action={ExportRevisionToS3:{RevisionDestination:{Bucket: any}}} (dataexchange.event-action-requires-entitled-dataset-rejected).
6. Cause 5: SendDataSetNotification on a never-created 38-char DataSetId with Type=DATA_UPDATE and Details={DataUpdate:{DataUpdatedAt: now}} (dataexchange.send-data-set-notification-error-semantics).
7. In each case assert the error body's Message equals exactly 'The requested resource was not found.' and that the missing resource's identity is carried in ResourceId/ResourceType rather than in the sentence.
