Issue-Source: trace-sweep-divergence
Issue-Trace: groundstation-tag-and-untag-resource-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: groundstation-tag-and-untag-resource-lifecycle groundstation-dataflow-endpoint-group-lifecycle groundstation-contact-read-and-error-paths
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation|TagResource|2xx->2xx|status
Issue-Key: groundstation|UntagResource|2xx->2xx|status
Issue-Key: groundstation|GetDataflowEndpointGroup|2xx->2xx|$.endpointsDetails[*].endpoint.status
Issue-Key: groundstation|GetDataflowEndpointGroup|2xx->2xx|$.endpointsDetails[*].healthReasons[*]
Issue-Key: groundstation|GetDataflowEndpointGroup|2xx->2xx|$.endpointsDetails[*].healthStatus
Issue-Key: groundstation|ListContactVersions|2xx->4xx|$.contactVersionsList
Issue-Key: groundstation|ListContactVersions|2xx->4xx|status
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# Success-path responses diverge: tag and untag status, endpoint-group readiness, and version listing for an unknown contact

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Four net-new wire causes, all in groundstation, all on paths AWS answers SUCCESSFULLY: the emulator returns the wrong HTTP status for an empty-body tagging response, never advances a dataflow endpoint group to its ready state, and refuses a version listing AWS answers with an empty page. They are bundled because they share one service, and each must be closed separately.

Cause 1 of 4: TagResource returns the wrong success status.
AWS: Smithy: TagResource has an EMPTY output shape (TagResourceResponse has no members) and its @http code is DEFAULTED in the model (the model does not state a success status), so the model is silent here — recorded behavior only. The recording shows the successful tagging answered with HTTP 204 and no body, the status AWS uses for this no-content response.
Emulator: answers HTTP 200 for the same successful tagging.
Why: a client that branches on the success status — or an HTTP layer, proxy or contract test that treats 200 as 'a body follows' and 204 as 'no content' — sees the wrong class of success; a 200 with no body is a protocol inconsistency AWS does not produce here.
Fix: the success-status selection for the TagResource handler (services/service-com-amazonaws-groundstation-groundstation/src/ops/tag_resource.rs) / its serializer must emit 204 with no body for the empty output shape.
Traces: groundstation-tag-and-untag-resource-lifecycle#task#3.

Cause 2 of 4: UntagResource returns the wrong success status.
AWS: Smithy: UntagResource is @idempotent with an EMPTY output shape (UntagResourceResponse has no members) and a DEFAULTED @http code, so the model is silent — recorded behavior only. The recording shows the successful untagging answered with HTTP 204 and no body.
Emulator: answers HTTP 200.
Why: same as cause 1 — the tagging pair is the one place a Ground Station client sees a no-content response, and both halves of it report the wrong success status, so status-driven handling and contract checks fail on an otherwise correct operation.
Fix: the success-status selection for the UntagResource handler (src/ops/untag_resource.rs) / its serializer.
Traces: groundstation-tag-and-untag-resource-lifecycle#task#5.

Cause 3 of 4: a dataflow endpoint group never leaves its initializing state, so GetDataflowEndpointGroup reports a permanently unhealthy, uncreated endpoint.
AWS: Smithy: GetDataflowEndpointGroup returns endpointsDetails (EndpointDetailsList); EndpointDetails.healthStatus is a CapabilityHealth enum (HEALTHY | UNHEALTHY) and .healthReasons a CapabilityHealthReasonList over CapabilityHealthReason (NO_REGISTERED_AGENT, INVALID_IP_OWNERSHIP, NOT_AUTHORIZED_TO_CREATE_SLR, UNVERIFIED_IP_OWNERSHIP, INITIALIZING_DATAPLANE, DATAPLANE_FAILURE, HEALTHY); EndpointDetails.endpoint is a DataflowEndpoint whose .status is an EndpointStatus enum (created | creating | deleted | deleting | failed). Both health members are documented as 'ignored when calling CreateDataflowEndpointGroup' — i.e. the SERVICE, not the caller, computes them. Recorded behavior: after the group is created, the read reports endpoint.status 'created', healthStatus 'HEALTHY' and healthReasons ['HEALTHY'] — the transient 'creating'/INITIALIZING_DATAPLANE state resolves to the ready state.
Emulator: the same read reports endpoint.status 'creating', healthStatus 'UNHEALTHY' and healthReasons ['INITIALIZING_DATAPLANE'] — the group is minted in the transient state and never transitions out of it.
Why: 'created' + HEALTHY is the precondition a client waits on before reserving a contact against the group. A waiter polling GetDataflowEndpointGroup for endpoint.status == 'created' or healthStatus == HEALTHY never completes and times out, and any code that refuses to schedule against an UNHEALTHY endpoint refuses forever — the group is unusable even though creation succeeded.
Fix: the endpoint-readiness transition belongs in the dataflow endpoint group's state (src/state.rs) and the read path in src/ops/get_dataflow_endpoint_group.rs: a created group's endpoints must settle to EndpointStatus 'created' with healthStatus HEALTHY and healthReasons [HEALTHY] rather than being pinned at creating/UNHEALTHY/INITIALIZING_DATAPLANE.
Traces: groundstation-dataflow-endpoint-group-lifecycle#task#8.

