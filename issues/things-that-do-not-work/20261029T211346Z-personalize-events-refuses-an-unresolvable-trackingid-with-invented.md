Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-events-put-action-interactions-invalid-input
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-events-put-action-interactions-invalid-input personalize-events-put-events-invalid-input
Issue-Shape-Id: com.amazonaws.personalizeevents#AmazonPersonalizeEvents
Issue-Key: personalize-events|PutActionInteractions|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: personalize-events|PutActionInteractions|4xx->4xx|status
Issue-Key: personalize-events|PutEvents|4xx->4xx|$.message
Issue-Related: 20261029T211346Z-personalize-events-constraint-violation-messages-diverge-from-aws.md
Issue-Commit: 0622c476c3133f3ae7e7fc8f4e228880242e86b0

# personalize-events refuses an unresolvable trackingId with invented wording and the wrong 404 status

> _Note: this issue was identified against the code base at commit `0622c476c3133f3ae7e7fc8f4e228880242e86b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutActionInteractions

## Description
Both ingestion operations resolve the request's trackingId against the in-memory event_trackers map first and report their own wording when the lookup misses - PutActionInteractions additionally reports the wrong error class and HTTP status. TWO causes.

Cause 1 of 2 - PutActionInteractions answers 404 ResourceNotFoundException where AWS answers 400 InvalidInputException (fixture personalize-events-put-action-interactions-invalid-input, step task#4).
AWS: Smithy: PutActionInteractions models InvalidInputException (httpError 400, "Provide a valid value for the field or parameter"), ResourceInUseException (409) and ResourceNotFoundException (404). Recorded behavior: for this request AWS answers HTTP 400 with the message "ActionInteractions is not supported." - that text is entirely fixed, with no variable part (no id, name or account interpolated). The recorded response does not assert x-amzn-errortype, but the 400 status puts it in the InvalidInputException class, not the 404 ResourceNotFoundException class. Hypothesis: AWS decides whether the named tracker can accept action interactions at all (the tracking ID's dataset group has no Action Interactions dataset) and reports that as an input fault, before and instead of reporting a missing resource.
Emulator: HTTP 404, x-amzn-errortype: ResourceNotFoundException, message "No event tracker with the tracking ID <trackingId> exists." (the recording shows the submitted UUID filling <trackingId>; that value is this run's input, not part of any template).
Why: the status class flips from 4xx-input to 4xx-not-found, so an SDK caller that branches on ResourceNotFoundException (create-then-retry, resource-provisioning wait loops) takes a resource-provisioning path for a request that is simply not supported, and an error-code match on InvalidInputException never fires. The message a caller surfaces is also wholly different text.
Fix: the PutActionInteractions handler's target-resolution and error selection (services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/src/ops/put_action_interactions.rs:192-201), where the event_trackers miss is mapped straight to ResourceNotFoundException.
Traces: personalize-events-put-action-interactions-invalid-input#task#4.

Cause 2 of 2 - PutEvents reports an unresolvable trackingId with its own wording (fixture personalize-events-put-events-invalid-input, step task#1).
AWS: Smithy: PutEvents models InvalidInputException ONLY (httpError 400) - no ResourceNotFoundException - so an unusable trackingId is an input fault. Recorded behavior: the message is "Invalid trackingId in account <accountId>": the fixed text is "Invalid trackingId in account " and the single variable part is the 12-digit AWS account id the call is authenticated to, taken from the caller's credentials / the emulator's own account (the recording shows 000000000000 because that is this environment's account, not because the literal is part of the message). Note that AWS does NOT echo the rejected tracking ID.
Emulator: HTTP 400 (correct) with "No event tracker with the tracking ID <trackingId> exists." - invented wording that also discloses the submitted tracking id and names no account.
Why: the status is right but the message is not AWS's, so a caller matching or displaying the documented text misreports the fault; and because PutEvents cannot answer 404, the message is the ONLY signal distinguishing a bad tracking id from any other input fault.
Fix: the PutEvents handler's tracker-resolution error (services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/src/ops/put_events.rs:205-210), which must render AWS's account-scoped wording from the request's authenticated account rather than echoing the tracking id.
Traces: personalize-events-put-events-invalid-input#task#1 (same cluster: task#2).

Related but not covering: the open issues "Wire the Personalize control plane to the Personalize Events registry" and "personalize-events ingestion unreachable: personalize ListSchemas is a 501 stub, and CreateDataset never registers the dataset for PutItems" are about trackers/datasets never being registered. Even with a populated registry the refusal path above still returns the wrong status, error class and message, so these causes are separate from that wiring gap.

## Evidence
Cause 1 - personalize-events-put-action-interactions-invalid-input#task#4 (PutActionInteractions):
  status: expected 400, got 404
  $.message: expected "ActionInteractions is not supported.", got "No event tracker with the tracking ID 67b05102-bdd5-4028-9bc4-107fb72e6ef5 exists."
  $headers.x-amzn-errortype: expected <shaped-not-asserted>, got "ResourceNotFoundException"

Cause 2 - personalize-events-put-events-invalid-input#task#1 (PutEvents; cluster also covers task#2):
  status: expected 400, got 400 (match)
  $.message: expected "Invalid trackingId in account 000000000000", got "No event tracker with the tracking ID 67b05102-bdd5-4028-9bc4-107fb72e6ef5 exists."
  (000000000000 is the environment's account id - the variable part of AWS's template; the UUID is this run's request value.)

Emulator source: put_action_interactions.rs:192-201 maps an event_trackers miss to ResourceNotFoundException(404); put_events.rs:205-210 maps the same miss to InvalidInputException with the same invented sentence.
Model cited: smithy_query(operation="PutActionInteractions,PutEvents") - PutEvents errors = [InvalidInputException httpError 400] only; PutActionInteractions errors = [InvalidInputException 400, ResourceInUseException 409, ResourceNotFoundException 404].

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Cause 1: POST /action-interactions with a well-formed body naming a trackingId the emulator holds no tracker for (fixture personalize-events-put-action-interactions-invalid-input, step task#4). Observe 404 / ResourceNotFoundException / "No event tracker with the tracking ID ... exists."; AWS answers 400 with "ActionInteractions is not supported."
3. Cause 2: POST /events with that same unresolvable trackingId plus sessionId and a one-event eventList (fixture personalize-events-put-events-invalid-input, step task#1). Observe 400 with "No event tracker with the tracking ID ... exists."; AWS answers 400 with "Invalid trackingId in account <accountId>".
4. Re-replay personalize-events-put-action-interactions-invalid-input#task#4 and personalize-events-put-events-invalid-input#task#1 to confirm both causes.
