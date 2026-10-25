Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dataexchange.finalize-revision-without-assets-rejected dataexchange.event-action-notfound-error-semantics dataexchange.event-action-requires-entitled-dataset-rejected
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange.finalize-revision-without-assets-rejected|run#5|AssertionError|assert code == "ValidationException", code
Issue-Key: dataexchange.event-action-notfound-error-semantics|run#4|AssertionError|assert code_get == "ResourceNotFoundException", (
Issue-Key: dataexchange.event-action-requires-entitled-dataset-rejected|run#7|AssertionError|assert code_owned == "ValidationException", (
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# DataExchange UpdateRevision and the EventAction operations answer 501 NotImplemented instead of their modeled success/refusal behavior

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRevision

## Description
Two distinct root causes, each a group of DataExchange operations still answering the scaffold's HTTP 501 `NotImplemented` where behavioral checks reach them through a caught ClientError (so they surface as response mismatches, not as the suite's auto-filed stub clusters for CreateJob / GetRevision / ListTagsForResource). This issue bundles 2 causes; both must be closed.

=== Cause 1 of 2: UpdateRevision is unimplemented, so revision finalization has no semantics ===
AWS: Smithy model (services/service-com-amazonaws-dataexchange-dataexchange/smithy-model/com-amazonaws-dataexchange-dataexchange.json) — UpdateRevision, PATCH /v1/data-sets/{DataSetId}/revisions/{RevisionId}, success 200, output carries Finalized/Comment/UpdatedAt; modeled errors include ValidationException (400), ConflictException (409), ResourceNotFoundException (404). Recorded behavior: UpdateRevision(Finalized=True) on a revision with zero assets is refused with ValidationException (HTTP 400), recorded message example "Cannot set Finalized to true for a Revision which has 0 Assets.", and the revision keeps Finalized=false; the same call on a revision that has at least one imported asset succeeds with 200 and Finalized=true. UpdateRevision on a deleted/never-created RevisionId is ResourceNotFoundException (404), recorded message example "The requested resource was not found."
Emulator: the handler is the generated scaffold — services/service-com-amazonaws-dataexchange-dataexchange/src/ops/update_revision.rs line 192 returns `Scaffolded::not_implemented("UpdateRevision")`, so every call answers HTTP 501 with error code `NotImplemented` ("operation UpdateRevision is scaffolded but not implemented in this emulator").
Why: a producer client can never finalize a revision, and gets an unmodeled 501/NotImplemented instead of either 200 or the 400/404 refusals it retries or reports on; the zero-asset guard that protects publishing is absent, so no caller can distinguish "cannot finalize an empty revision" from "revision gone".
Fix: implement the UpdateRevision handler (comment/finalized update against revision state, the zero-asset ValidationException guard, and the not-found path) in services/service-com-amazonaws-dataexchange-dataexchange/src/ops/update_revision.rs, and remove the SCAFFOLDED marker.
Traces: dataexchange.finalize-revision-without-assets-rejected.

=== Cause 2 of 2: the EventAction operations (CreateEventAction, GetEventAction, UpdateEventAction, DeleteEventAction) are unimplemented ===
AWS: Smithy model — GetEventAction GET /v1/event-actions/{EventActionId} (200), UpdateEventAction PATCH /v1/event-actions/{EventActionId} (200) and DeleteEventAction DELETE /v1/event-actions/{EventActionId} (204) all model ResourceNotFoundException (404); CreateEventAction POST /v1/event-actions (201) models ValidationException (400). Recorded behavior: Get/Update/DeleteEventAction on a well-formed but never-created EventActionId each return ResourceNotFoundException (404), recorded message example "The requested resource was not found."; CreateEventAction with a RevisionPublished event naming a data set whose Origin is OWNED is refused with ValidationException (400), recorded message example "Event actions are only compatible with Data Sets where the value for Origin is ENTITLED.", and one naming a never-created DataSetId is refused with ResourceNotFoundException (404). A rejected create must leave no event action behind (ListEventActions(EventSourceId=...) stays empty).
Emulator: these handlers are generated scaffolds — e.g. services/service-com-amazonaws-dataexchange-dataexchange/src/ops/get_event_action.rs line 164 returns `Scaffolded::not_implemented("GetEventAction")` — so each call answers HTTP 501 with error code `NotImplemented` instead of the modeled 404/400.
Why: a client that provisions export-on-publish automation sees an unmodeled 501/NotImplemented on every event-action call; not-found and the ENTITLED-only constraint are indistinguishable from "service unavailable", and error handling that branches on ResourceNotFoundException vs ValidationException never fires.
Fix: implement the four event-action handlers (create_event_action.rs, get_event_action.rs, update_event_action.rs, delete_event_action.rs under services/service-com-amazonaws-dataexchange-dataexchange/src/ops/), including the Origin==ENTITLED requirement for a RevisionPublished event and the unknown-data-set 404, backed by event-action state, and remove their SCAFFOLDED markers.
Traces: dataexchange.event-action-notfound-error-semantics, dataexchange.event-action-requires-entitled-dataset-rejected.

## Evidence
Cause 1: dataexchange.finalize-revision-without-assets-rejected — AssertionError at `assert code == "ValidationException"`: got 'NotImplemented' from dx.update_revision(DataSetId=..., RevisionId=<empty revision>, Finalized=True). Expected HTTP 400 ValidationException (message example "Cannot set Finalized to true for a Revision which has 0 Assets.") with Finalized still false; got HTTP 501 NotImplemented. Scaffold confirmed at src/ops/update_revision.rs:4 (SCAFFOLDED marker) and :192.

Cause 2: dataexchange.event-action-notfound-error-semantics — AssertionError: GetEventAction on a never-created id: expected ResourceNotFoundException, got 'NotImplemented' (HTTP 501); same stub path for UpdateEventAction/DeleteEventAction. dataexchange.event-action-requires-entitled-dataset-rejected — AssertionError: expected ValidationException for a RevisionPublished event action on an OWNED (non-ENTITLED) data set, got 'NotImplemented'. Scaffold confirmed at src/ops/get_event_action.rs:4 and :164.

## Steps to Reproduce
Cause 1: CreateDataSet(AssetType=S3_SNAPSHOT, Name=...) -> CreateRevision(DataSetId=<id>) -> UpdateRevision(DataSetId=<id>, RevisionId=<rev>, Finalized=True). Expected 400 ValidationException (zero-asset guard); observed 501 NotImplemented.

Cause 2: (a) GetEventAction(EventActionId="b"*38), UpdateEventAction(EventActionId="b"*38, Action={"ExportRevisionToS3": {"RevisionDestination": {"Bucket": "any-bucket"}}}), DeleteEventAction(EventActionId="b"*38) — expected 404 ResourceNotFoundException each; observed 501 NotImplemented. (b) CreateDataSet(AssetType=S3_SNAPSHOT) then CreateEventAction(Event={"RevisionPublished": {"DataSetId": <owned id>}}, Action={"ExportRevisionToS3": {"RevisionDestination": {"Bucket": <bucket>}}}) — expected 400 ValidationException (ENTITLED-only), and with DataSetId="a"*38 expected 404 ResourceNotFoundException; observed 501 NotImplemented, and ListEventActions(EventSourceId=...) must remain empty.
