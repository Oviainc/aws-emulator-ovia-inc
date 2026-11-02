Issue-Source: trace-sweep-divergence
Issue-Trace: deadline-queue-fleet-association-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: deadline-queue-fleet-association-lifecycle
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline|GetQueueFleetAssociation|2xx->2xx|$.createdAt
Issue-Key: deadline|GetQueueFleetAssociation|4xx->4xx|$.message
Issue-Key: deadline|GetQueueFleetAssociation|4xx->4xx|$.resourceType
Issue-Key: deadline|GetQueueFleetAssociation|2xx->2xx|$
Issue-Key: deadline|GetQueueFleetAssociation|2xx->2xx|$.status
Issue-Key: deadline|GetQueueFleetAssociation|2xx->2xx|$.updatedAt
Issue-Key: deadline|GetQueueFleetAssociation|2xx->2xx|$.updatedBy
Issue-Key: deadline|GetQueueFleetAssociation|4xx->2xx|$.message
Issue-Key: deadline|GetQueueFleetAssociation|4xx->2xx|$.resourceId
Issue-Key: deadline|GetQueueFleetAssociation|4xx->2xx|$.resourceType
Issue-Key: deadline|GetQueueFleetAssociation|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: deadline|GetQueueFleetAssociation|4xx->2xx|status
Issue-Key: deadline|DeleteFleet|2xx->4xx|status
Issue-Commit: 855e46d3b98757a511fcacb47a7c9e35083e2051

# deadline queue-fleet-association lifecycle diverges on five counts: epoch-seconds createdAt, reworded/mis-typed 404, status never leaves ACTIVE, deleted association still readable, and DeleteFleet then 409s

> _Note: this issue was identified against the code base at commit `855e46d3b98757a511fcacb47a7c9e35083e2051`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueueFleetAssociation

## Description
FIVE independent defects on the deadline queue-fleet-association path, all observed replaying the queue-fleet-association lifecycle fixture (deadline-queue-fleet-association-lifecycle). A fixer must close ALL FIVE before submitting.

