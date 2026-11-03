Issue-Source: trace-sweep-divergence
Issue-Trace: detective-investigation-error-paths
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: detective-investigation-error-paths
Issue-Shape-Id: com.amazonaws.detective#AmazonDetective
Issue-Key: detective|GetInvestigation|4xx->4xx|$.Message
Issue-Key: detective|UpdateInvestigationState|4xx->4xx|$.Message
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# detective ResourceNotFoundException omits its Message member

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInvestigation

## Description
The Amazon Detective emulator returns the right HTTP status for a lookup of an investigation that does not exist (404), but the body of the ResourceNotFoundException carries no `Message` member at all. Smithy: com.amazonaws.detective#ResourceNotFoundException is a @error("client") with @httpError(404) and a single member `Message` (target ErrorMessage), documented "The request refers to a nonexistent resource."; it is a modeled error of both GetInvestigation and UpdateInvestigationState (restJson1). Both bundled causes are the same defect: the not-found error value is constructed/serialized without populating `Message`. This issue bundles 2 causes, both on service `detective`.

Cause 1 of 2: GetInvestigation on an unknown InvestigationId (fixture detective-investigation-error-paths, step task#4).
- AWS: returns HTTP 404 with a ResourceNotFoundException whose `Message` is populated. For GetInvestigation on an investigation id that does not resolve in the given graph, the recorded response's message is the fixed text `The specified investigation does not exist` (no variable parts — the id and graph ARN from the request are not interpolated into this text). Status/error class are modeled; the exact message is recorded behavior only.
- Emulator: answers 404 but the JSON body has no `Message` key at all ($.Message missing, got None), so the error has an empty/absent reason string.
- Why: SDK clients surface `e.response['Error']['Message']` / `ResourceNotFoundException.message` as the user-visible reason. With the member absent, botocore reports an empty message, retry/alerting logic that matches on the reason text sees nothing, and an operator cannot distinguish "no such investigation" from any other 404 produced by the service.
- Fix: the GetInvestigation handler's not-found path (and/or the shared ResourceNotFoundException constructor it uses) must populate the `Message` member before the restJson1 error serializer runs, in the detective service crate's ops/get_investigation.rs and its error-construction helper.
- Traces: detective-investigation-error-paths#task#4 (expected $.Message = 'The specified investigation does not exist', actual missing).

Cause 2 of 2: UpdateInvestigationState on an investigation that cannot be resolved (fixture detective-investigation-error-paths, step task#9).
- AWS: returns HTTP 404 ResourceNotFoundException with `Message` populated. For UpdateInvestigationState the recorded message is the fixed text `Unable to find Investigation based on the Graph Arn or Investigation ID provided` — note it is a DIFFERENT fixed string from GetInvestigation's, and again has no variable parts interpolated from the request; a single shared generic message is not what AWS returns. Recorded behavior only for the wording; the 404/ResourceNotFoundException pairing is modeled.
- Emulator: answers 404 with no `Message` key in the body ($.Message missing, got None).
- Why: clients that update investigation state and branch on the failure reason (resource gone vs. validation vs. throttling) get an empty reason; the per-operation wording AWS uses to tell callers the graph ARN *or* the investigation id was unresolvable is lost.
- Fix: the UpdateInvestigationState handler's not-found path in the detective service crate's ops/update_investigation_state.rs must build ResourceNotFoundException with its own message text, not an empty error. Keep the two operations' message templates distinct rather than sharing one generic string.
- Traces: detective-investigation-error-paths#task#9 (expected $.Message = 'Unable to find Investigation based on the Graph Arn or Investigation ID provided', actual missing).

Related but not covering this: open issue 20261029T195326Z-detective-updateinvestigationstate-stub.md ("Implement detective UpdateInvestigationState: unimplemented scaffold stub") asks for the operation to be implemented at all and is grounded in a behavioral check, not in these wire steps — it carries no replay key for fixture detective-investigation-error-paths. Cause 2 here is about the message content of the 404 that UpdateInvestigationState now returns, so it must be closed on its own evidence.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 81.2%, emulator up, 48 steps, 39 passed):

Cluster 1 — service=detective | GetInvestigation | status=404->404 | keys=$.Message
  fixture detective-investigation-error-paths, step task#4, verdict mismatch
  expected_status 404, actual_status 404
  detail: $.Message: missing (expected 'The specified investigation does not exist', got None)

Cluster 2 — service=detective | UpdateInvestigationState | status=404->404 | keys=$.Message
  fixture detective-investigation-error-paths, step task#9, verdict mismatch
  expected_status 404, actual_status 404
  detail: $.Message: missing (expected 'Unable to find Investigation based on the Graph Arn or Investigation ID provided', got None)

Model evidence: Smithy ResourceNotFoundException { Message: ErrorMessage } @error(client) @httpError(404); bound as a modeled error on GetInvestigation (POST /investigations/getInvestigation) and UpdateInvestigationState (POST /investigations/updateInvestigationState). Expected body shape is therefore {"Message": "<reason>"} plus the x-amzn-errortype/__type discriminator; the emulator emits the discriminator and status only.

Neither cluster carries a stub_path or owned_by in the replay output, so neither is already filed.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `detective-investigation-error-paths` from trace set simparity-wire-aws-v1.
2. Step task#4: call GetInvestigation with a valid GraphArn and an InvestigationId (21 digits) that was never created. Observe HTTP 404 and inspect the response body: it has no `Message` member; AWS returns `{"Message": "The specified investigation does not exist"}`.
3. Step task#9: call UpdateInvestigationState with a GraphArn/InvestigationId pair that does not resolve and a valid State. Observe HTTP 404 with no `Message` member; AWS returns `{"Message": "Unable to find Investigation based on the Graph Arn or Investigation ID provided"}`.
4. Equivalently with botocore: `client.get_investigation(...)` / `client.update_investigation_state(...)` and print `e.response['Error']['Message']` — it is empty on the emulator, non-empty on AWS.
