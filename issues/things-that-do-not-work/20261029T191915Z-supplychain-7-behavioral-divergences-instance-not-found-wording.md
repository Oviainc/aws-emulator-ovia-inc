Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: supplychain.get-instance-nonexistent-not-found supplychain.instance-delete-lifecycle-to-deleted supplychain.instance-update-persists-and-bumps-version supplychain.list-instances-maxresults-out-of-range-rejected supplychain.send-event-payload-too-large-rejected supplychain.update-instance-during-deletion-rejected supplychain.get-flow-execution-nonexistent-not-found supplychain.concurrent-create-namespace-same-name-single-winner supplychain.tag-resource-missing-permission-rejected
Issue-Shape-Id: com.amazonaws.supplychain#GalaxyPublicAPIGateway
Issue-Key: supplychain.get-instance-nonexistent-not-found|run#11|AssertionError|assert message == "Instance not found.", f"expected 'Instance not found.', got {message!r}"
Issue-Key: supplychain.instance-delete-lifecycle-to-deleted|run#8|AssertionError|assert message == "Instance not found.", f"expected 'Instance not found.', got {message!r}"
Issue-Key: supplychain.instance-update-persists-and-bumps-version|run#9|AssertionError|assert updated["versionNumber"] == before["versionNumber"], (
Issue-Key: supplychain.list-instances-maxresults-out-of-range-rejected|run#5|AssertionError|assert message_high == expected_high, f"expected {expected_high!r}, got {message_high!r}"
Issue-Key: supplychain.send-event-payload-too-large-rejected|run#12|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: supplychain.update-instance-during-deletion-rejected|run#9|AssertionError|assert error["Code"] == "ValidationException", (
Issue-Key: supplychain.get-flow-execution-nonexistent-not-found|run#6|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: supplychain.concurrent-create-namespace-same-name-single-winner|run#10|AssertionError|assert len(successes) == 1, f"expected exactly one winner, got {len(successes)}: {results}"
Issue-Key: supplychain.tag-resource-missing-permission-rejected|run#13|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Commit: 0ebbd81840eb1d8236e49c909db275b2251d47fa

# supplychain: 7 behavioral divergences — instance not-found wording, versionNumber bump, validation-message value echo, no Deleting window, and three unimplemented/denial semantics

