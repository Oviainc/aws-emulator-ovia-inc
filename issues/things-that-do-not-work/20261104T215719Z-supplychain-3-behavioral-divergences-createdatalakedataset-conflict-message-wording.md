Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: supplychain.create-dataset-duplicate-name-conflict-then-new-name-succeeds supplychain.delete-namespace-with-dataset-conflict-then-succeeds supplychain.send-event-dataset-target-missing-ref-not-found
Issue-Shape-Id: com.amazonaws.supplychain#GalaxyPublicAPIGateway
Issue-Key: supplychain.create-dataset-duplicate-name-conflict-then-new-name-succeeds|run#12|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: supplychain.delete-namespace-with-dataset-conflict-then-succeeds|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: supplychain.send-event-dataset-target-missing-ref-not-found|run#11|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Commit: a882deb4fd953cade7fc78cd30fe0fd0d0c00170

# supplychain: 3 behavioral divergences — CreateDataLakeDataset conflict message wording, DeleteDataLakeNamespace unimplemented (non-empty-namespace refusal + not-exist wording), SendDataIntegrationEvent missing-dataset-target refusal

> _Note: this issue was identified against the code base at commit `a882deb4fd953cade7fc78cd30fe0fd0d0c00170`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataLakeDataset

## Description
Bundled behavioral divergences for the AWS Supply Chain (scn) emulator, grounded in 3 failing checks of the behavioral suite. This issue carries THREE distinct root causes (Cause 1 of 3 .. Cause 3 of 3); ALL THREE must be closed before submitting. The service is bootstrapped (its bootstrap issue is resolved) and does answer, so these are fidelity defects, not bootstrap gaps.

Already-filed failures were skipped and are NOT part of this issue: the clusters the suite marked ALREADY FILED against unimplemented CreateDataIntegrationFlow / ListDataLakeNamespaces / ListDataLakeDatasets / UpdateDataLakeDataset / SendDataIntegrationEvent / ListDataIntegrationEvents / TagResource stubs, and the six checks the suite marked OWNED by 20261029T191915Z-supplychain-7-behavioral-divergences-instance-not-found-wording.md. That OWNED issue covers instance not-found wording, versionNumber bumping, validation-message value echo, the Deleting window, GetDataIntegrationFlowExecution, CreateDataLakeNamespace conflict semantics and TagResource denial — it does NOT cover any of the three causes below.