Cause 4 of 4: ListContactVersions refuses a contactId that names no contact, where AWS answers an empty page.
AWS: Smithy: ListContactVersions is @readonly and @paginated (items contactVersionsList, inputToken/outputToken nextToken, pageSize maxResults); contactVersionsList is a ContactVersionsList, NOT marked @required but the listing's items member. Recorded behavior: for a contactId with no versions to report the service answers HTTP 200 with contactVersionsList: [] (an empty page) and no nextToken, rather than refusing.
Emulator: answers HTTP 434 (ResourceNotFoundException) and no contactVersionsList member at all.
Why: this is the AWS listing convention — a listing yields an empty page instead of a not-found error, and SDK paginators rely on it: a paginator over ListContactVersions raises instead of yielding zero items, so a caller enumerating versions for a set of contact ids aborts on the first id that has none instead of continuing. It also makes the operation inconsistent with the sibling read, DescribeContactVersion, which is the operation that IS modeled to refuse.
Fix: the contact resolution step of the ListContactVersions handler (src/ops/list_contact_versions.rs) must return 200 with an empty contactVersionsList instead of raising the not-found error from the shared contact lookup in src/state.rs.
Traces: groundstation-contact-read-and-error-paths#task#4.

## Evidence
Cause 1 — fixture groundstation-tag-and-untag-resource-lifecycle step task#3, TagResource:
  expected: status 204 (no content)
  actual:   status 200

Cause 2 — fixture groundstation-tag-and-untag-resource-lifecycle step task#5, UntagResource:
  expected: status 204 (no content)
  actual:   status 200

Cause 3 — fixture groundstation-dataflow-endpoint-group-lifecycle step task#8, GetDataflowEndpointGroup (status 200 on both sides):
  $.endpointsDetails[0].endpoint.status: expected 'created', actual 'creating'
  $.endpointsDetails[0].healthStatus:    expected 'HEALTHY',  actual 'UNHEALTHY'
  $.endpointsDetails[0].healthReasons[0]: expected 'HEALTHY', actual 'INITIALIZING_DATAPLANE'
  All six values are members of the modeled enums EndpointStatus / CapabilityHealth / CapabilityHealthReason, so the divergence is which state the group settles in, not a formatting difference. The group id and endpoint names in the fixture are per-run values and are not part of the requirement.

Cause 4 — fixture groundstation-contact-read-and-error-paths step task#4, ListContactVersions:
  expected: status 200 with $.contactVersionsList == [] (empty page)
  actual:   status 434, no $.contactVersionsList member (ResourceNotFoundException)

## Steps to Reproduce
Causes 1-2: create any taggable Ground Station resource (e.g. CreateMissionProfile or CreateDataflowEndpointGroup), call TagResource(resourceArn=<its arn>, tags={'k':'v'}) and record the HTTP status, then UntagResource(resourceArn=<same>, tagKeys=['k']) and record the HTTP status. Both must be 204 with no body; both are 200 today. ListTagsForResource confirms the tag was actually applied and removed, so only the status diverges.

Cause 3: CreateDataflowEndpointGroup with a complete endpointDetails entry (endpoint + securityDetails), then GetDataflowEndpointGroup on the returned id and read endpointsDetails[0].endpoint.status, endpointsDetails[0].healthStatus and endpointsDetails[0].healthReasons. Poll it: AWS settles at created/HEALTHY/[HEALTHY]; the emulator stays at creating/UNHEALTHY/[INITIALIZING_DATAPLANE] indefinitely.

Cause 4: call ListContactVersions(contactId=<a well-formed uuid that names no contact>) and read the HTTP status and contactVersionsList; AWS answers 200 with an empty list. Contrast DescribeContactVersion on the same id, which is the operation that does refuse.