> _Note: this issue was identified against the code base at commit `0ebbd81840eb1d8236e49c909db275b2251d47fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateInstance (bundle also covers GetInstance, DeleteInstance, ListInstances, SendDataIntegrationEvent, GetDataIntegrationFlowExecution, CreateDataLakeNamespace, TagResource)

## Description
Bundled behavioral divergences for the AWS Supply Chain (scn) emulator, grounded in 9 failing checks of the behavioral suite. This issue carries SEVEN distinct root causes; all seven must be closed before submitting. No open supplychain issue existed when this was filed (list_existing_issues 'supplychain' -> none open); the bootstrap issue for the service is already resolved, so the service does answer and answers wrongly.

=== Cause 1 of 7: ResourceNotFoundException message for a nonexistent instance uses the wrong template ===
AWS: Smithy: GetInstance / DeleteInstance / UpdateInstance all declare ResourceNotFoundException (errorKind client, httpError 404, doc "Request references a resource which does not exist"). The model does not fix the message text, so recorded behavior only: real AWS answers the message as the FIXED string "Instance not found." — it has no variable parts at all, and in particular does NOT echo the requested instanceId.
Emulator: answers 404 ResourceNotFoundException with the message "instance <instanceId> does not exist" — a different fixed text, plus the instanceId interpolated from the request (observed: 'instance d0e75531-eb2d-4a3b-a1bf-8dd8e148e377 does not exist', where only the UUID is per-run; the wording is the defect).
Why: a client that classifies or surfaces AWS error messages, or that asserts on the documented not-found text, sees an unknown message; the emulator also leaks a request echo AWS does not emit.
Fix: the not-found refusal of the instance handlers — the ResourceNotFoundException construction in the GetInstance, DeleteInstance and UpdateInstance handlers must emit the fixed text "Instance not found." with no interpolation.
Traces: supplychain.get-instance-nonexistent-not-found, supplychain.instance-delete-lifecycle-to-deleted. Code sites: services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/update_instance.rs:166-176 and .../src/ops/delete_instance.rs:169-179 (the GetInstance handler uses the same template).

=== Cause 2 of 7: UpdateInstance advances versionNumber ===
AWS: Smithy: Instance.versionNumber is a Double, documented only as "The version number of the instance"; UpdateInstance's model says nothing about advancing it. Recorded behavior only: real AWS returns the IDENTICAL versionNumber before an UpdateInstance, in the UpdateInstance response, on an independent GetInstance, and after a second successive UpdateInstance that changed instanceName/instanceDescription — versionNumber is not an optimistic-lock counter the update bumps.
Emulator: the value increases by 1.0 on every mutating call (before=1.0, after=2.0), because the UpdateInstance handler calls InstanceRecord::touch, which bumps version_number along with last_modified_time.
Why: a client that caches an instance and compares versionNumber to detect a stale copy sees a change AWS never reports, and any consumer pinning the value across an update diverges.
Fix: the UpdateInstance handler / the shared record-mutation helper in the service state — lastModifiedTime may advance on an update, versionNumber must not.
Traces: supplychain.instance-update-persists-and-bumps-version. Code sites: .../src/ops/update_instance.rs:178-190 and .../src/state.rs:139-144 (InstanceRecord::touch).

=== Cause 3 of 7: ValidationException constraint message echoes the rejected value ===
AWS: Smithy: ListInstances.maxResults targets InstanceMaxResults with @range min 0 max 20; SendDataIntegrationEvent.data targets DataIntegrationEventData with @length min 1 max 1048576 and is @sensitive. Both operations declare ValidationException (httpError 400, "The input does not satisfy the constraints specified by an AWS service"). Recorded behavior for the message template: "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must have <constraint clause>" — the fixed text is "1 validation error detected: Value at '" / "' failed to satisfy constraint: Member must have "; the variable parts are the member name taken from the request's member path and the constraint clause taken from the model's bound ("value less than or equal to 20" for maxResults, "length less than or equal to 1048576" for data). The rejected VALUE is not part of the message for these members.
Emulator: inserts the rejected value: "1 validation error detected: Value '21' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20", and for data it inlines a prefix of the payload plus a byte count ("Value '{\"supplier_tpartner_id\": ... [1048872 bytes]' at 'data' failed to satisfy constraint: ...").
Why: the message text differs from AWS for every constraint violation the service can report, so message assertions and log scrapers diverge; worse, for data the emulator writes a @sensitive member's content into an error response AWS keeps out of it.
Fix: the shared constraint-violation message formatter in the generated service's centralized validation layer (deserialize -> validate step of the request walk), not the individual handlers.
Traces: supplychain.list-instances-maxresults-out-of-range-rejected, supplychain.send-event-payload-too-large-rejected.

=== Cause 4 of 7: DeleteInstance hard-deletes synchronously, so there is no Deleting window and UpdateInstance is not state-gated ===
AWS: Smithy: DeleteInstance is @idempotent, returns the Instance (state Deleting) with HTTP 200, and UpdateInstance declares ValidationException (400) alongside ResourceNotFoundException (404). Recorded behavior: DeleteInstance flips the instance to Deleting and the record stays addressable while the deletion settles; an UpdateInstance issued in that window is refused with ValidationException whose message names the state requirement — "Instance must be in Active state to perform this operation." (AWS also answers the equivalent variant "Instance must be in Active or Inactive state to perform this operation."); the rejected call does not mutate the description. Only later does the instance become un-addressable (ResourceNotFoundException).
Emulator: DeleteInstance removes the record from the store immediately and only decorates the copy it returns with state Deleting, so the record is gone the instant the call returns; the subsequent UpdateInstance answers 404 ResourceNotFoundException with message "instance <instanceId> does not exist" instead of 400 ValidationException with the Active-state message.
Why: a client that mutates or polls an instance during deletion gets the wrong error class (404 vs 400) and no indication that the state machine, not the instance's existence, refused the call; the Deleting state is unobservable to any reader.
Fix: the DeleteInstance handler's state transition plus record retention in the service state (keep the record in Deleting until the deletion settles) and a state gate in the UpdateInstance handler that refuses a non-Active instance with ValidationException.
Traces: supplychain.update-instance-during-deletion-rejected. Code sites: .../src/ops/delete_instance.rs:169-194, .../src/ops/update_instance.rs:166-195, .../src/state.rs:176-182.

=== Cause 5 of 7: GetDataIntegrationFlowExecution is unimplemented (not covered by any auto-filed stub cluster) ===
AWS: Smithy: GetDataIntegrationFlowExecution GET /api-data/data-integration/instance/{instanceId}/data-integration-flows/{flowName}/executions/{executionId}, errors include AccessDeniedException (httpError 403) and ResourceNotFoundException (404). Recorded behavior: for an instanceId/flowName/executionId triple that was never created, AWS evaluates authorization against the instance resource ARN BEFORE any existence lookup and answers 403 AccessDeniedException, message "Authorization Failed (Public). The instanceId <instanceId> does not exist, or does not belong to the account <accountId>" — fixed text "Authorization Failed (Public). The instanceId " / " does not exist, or does not belong to the account ", with instanceId taken from the request and the account id from the caller's identity. It is NOT a 404.
Emulator: answers HTTP 501 NotImplemented ("operation GetDataIntegrationFlowExecution is scaffolded but not implemented in this emulator"). The handler still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker. The suite did NOT mark this check as ALREADY FILED, so no auto-filed stub issue covers this operation.
Why: a client probing a flow execution gets a 501 with an unmodeled error code instead of the modeled 403 refusal, so error handling for the missing-instance case never runs.
Fix: .../src/ops/get_data_integration_flow_execution.rs — implement the operation and its instance-authorization refusal.
Traces: supplychain.get-flow-execution-nonexistent-not-found.

=== Cause 6 of 7: CreateDataLakeNamespace must treat a duplicate name as a conflict, with exactly one winner under concurrency ===
AWS: Smithy: CreateDataLakeNamespace is PUT /api/datalake/instance/{instanceId}/namespaces/{name}, marked @idempotent, and declares ConflictException (httpError 409). Recorded behavior: despite the PUT shape, a duplicate instance/name is a CONFLICT and not an idempotent overwrite — with N concurrent creates of the same instanceId/name, exactly one caller receives HTTP 200 with the namespace and every other caller receives ConflictException; a following GetDataLakeNamespace returns the single namespace under that name.
Emulator: every racer receives HTTP 501 NotImplemented, so zero winners and zero conflicts. The bare stub for CreateDataLakeNamespace is already covered by the auto-filed unimplemented-operation issue; what this cause adds, and that issue does not state, is the conflict/single-winner requirement the implementation must satisfy — implementing the operation as an idempotent PUT overwrite would still diverge.
Why: a client that relies on create-wins-once to claim a namespace name (or that handles ConflictException to discover an existing namespace) cannot distinguish winner from loser.
Fix: .../src/ops/create_data_lake_namespace.rs plus a namespace store in .../src/state.rs, with the duplicate-name check and the insert performed under the single state lock so concurrent creates serialize to one winner.
Traces: supplychain.concurrent-create-namespace-same-name-single-winner.

=== Cause 7 of 7: TagResource must refuse an unauthorized principal with AccessDeniedException and apply no tag ===
AWS: Smithy: TagResource POST /api/tags/{resourceArn}, errors include AccessDeniedException (httpError 403, "You do not have the required privileges to perform this action"). Recorded behavior: a principal that assumed an IAM role with no identity-based permissions gets 403 AccessDeniedException with the message "User: <caller principal ARN> is not authorized to perform: scn:TagResource on resource: <resourceArn> because no identity-based policy allows the scn:TagResource action" — fixed text "User: " / " is not authorized to perform: scn:TagResource on resource: " / " because no identity-based policy allows the scn:TagResource action", with the principal ARN taken from the signing credentials (the assumed-role ARN) and resourceArn taken from the request; afterwards ListTagsForResource shows the tag was NOT applied, while an authorized caller tags the same resource successfully.
Emulator: answers HTTP 501 NotImplemented for TagResource regardless of the caller. The bare stub is covered by the auto-filed unimplemented-TagResource issue; the authorization-denial semantics above are not, and an implementation that tags unconditionally would still diverge.
Why: a client testing least-privilege behavior sees an unmodeled 501 instead of the modeled 403, and (once the stub is filled in naively) an unauthorized caller would silently succeed — an overly permissive emulator.
Fix: .../src/ops/tag_resource.rs plus the request's authorization seam (the signed principal in the operation context) so an unauthorized caller is refused before the tag is written.
Traces: supplychain.tag-resource-missing-permission-rejected.

## Evidence
Behavioral suite: supplychain 1/23 passed (4%), 22 failed. Nine failing checks are clustered here (the five checks the report marked ALREADY FILED against unimplemented CreateBillOfMaterialsImportJob / CreateDataLakeNamespace / CreateDataLakeDataset / ListDataLakeNamespaces / TagResource stubs are skipped).

Cause 1 — supplychain.get-instance-nonexistent-not-found:
  AssertionError: expected 'Instance not found.', got 'instance d0e75531-eb2d-4a3b-a1bf-8dd8e148e377 does not exist'
  supplychain.instance-delete-lifecycle-to-deleted:
  AssertionError: expected 'Instance not found.', got 'instance a0738ec7-9509-44d6-a85e-ba117a7332f8 does not exist'
  (error code ResourceNotFoundException matched; only the message text diverges; the UUID is per-run, the wording is not)

Cause 2 — supplychain.instance-update-persists-and-bumps-version:
  AssertionError: versionNumber changed on a plain UpdateInstance: before=1.0 after=2.0
  (expected before == after; src/state.rs:141-144 `touch` does `version_number += 1.0`)

Cause 3 — supplychain.list-instances-maxresults-out-of-range-rejected:
  expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20"
  got      "1 validation error detected: Value '21' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20"
  supplychain.send-event-payload-too-large-rejected:
  expected "1 validation error detected: Value at 'data' failed to satisfy constraint: Member must have length less than or equal to 1048576"
  got      "1 validation error detected: Value '{\"supplier_tpartner_id\": \"...\", ... [1048872 bytes]' at 'data' failed to satisfy constraint: Member must have length less than or equal to 1048576"
  (error code ValidationException matched in both; the emulator adds a "Value '<value>'" clause AWS omits, and the echoed member `data` is @sensitive)

Cause 4 — supplychain.update-instance-during-deletion-rejected:
  AssertionError: expected ValidationException while Deleting, got {'Message': 'instance 317b4800-2555-4ee0-8731-c2ceee4739c6 does not exist', 'Code': 'ResourceNotFoundException'}
  expected code ValidationException (HTTP 400) with message "Instance must be in Active state to perform this operation." (or the "Active or Inactive" variant); got ResourceNotFoundException (HTTP 404). Root cause visible in src/ops/delete_instance.rs:169 — `store.instances.remove(...)` drops the record, then line 187 sets state=Deleting only on the returned copy.

Cause 5 — supplychain.get-flow-execution-nonexistent-not-found:
  AssertionError: expected AccessDeniedException, got NotImplemented
  expected 403 AccessDeniedException "Authorization Failed (Public). The instanceId <id> does not exist, or does not belong to the account <account>"; got HTTP 501 NotImplemented. src/ops/get_data_integration_flow_execution.rs still declares "SCAFFOLDED, NOT IMPLEMENTED" and this check was NOT marked ALREADY FILED by the suite.

Cause 6 — supplychain.concurrent-create-namespace-same-name-single-winner:
  AssertionError: expected exactly one winner, got 0: [('error','NotImplemented') x6]
  expected 1 x HTTP 200 and 5 x ConflictException from 6 concurrent CreateDataLakeNamespace calls on the same instanceId/name.

Cause 7 — supplychain.tag-resource-missing-permission-rejected:
  AssertionError: expected AccessDeniedException, got NotImplemented
  expected 403 AccessDeniedException "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: scn:TagResource on resource: <instance arn> because no identity-based policy allows the scn:TagResource action", and ListTagsForResource unchanged; got HTTP 501 NotImplemented.

## Steps to Reproduce
Boot the emulator and replay the supplychain behavioral checks (all pinned to region us-east-1, where AWS Supply Chain is served):

Cause 1: CreateInstance, poll GetInstance to Active (positive control), then GetInstance with a fresh random UUID -> expect 404 ResourceNotFoundException, Message exactly "Instance not found.". Same for DeleteInstance on an instance already fully deleted.
Cause 2: CreateInstance -> Active; GetInstance to read versionNumber; UpdateInstance with a new instanceName+instanceDescription; assert the response versionNumber equals the pre-update one, that an independent GetInstance agrees, and that a second UpdateInstance still reports the same versionNumber.
Cause 3: with botocore parameter_validation disabled, ListInstances(maxResults=21) -> expect ValidationException whose Message is "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20" (no "Value '21'" clause); and SendDataIntegrationEvent with eventType=scn.data.shipment and a `data` JSON payload > 1048576 bytes -> expect "1 validation error detected: Value at 'data' failed to satisfy constraint: Member must have length less than or equal to 1048576" with no payload echoed.
Cause 4: CreateInstance -> Active; UpdateInstance (succeeds, positive control); DeleteInstance (returns state Deleting); immediately UpdateInstance again -> expect 400 ValidationException naming the Active-state requirement, and GetInstance must still return the pre-delete description.
Cause 5: GetDataIntegrationFlowExecution with a random instanceId, flowName "sim2real-nonexistent-flow" and a random executionId -> expect 403 AccessDeniedException with the "Authorization Failed (Public). The instanceId ... does not exist, or does not belong to the account ..." message.
Cause 6: CreateInstance -> Active; fire 6 concurrent CreateDataLakeNamespace calls with the same instanceId and the same lowercase name -> expect exactly one HTTP 200 and five ConflictException, then GetDataLakeNamespace returns that name.
Cause 7: CreateInstance -> Active; create an IAM role with no attached policies, assume it via STS, call TagResource(resourceArn=arn:aws:scn:us-east-1:<account>:instance/<instanceId>, tags={team:sim2real}) with those credentials -> expect 403 AccessDeniedException with the "is not authorized to perform: scn:TagResource" message and no tag applied; the privileged caller can tag the same resource.
