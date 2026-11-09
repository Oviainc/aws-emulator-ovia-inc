Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-cloud-connector-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-cloud-connector-error-paths
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|DeleteCloudConnector|2xx->4xx|$
Issue-Key: iot-managed-integrations|DeleteCloudConnector|2xx->4xx|status
Issue-Commit: 016c7defff76bf6b624ee66d2926fa081d59376e

# DeleteCloudConnector refuses an absent identifier with 404 instead of AWS's idempotent 200 empty-body success

> _Note: this issue was identified against the code base at commit `016c7defff76bf6b624ee66d2926fa081d59376e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCloudConnector

## Description
This bundle has 1 cause; it must be fixed before the issue is closed.

Cause 1 of 4 is not applicable — this issue carries exactly ONE cause. Cause 1 of 1: DELETE /cloud-connectors/{Identifier} for an identifier the account holds no cloud connector under is answered as a modelled refusal, where AWS answers it as a successful no-op.

AWS: Smithy (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json): DeleteCloudConnector is DELETE /cloud-connectors/{Identifier} with http success code 200, output shape Unit (an empty body), and is marked @idempotent; the sole input member Identifier is an @required @httpLabel of CloudConnectorId with @length(1..64) and @pattern ^[A-Za-z0-9-_]+$. ResourceNotFoundException (httpError 404) IS declared on the operation, so the model alone does not settle which path an absent identifier takes — for that, recorded behavior only: the corpus records this operation against an identifier that was never created (a syntactically valid CloudConnectorId matching the modelled pattern) and AWS answers HTTP 200 with an EMPTY body and no error payload at all. That is the @idempotent delete contract: the post-condition 'no cloud connector is held under this identifier' is already satisfied, so the call succeeds and the declared 404 is reserved for other conditions. Because the recorded response is a success, there is NO error code and NO error message owed on this path; the requirement is the status (200) and the empty body, not any sentence.

Emulator: HTTP 404 with the JSON error body {"__type": "ResourceNotFoundException", "Message": "Cloud connector not found.", "ResourceId": "<the Identifier from the request>", "ResourceType": "CloudConnector"} — i.e. error code ResourceNotFoundException and the fixed sentence 'Cloud connector not found.' with the submitted identifier interpolated into ResourceId and the fixed resource-class token 'CloudConnector' in ResourceType. The sentence itself is this resource's correct wording for a genuine not-found; the defect is that the emulator raises the error class at all on an already-absent identifier instead of returning the recorded 200/empty success. The choice is deliberate in the code: the handler's module header asserts 'No recording shows this delete against an identifier the account holds nothing under, so it follows the service's prevailing recorded shape' — the corpus contradicts that premise, because this fixture records exactly that call and records it as a 200.

Why: a client relying on AWS's behavior treats cloud-connector deletion as idempotent — teardown and cleanup code, retried deletes whose first attempt already landed, and reconcilers that delete unconditionally all expect 200 and no error. Against the emulator each of those gets a 404 ResourceNotFoundException, so SDK calls raise, retries of an interrupted delete fail where on AWS they succeed, and fixtures/teardown paths abort mid-way leaving the rest of their cleanup unrun. It also makes the emulator's own @idempotent contract unobservable: the second of two identical deletes behaves differently from the first.

Fix: DeleteCloudConnector's handler — services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/delete_cloud_connector.rs, the `if !removed` branch at lines 225-235 that returns DeleteCloudConnectorErrorModeled::ResourceNotFoundException(crate::state::cloud_connector_not_found(&input.identifier)). The absent-identifier case belongs on the success path (return DeleteCloudConnectorOutput, status 200, empty body) rather than the error channel, and the module header's reasoning at lines 24-30 must be corrected to match the recording. This is scoped to the cloud-connector delete only: the sibling deletes named in that header (managed thing, credential locker, OTA task configuration) have their own recordings and must keep whatever their own fixtures record. The shared not-found formatter crate::state::cloud_connector_not_found stays as-is — it is still what a genuine read/update 404 renders.

Related but NOT the same filing: open issue 20261031T225241Z-iot-managed-integrations-updatecloudconnector-not-found-message-uses-a-mutation-specific.md covers step task#3 of this same fixture (UpdateCloudConnector's 404 message wording) and open issue 20261029T193935Z-iot-managed-integrations-createaccountassociation-skips-connector-destination-resolution.md covers refusal bodies elsewhere; neither mentions DeleteCloudConnector or this step, and the replay reports task#4 as owned by no issue, so closing either would leave this step diverging.

Traces: iot-managed-integrations-cloud-connector-error-paths#task#4 (the representative and only member of this cluster, affected_count 1).

## Evidence
Replay of corpus simparity-wire-aws-v1 (trace_set_id simparity-wire-aws-v1), fixture iot-managed-integrations-cloud-connector-error-paths, step task#4, operation DeleteCloudConnector, service iot-managed-integrations, verdict mismatch, affected_count 1, stub_path empty, owned_by empty.

Expected status 200, actual status 404.
Detail, verbatim from the replay:
  status: expected 200, got 404
  $: value_mismatch (expected {}, got {'__type': 'ResourceNotFoundException', 'Message': 'Cloud connector not found.', 'ResourceId': 'nonexistentconnector000001', 'ResourceType': 'CloudConnector'})

Expected body: {} — the modelled Unit output, no members.
Actual body: a ResourceNotFoundException payload. Of its fields, '__type' value ResourceNotFoundException and the whole sentence 'Cloud connector not found.' and 'CloudConnector' are FIXED text for that error class; the ResourceId value is a VALUE taken from the request's Identifier label (the recording's run used the never-created identifier shown above — that specific string is a per-run fixture input, not part of the requirement). On AWS this path emits none of it: there is no error code and no message, only 200 with an empty body.

Model grounding: smithy_query(query="operation", operation="DeleteCloudConnector") on services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json reports http {method: DELETE, uri: /cloud-connectors/{Identifier}, code: 200}, output_shape Unit, idempotent: true, and lists ResourceNotFoundException (httpError 404) among the declared errors.

Code path: src/ops/delete_cloud_connector.rs:210 resolves store.cloud_connector_in(&region, &input.identifier) into `removed`; lines 225-235 then convert `!removed` into the 404 via crate::state::cloud_connector_not_found(&input.identifier). Lines 24-30 of the module header state the (recording-contradicted) assumption that no recording exercises this path.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an iot-managed-integrations client at its data listener, signing for region eu-west-1 (the fixture's region).
2. Replay iot-managed-integrations-cloud-connector-error-paths#task#4: send DELETE /cloud-connectors/{Identifier} with an Identifier that matches CloudConnectorId's @pattern ^[A-Za-z0-9-_]+$ and @length(1..64) but that no CreateCloudConnector in that region ever produced (the fixture uses a never-created identifier).
3. Observe HTTP 404 with x-amzn-errortype/__type ResourceNotFoundException and a body carrying Message 'Cloud connector not found.', ResourceId echoing the submitted Identifier, and ResourceType 'CloudConnector'.
4. Compare with the recorded AWS response for that step: HTTP 200 with an empty body and no error payload.
5. Equivalent second-delete check of the same contract: create a cloud connector, delete it (200), then delete the same Identifier again — AWS answers 200 with an empty body both times (@idempotent); the emulator answers 200 then 404.