=== Cause 1 of 3: CreateDataLakeDataset's duplicate-name ConflictException message uses the wrong template and appends an instance clause AWS does not emit ===
AWS: Smithy: CreateDataLakeDataset is PUT /api/datalake/instance/{instanceId}/namespaces/{namespace}/datasets/{name}, @idempotent, and declares ConflictException (errorKind client, httpError 409, doc "Updating or deleting a resource can cause an inconsistent state"). The model does not fix the message text, so recorded behavior only: a second create for an instanceId/namespace/name triple that already holds a dataset is refused 409 ConflictException with the message "Dataset <name> already exist in namespace <namespace>". Fixed text: "Dataset " / " already exist in namespace " — capital D, the singular verb form "already exist", and NOTHING after the namespace. Variable parts: <name> and <namespace>, both taken verbatim from the request path. The instanceId is NOT part of the message even though it is part of the resource identity.
Emulator: answers 409 ConflictException (code correct) with the message "dataset <name> already exists in namespace <namespace> of instance <instanceId>" — lower-case "dataset", the verb inflected as "already exists", and an extra " of instance <instanceId>" clause interpolated from the request. Observed: expected 'Dataset sim2realscdsc577e0088a05 already exist in namespace sim2realscnsf53f7acd99bf', got 'dataset sim2realscdsc577e0088a05 already exists in namespace sim2realscnsf53f7acd99bf of instance 223c0788-0fec-4587-b272-f2c1166d0bf7'. The generated names and the instance UUID are per-run values that merely fill the template; the divergence is the wording and the extra clause.
Why: a client that asserts on, logs, parses or surfaces AWS's documented conflict text for a duplicate dataset sees an unrecognised message, and the emulator leaks an identifier echo AWS keeps out of the response.
Fix: the ConflictException construction in the CreateDataLakeDataset handler's duplicate-triple refusal (the handler, not the shared validation layer): emit the fixed text "Dataset "/" already exist in namespace " with only name and namespace interpolated.
Traces: supplychain.create-dataset-duplicate-name-conflict-then-new-name-succeeds. Code site: services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/create_data_lake_dataset.rs:250-258. (The rest of that check passes: the rejected create leaves the original dataset's description and schema untouched, and a differently-named create still succeeds.)

=== Cause 2 of 3: DeleteDataLakeNamespace is unimplemented — no non-empty-namespace refusal, no delete, no not-exist wording (NOT covered by any auto-filed stub issue) ===
AWS: Smithy: DeleteDataLakeNamespace is DELETE /api/datalake/instance/{instanceId}/namespaces/{name}, @idempotent, output {instanceId, name}, and declares ValidationException (httpError 400, "The input does not satisfy the constraints specified by an AWS service") and ResourceNotFoundException (httpError 404, "Request references a resource which does not exist"). The model does not fix the message text, so recorded behavior only for the templates:
  (a) deleting a namespace that still contains at least one dataset is refused 400 ValidationException with message "Datasets found under the namespace <name>. Please delete them before deleting the namespace" — fixed text "Datasets found under the namespace " / ". Please delete them before deleting the namespace", with <name> taken from the request path; the namespace and its datasets are left intact (a following GetDataLakeNamespace still returns it);
  (b) once the namespace holds no datasets, DeleteDataLakeNamespace succeeds (HTTP 200) and the namespace becomes unaddressable: GetDataLakeNamespace answers 404 ResourceNotFoundException with message "Namespace <name> not exist" — fixed text "Namespace " / " not exist", with <name> from the request;
  (c) a repeat DeleteDataLakeNamespace on the now-gone name answers the same 404 ResourceNotFoundException with that same "Namespace <name> not exist" message (the @idempotent trait does not make a repeat delete succeed).
Emulator: DeleteDataLakeNamespace answers HTTP 501 with the unmodeled error code NotImplemented for every request; the handler still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and its handle() body is `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteDataLakeNamespace"))`. The suite did NOT mark this check ALREADY FILED and no open issue names this operation, so nothing else covers it. Consequence: none of (a), (b) or (c) is observable — no namespace can ever be deleted.
Why: a client cannot delete a namespace at all, cannot discover that a namespace is non-empty (it gets an unmodeled 501 instead of the modeled 400 that names the blocker), and cannot observe a deleted namespace becoming not-found; any teardown or namespace-lifecycle flow is stuck.
Fix: services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/delete_data_lake_namespace.rs — implement the handler against the namespace/dataset store in src/state.rs: refuse with ValidationException while any dataset of that instance/namespace exists, otherwise remove the namespace record and return {instanceId, name}; make reads and repeat deletes of a removed namespace refuse with ResourceNotFoundException using the wording above. Delete the SCAFFOLDED marker line when done.
Traces: supplychain.delete-namespace-with-dataset-conflict-then-succeeds.

=== Cause 3 of 3: SendDataIntegrationEvent does not refuse a datasetTarget naming a nonexistent dataset ===
AWS: Smithy: SendDataIntegrationEvent is POST /api-data/data-integration/instance/{instanceId}/data-integration-events, with required eventType (enum, here scn.data.dataset), required data (@sensitive, @length 1..1048576), required eventGroupId (@length 1..255), optional datasetTarget (DataIntegrationEventDatasetTargetConfiguration), and declares ResourceNotFoundException (errorKind client, httpError 404). The model does not fix the message text, so recorded behavior only: when datasetTarget.datasetIdentifier is a well-formed dataset ARN (arn:aws:scn:<region>:<account>:instance/<instanceId>/namespaces/<ns>/datasets/<ds>) for a dataset that does not exist, the call is refused 404 ResourceNotFoundException with message "Target dataset resource arn: <arn> is not valid" — fixed text "Target dataset resource arn: " / " is not valid", with <arn> taken verbatim from the request's datasetTarget.datasetIdentifier. The refusal has no effect: no event is recorded, so the eventGroupId does not appear in ListDataIntegrationEvents. Once the dataset is created the identical event is accepted and its load reports SUCCEEDED.
Emulator: answers HTTP 501 with the unmodeled error code NotImplemented — the handler is a scaffold stub (`Err(crate::scaffolded::Scaffolded::not_implemented("SendDataIntegrationEvent"))`). Observed: expected ResourceNotFoundException, got NotImplemented.
Why: a producer that relies on the dataset-target existence check to detect a mis-addressed ARN gets an unmodeled 501 instead of the modeled 404, so its not-found handling never runs and it cannot tell a bad target from an outage.
Fix: services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/send_data_integration_event.rs — implement the handler and, for eventType scn.data.dataset, resolve datasetTarget.datasetIdentifier against the dataset store in src/state.rs and refuse an unresolvable ARN with ResourceNotFoundException using the wording above, recording no event. NOTE: the bare 501 for this operation is also covered by the auto-filed "Implement supplychain SendDataIntegrationEvent: unimplemented scaffold stub" issue; what this cause adds, and that issue does not state, is the missing-dataset-target refusal contract (error code, message template, and the absence of any recorded event) — an implementation that accepted the event unconditionally would still diverge.
Traces: supplychain.send-event-dataset-target-missing-ref-not-found.

## Evidence
Behavioral suite: supplychain 3/23 passed (13%), 20 failed. Three failing checks are clustered here; the remaining failures were skipped as ALREADY FILED (unimplemented CreateDataIntegrationFlow / ListDataLakeNamespaces / ListDataLakeDatasets / UpdateDataLakeDataset / SendDataIntegrationEvent / ListDataIntegrationEvents / TagResource stubs) or OWNED by 20261029T191915Z-supplychain-7-behavioral-divergences-instance-not-found-wording.md.

Cause 1 — [mismatch] supplychain.create-dataset-duplicate-name-conflict-then-new-name-succeeds:
  check_supplychain_create_dataset_duplicate_name_conflict_then_new_name_succeeds.py:56
  AssertionError: expected 'Dataset sim2realscdsc577e0088a05 already exist in namespace sim2realscnsf53f7acd99bf', got 'dataset sim2realscdsc577e0088a05 already exists in namespace sim2realscnsf53f7acd99bf of instance 223c0788-0fec-4587-b272-f2c1166d0bf7'
  The error CODE matched (ConflictException, HTTP 409); only the message diverges. Fixed-text divergences: "Dataset" -> "dataset" and "already exist" -> "already exists"; plus an added " of instance <instanceId>" clause. Variable parts filled from the request: the dataset name and the namespace name (the dataset/namespace names and the instance UUID shown are per-run values, not requirements).
  Source: src/ops/create_data_lake_dataset.rs:251-256 formats "dataset {} already exists in namespace {} of instance {}" from input.name, input.namespace, input.instance_id.

Cause 2 — [mismatch] supplychain.delete-namespace-with-dataset-conflict-then-succeeds:
  check_supplychain_delete_namespace_with_dataset_conflict_then_succeeds.py:49
  AssertionError: expected ValidationException, got NotImplemented
  Expected: 400 ValidationException, Message "Datasets found under the namespace <ns>. Please delete them before deleting the namespace"; then the namespace still readable via GetDataLakeNamespace; then, after DeleteDataLakeDataset, DeleteDataLakeNamespace succeeds and both GetDataLakeNamespace and a repeat DeleteDataLakeNamespace answer 404 ResourceNotFoundException with Message "Namespace <ns> not exist".
  Actual: HTTP 501, error code NotImplemented, for the very first DeleteDataLakeNamespace call.
  Source: src/ops/delete_data_lake_namespace.rs:4 still declares "SCAFFOLDED, NOT IMPLEMENTED"; line 171 is `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteDataLakeNamespace"))`. No open issue names this operation, and the suite did not mark this check ALREADY FILED.

Cause 3 — [mismatch] supplychain.send-event-dataset-target-missing-ref-not-found:
  check_supplychain_send_event_dataset_target_missing_ref_not_found.py:46
  AssertionError: expected ResourceNotFoundException, got NotImplemented
  Expected: 404 ResourceNotFoundException, Message "Target dataset resource arn: arn:aws:scn:<region>:<account>:instance/<instanceId>/namespaces/<ns>/datasets/<ds> is not valid" (fixed text "Target dataset resource arn: "/" is not valid", the ARN copied from datasetTarget.datasetIdentifier), and the eventGroupId absent from ListDataIntegrationEvents afterwards.
  Actual: HTTP 501, error code NotImplemented.
  Source: src/ops/send_data_integration_event.rs:202 `Err(crate::scaffolded::Scaffolded::not_implemented("SendDataIntegrationEvent"))`.

## Steps to Reproduce
Boot the emulator and replay the three supplychain behavioral checks below. All of them pin the client to us-east-1 (ctx.client("supplychain", region="us-east-1")), because real AWS Supply Chain refuses CreateInstance elsewhere; each first calls CreateInstance and polls GetInstance until the instance is Active.

Cause 1 (supplychain.create-dataset-duplicate-name-conflict-then-new-name-succeeds):
1. CreateInstance -> poll GetInstance to Active.
2. CreateDataLakeNamespace(instanceId, name=<ns>) with a lowercase ^[a-z0-9_]+$ name.
3. CreateDataLakeDataset(instanceId, namespace=<ns>, name=<ds>, schema={name:"OriginalSchema", fields:[{name:"id",type:"STRING",isRequired:true}]}, description="original description").
4. CreateDataLakeDataset again with the SAME instanceId/namespace/name but a different schema and description.
   -> expect HTTP 409 ConflictException with Message exactly "Dataset <ds> already exist in namespace <ns>" (no " of instance ..." suffix, capital "Dataset", verb "already exist").
5. GetDataLakeDataset must still report description "original description" and schema name "OriginalSchema"; a CreateDataLakeDataset with a different name must still succeed.

Cause 2 (supplychain.delete-namespace-with-dataset-conflict-then-succeeds):
1. CreateInstance -> Active; CreateDataLakeNamespace(instanceId, name=<ns>).
2. CreateDataLakeDataset(instanceId, namespace=<ns>, name=<ds>, schema={name:"SimSchema", fields:[{name:"id",type:"STRING",isRequired:true}]}).
3. DeleteDataLakeNamespace(instanceId, name=<ns>)
   -> expect HTTP 400 ValidationException, Message "Datasets found under the namespace <ns>. Please delete them before deleting the namespace".
4. GetDataLakeNamespace(instanceId, name=<ns>) must still return the namespace (the refused delete had no effect).
5. DeleteDataLakeDataset(instanceId, namespace=<ns>, name=<ds>), then DeleteDataLakeNamespace(instanceId, name=<ns>) -> expect HTTP 200.
6. GetDataLakeNamespace(instanceId, name=<ns>) -> expect 404 ResourceNotFoundException, Message "Namespace <ns> not exist".
7. DeleteDataLakeNamespace(instanceId, name=<ns>) again -> expect the same 404 ResourceNotFoundException with Message "Namespace <ns> not exist".

Cause 3 (supplychain.send-event-dataset-target-missing-ref-not-found):
1. CreateInstance -> Active; CreateDataLakeNamespace(instanceId, name=<ns>). Do NOT create the dataset yet.
2. Build missing_dataset_arn = arn:aws:scn:<region>:<account>:instance/<instanceId>/namespaces/<ns>/datasets/<ds>.
3. SendDataIntegrationEvent(instanceId, eventType="scn.data.dataset", eventGroupId=<group>, data=<small JSON object>, datasetTarget={datasetIdentifier: missing_dataset_arn, operationType: "APPEND"})
   -> expect HTTP 404 ResourceNotFoundException, Message "Target dataset resource arn: <missing_dataset_arn> is not valid".
4. ListDataIntegrationEvents(instanceId) must not contain <group> (the refusal recorded nothing).
5. Now CreateDataLakeDataset for that same namespace/name and resend the identical event -> expect HTTP 200 with an eventId, and GetDataIntegrationEvent to report the load as SUCCEEDED.