Cause 1 of 5 — GetQueueFleetAssociation serializes createdAt as an epoch-seconds NUMBER instead of a date-time string (fixture deadline-queue-fleet-association-lifecycle, step task#8).
AWS: Smithy: GetQueueFleetAssociation is GET /2023-10-12/farms/{farmId}/queue-fleet-associations/{queueId}/{fleetId} (success 200); output member createdAt targets CreatedAt, @required, type timestamp, timestampFormat "date-time" (updatedAt targets UpdatedAt with the same timestampFormat). The service speaks restJson1, so a date-time timestamp goes on the wire as an ISO-8601/RFC-3339 STRING, never a JSON number. The recording asserts that member by SHAPE, not by value (per-run instants are not a requirement).
Emulator: the 200 body carries $.createdAt as a bare JSON number of epoch seconds with a fractional part. Every other asserted member of that 200 matches.
Why: SDKs deserialize this member per the model, so a number where a date-time string is modeled either fails to parse or is silently reinterpreted; any client doing date math, display or ordering on createdAt/updatedAt gets a wrong or absent instant.
Fix: the operation's response serialization of timestamp members — the generated output schema / serializer timestampFormat wiring, not the handler, which already builds a correctly typed value. Hypothesis: the serializer falls back to restJson1's protocol-default epoch-seconds encoding because @timestampFormat("date-time") on CreatedAt/UpdatedAt is not honored at serialization time. This is the SAME serializer defect already filed for GetStorageProfile (20261030T195531Z-...) and GetLimit (20261031T084329Z-...); neither carries a replay key for this operation or this fixture, so it is filed here for GetQueueFleetAssociation.
Traces: deadline-queue-fleet-association-lifecycle#task#8.

Cause 2 of 5 — the 404 for a queue-fleet association whose fleet does not exist uses the emulator's own wording and reports the ASSOCIATION as the missing resource instead of the FLEET (fixture deadline-queue-fleet-association-lifecycle, step task#9).
AWS: Smithy: GetQueueFleetAssociation models ResourceNotFoundException (httpError 404) whose required members are message, resourceId and resourceType; the model documents resourceType only as "The type of the resource that couldn't be found" and enumerates no values. Recorded behavior: a GET naming an existing farm and queue but a syntactically valid (^fleet-[0-9a-f]{32}$) fleetId that no fleet carries answers HTTP 404 ResourceNotFoundException with resourceType 'fleet' and message 'Resource of type fleet with id <fleetId> does not exist.' — i.e. the resource reported missing is the FLEET named in the path, not the association. Fixed text: 'Resource of type <resourceType> with id <resourceId> does not exist.' including the trailing period; the variable parts are the resource kind ('fleet') and the fleetId taken verbatim from the request. The recorded resourceId (the fleetId) already matched and is not part of this finding.
Emulator: same status (404) and error code, but message "The queueFleetAssociation <fleetId> can't be found." and resourceType 'queueFleetAssociation' — the template "The {resource_type} {resource_id} can't be found." rendered from a camelCase token, and the wrong resource kind chosen for this case.
Why: the message and the resourceType discriminator are part of the contract. A client that matches AWS's sentence, or switches on resourceType to decide whether to re-create the FLEET versus re-create the ASSOCIATION, takes the wrong branch or falls through to a generic error, so not-found handling that works against AWS breaks here.
Fix: the service's single-homed refusal builder (crate::state::not_found — change the template, not its call sites) plus the not-found branch of this operation's handler, which must report the fleet (RESOURCE_TYPE_FLEET, fleetId) when the farm's fleet does not exist and reserve the association kind for an existing-fleet/queue pair that simply is not associated (see Cause 4). Related but NOT covering this: open issues 20261029T172830Z-deadline-resourcenotfoundexception-message-is-the-emulator-s-own (farm, from the behavioral suite) and 20261031T070507Z-deadline-fleet-resourcenotfoundexception-message-is-the-emulator-s carry the wording defect for other operations and no replay key for this fixture step, and say nothing about this operation choosing the wrong resource kind.
Traces: deadline-queue-fleet-association-lifecycle#task#9.

Cause 3 of 5 — an association never leaves ACTIVE after UpdateQueueFleetAssociation, and updatedAt/updatedBy are never set, so a poll for STOPPED never converges (fixture deadline-queue-fleet-association-lifecycle, step task#11).
AWS: Smithy: UpdateQueueFleetAssociation is PATCH /2023-10-12/farms/{farmId}/queue-fleet-associations/{queueId}/{fleetId} (idempotent, success 200, empty output) with a @required status of UpdateQueueFleetAssociationStatus; QueueFleetAssociationStatus is ACTIVE | STOPPED | STOP_SCHEDULING_AND_CANCEL_TASKS | STOP_SCHEDULING_AND_COMPLETE_TASKS. Recorded behavior: after the update asks the association to stop scheduling, GetQueueFleetAssociation reaches status 'STOPPED' and reports updatedAt (date-time) and updatedBy as set, since the association has now been modified by a caller.
Emulator: the read keeps answering 200 with status 'ACTIVE', $.updatedBy and $.updatedAt absent, for 13 poll attempts until the fixture's 60s poll cap expires. The update left the stored association untouched.
Why: the stop/drain workflow is the only way to take a fleet out of a queue's scheduling, and it is also the prerequisite for tearing the pair down. A client that issues the update and polls for STOPPED (as the console and the AWS CLI examples do) hangs forever, and anything keyed on updatedAt/updatedBy to detect that the change landed never sees it.
Fix: UpdateQueueFleetAssociation's handler and the association's state transition — the stored QueueFleetAssociation's status must move to the requested status and its updated_at/updated_by must be stamped, so the existing read path reports them. Note UpdateQueueFleetAssociation is still an unimplemented scaffold stub in this checkout (src/ops/update_queue_fleet_association.rs returns Scaffolded::not_implemented), so its own 501 is a blocked step owned elsewhere; standing the operation up must also make this subsequent READ report STOPPED with updatedAt/updatedBy, which is the divergence recorded here.
Traces: deadline-queue-fleet-association-lifecycle#task#11.

Cause 4 of 5 — a queue-fleet association remains readable after it has been deleted: 200 where AWS answers 404 (fixture deadline-queue-fleet-association-lifecycle, step task#13).
AWS: Smithy: DeleteQueueFleetAssociation is DELETE /2023-10-12/farms/{farmId}/queue-fleet-associations/{queueId}/{fleetId} (idempotent, success 200, empty output); once it succeeds the association no longer exists, so GetQueueFleetAssociation on that triple answers HTTP 404 ResourceNotFoundException — message 'Resource of type queue-fleet-association with id <farmId>, <queueId> and <fleetId> does not exist.', resourceType 'queue-fleet-association', resourceId the composite '<farmId>, <queueId> and <fleetId>' — plus the restJson1 error header x-amzn-errortype naming the exception. Fixed text: the same 'Resource of type <resourceType> with id <resourceId> does not exist.' sentence, with the kebab-case kind matching the URI segment /queue-fleet-associations/ and the id being the three path ids joined as '<farmId>, <queueId> and <fleetId>' (comma between the first two, the word "and" before the last), all taken from the request.
Emulator: the same read answers HTTP 200 with the association body still intact — no message, no resourceType, no resourceId, no x-amzn-errortype header. The delete left the association in state.
Why: a client cannot tell a deleted association from a live one. Deletion appears to silently fail, so teardown loops that poll for 404 never converge, re-association and uniqueness checks see a phantom association, and the fleet stays pinned to the queue (see Cause 5).
Fix: the delete path's state mutation — the association must be removed from its queue's fleet_associations map so the read path's existing not-found branch fires — and that branch's composite resourceType/resourceId/message for an existing fleet and queue that are no longer associated. Note DeleteQueueFleetAssociation is still an unimplemented scaffold stub here (src/ops/delete_queue_fleet_association.rs returns Scaffolded::not_implemented) and its 501 is a blocked step owned elsewhere; standing it up must also make this read answer the modeled 404.
Traces: deadline-queue-fleet-association-lifecycle#task#13.

Cause 5 of 5 — DeleteFleet refuses with 409 ConflictException where AWS answers 200, because the association the teardown already deleted is still held in state (fixture deadline-queue-fleet-association-lifecycle, step teardown#14).
AWS: Smithy: DeleteFleet is DELETE /2023-10-12/farms/{farmId}/fleets/{fleetId} (idempotent, success 200, empty output); it models ConflictException (409) for a fleet that is still in use, but a fleet whose queue-fleet association has already been deleted is NOT in conflict, so the recording shows HTTP 200 with an empty body for the teardown delete that follows the association delete.
Emulator: the same delete answers HTTP 409 ConflictException. The conflict guard reads the store for a queue still associated with the fleet and finds the association that the earlier DeleteQueueFleetAssociation never removed, so it refuses a deletion AWS performs.
Why: the terminal cleanup of the resource tree fails. Every fixture, test or teardown script that deletes an association and then its fleet aborts on a 409 that AWS never returns, leaving fleets and farms undeletable and the account's resource tree unbounded; retrying never clears it, because nothing can ever remove the association.
Fix: the association-delete path's state mutation (the same removal Cause 4 needs) — the DeleteFleet conflict guard itself is correct and must stay. Hypothesis: with the association removed, this delete needs no change at all; it is listed as its own cause because it is a separate operation answering a separate wrong status, and a fixer must re-replay this step to confirm it.
Traces: deadline-queue-fleet-association-lifecycle#teardown#14.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, steps_total=71, passed=35, score 49.3%). Service token 'deadline' is bootstrapped.

Cause 1 — fixture deadline-queue-fleet-association-lifecycle step task#8, GetQueueFleetAssociation, expected 200 / actual 200:
  "$.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1793612789.6206913)"
The expected value is asserted by shape (a date-time string); the emulator emitted a JSON number of epoch seconds. That numeral is one run's instant and is not a requirement. Model evidence: smithy_query(operation="GetQueueFleetAssociation") -> output createdAt {target CreatedAt, required, type timestamp, timestampFormat "date-time"}, updatedAt {target UpdatedAt, timestampFormat "date-time"}. Code: services/service-com-amazonaws-deadline-deadline/src/ops/get_queue_fleet_association.rs:197-199 sets created_at: crate::state::timestamp(association.created_at) / updated_at: ...map(crate::state::timestamp), and src/state.rs:915-917 returns an aws_smithy_types::DateTime — so the date-time intent is lost downstream of the handler, in serialization.

Cause 2 — fixture deadline-queue-fleet-association-lifecycle step task#9, GetQueueFleetAssociation, expected 404 / actual 404:
  "$.message: value_mismatch (expected 'Resource of type fleet with id fleet-089d958986b0714a520d2989b867a114 does not exist.', got \"The queueFleetAssociation fleet-089d958986b0714a520d2989b867a114 can't be found.\")"
  "$.resourceType: value_mismatch (expected 'fleet', got 'queueFleetAssociation')"
The fleet- id shown is that run's value and is not a requirement; the requirements are the sentence template and the resourceType token 'fleet' for this case. Code: src/state.rs:920-930 — not_found() formats message: format!("The {resource_type} {resource_id} can't be found."); src/state.rs:147-152 — RESOURCE_TYPE_QUEUE_FLEET_ASSOCIATION: &str = "queueFleetAssociation" (its comment derives the token from the shape name rather than from the /queue-fleet-associations/ path segment); call site src/ops/get_queue_fleet_association.rs:181-191, whose comment deliberately reports the association rather than the fleet for an unassociated/absent fleetId — the recording shows AWS reports the fleet. Model evidence: smithy_query(shape="ResourceNotFoundException") -> httpError 404, required members message/resourceId/resourceType.

Cause 3 — fixture deadline-queue-fleet-association-lifecycle step task#11, GetQueueFleetAssociation, expected 200 / actual 200:
  "$: poll capped at 60s (expected {... 'status': 'STOPPED', 'createdAt': '<shaped-not-asserted>', 'createdBy': '<shaped-not-asserted>', 'updatedBy': '<shaped-not-asserted>', 'updatedAt': '<shaped-not-asserted>'}, got {... 'status': 'ACTIVE', 'createdAt': 1793612789.6206913, 'createdBy': 'arn:aws:iam::...:user/EMULATOR...'})"
  "$.status: value_mismatch (expected 'STOPPED', got 'ACTIVE')"
  "$.updatedBy: missing (expected '<shaped-not-asserted>', got None)"
  "$.updatedAt: missing (expected '<shaped-not-asserted>', got None)"
  "$.status: poll: not in ['STOPPED'] after 13 attempt(s) (expected 'STOPPED', got 'ACTIVE')"
The queueId/fleetId/createdBy values are that run's and are not requirements. Model evidence: smithy_query(operation="UpdateQueueFleetAssociation") -> PATCH /2023-10-12/farms/{farmId}/queue-fleet-associations/{queueId}/{fleetId}, idempotent, success 200, required input status of UpdateQueueFleetAssociationStatus. Code: src/state.rs:114-121 — QUEUE_FLEET_ASSOCIATION_STATUS_ACTIVE is the created status and its comment states only UpdateQueueFleetAssociation moves it; src/ops/update_queue_fleet_association.rs:143-161 is an unimplemented stub returning Scaffolded::not_implemented("UpdateQueueFleetAssociation"), so nothing ever moves it.

Cause 4 — fixture deadline-queue-fleet-association-lifecycle step task#13, GetQueueFleetAssociation, expected 404 / actual 200:
  "status: expected 404, got 200"
  "$.message: missing (expected 'Resource of type queue-fleet-association with id farm-b0fd791bfc89942940de018235415258, queue-7cb35cdc7b37cbbcc92c099f2be83cbb and fleet-e7e65e6fd984b422fbe507a9a8defefd does not exist.', got None)"
  "$.resourceType: missing (expected 'queue-fleet-association', got None)"
  "$.resourceId: missing (expected 'farm-b0fd791bfc89942940de018235415258, queue-7cb35cdc7b37cbbcc92c099f2be83cbb and fleet-e7e65e6fd984b422fbe507a9a8defefd', got None)"
  "$headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)"
The three ids are that run's values; the requirement is the kebab-case kind 'queue-fleet-association' and the composite id '<farmId>, <queueId> and <fleetId>' built from the request. Model evidence: smithy_query(operation="DeleteQueueFleetAssociation") -> DELETE /2023-10-12/farms/{farmId}/queue-fleet-associations/{queueId}/{fleetId}, idempotent, success 200, empty output; GetQueueFleetAssociation models ResourceNotFoundException (404). Code: the read's not-found branch exists (src/ops/get_queue_fleet_association.rs:181-191) and fires only when the association is absent from the queue's fleet_associations map (src/state.rs:878-886, store.queue_fleet_association), so the gap is the removal on delete; src/ops/delete_queue_fleet_association.rs:147-165 is an unimplemented stub.

Cause 5 — fixture deadline-queue-fleet-association-lifecycle step teardown#14, DeleteFleet, expected 200 / actual 409:
  "status: expected 200, got 409"
Model evidence: smithy_query(operation="DeleteFleet") -> DELETE /2023-10-12/farms/{farmId}/fleets/{fleetId}, idempotent, success 200, empty output, ConflictException httpError 409. Code: src/ops/delete_fleet.rs:173-185 refuses with crate::state::fleet_associated_with_queue(..) whenever store.queue_associated_with_fleet(farmId, fleetId) finds a queue (src/state.rs:888-897); the 409 body's message template is src/state.rs:983-991 ("The fleet {fleet_id} can't be deleted because it is associated with the queue {queue_id}."). The guard is right; the association it finds is one the earlier delete should have removed.

## Steps to Reproduce
Replay fixture deadline-queue-fleet-association-lifecycle of trace set simparity-wire-aws-v1 against the emulator, or by hand:

1. deadline.create_farm(displayName=...) -> farmId; deadline.create_queue(farmId=farmId, displayName=...) -> queueId; deadline.create_fleet(farmId=farmId, displayName=..., ...) -> fleetId; deadline.create_queue_fleet_association(farmId=farmId, queueId=queueId, fleetId=fleetId).
2. (Cause 1, step task#8) deadline.get_queue_fleet_association(farmId=farmId, queueId=queueId, fleetId=fleetId). Observed: 200 with "createdAt" as a JSON number of epoch seconds. Expected: 200 with createdAt as an ISO-8601 date-time string, per @timestampFormat("date-time").
3. (Cause 2, step task#9) deadline.get_queue_fleet_association(farmId=farmId, queueId=queueId, fleetId=<a syntactically valid ^fleet-[0-9a-f]{32}$ id that was never created>). Observed: 404 ResourceNotFoundException, message "The queueFleetAssociation <fleetId> can't be found.", resourceType "queueFleetAssociation". Expected: 404 ResourceNotFoundException, message 'Resource of type fleet with id <fleetId> does not exist.', resourceType "fleet" (resourceId already correct).
4. (Cause 3, step task#11) deadline.update_queue_fleet_association(farmId=farmId, queueId=queueId, fleetId=fleetId, status="STOP_SCHEDULING_AND_CANCEL_TASKS"), then poll deadline.get_queue_fleet_association(farmId=farmId, queueId=queueId, fleetId=fleetId) for status "STOPPED". Observed: status stays "ACTIVE" with no updatedAt/updatedBy until the 60s poll cap (13 attempts). Expected: the association reaches "STOPPED" and reports updatedAt (date-time string) and updatedBy. (UpdateQueueFleetAssociation is currently an unimplemented stub, so the update itself answers 501 here — standing it up must also make this read converge.)
5. (Cause 4, step task#13) deadline.delete_queue_fleet_association(farmId=farmId, queueId=queueId, fleetId=fleetId), then deadline.get_queue_fleet_association(farmId=farmId, queueId=queueId, fleetId=fleetId). Observed: the read answers 200 with the association body, no error members and no x-amzn-errortype header. Expected: 404 ResourceNotFoundException with resourceType "queue-fleet-association", resourceId "<farmId>, <queueId> and <fleetId>", message 'Resource of type queue-fleet-association with id <farmId>, <queueId> and <fleetId> does not exist.', and an x-amzn-errortype header. (DeleteQueueFleetAssociation is currently an unimplemented stub, so the delete itself answers 501 here.)
6. (Cause 5, step teardown#14) after that delete, deadline.delete_fleet(farmId=farmId, fleetId=fleetId). Observed: 409 ConflictException "The fleet <fleetId> can't be deleted because it is associated with the queue <queueId>." Expected: 200 with an empty body, because the association no longer exists.
