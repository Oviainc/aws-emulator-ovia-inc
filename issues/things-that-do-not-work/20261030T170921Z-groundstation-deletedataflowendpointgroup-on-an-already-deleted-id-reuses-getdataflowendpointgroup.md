Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: groundstation.dataflow-endpoint-group-lifecycle
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation.dataflow-endpoint-group-lifecycle|run#35|AssertionError|assert refused == (
Issue-Commit: 5e13030fba3716cb0d2a1354c811e28788b6a350

# DeleteDataflowEndpointGroup on an already-deleted id reuses GetDataflowEndpointGroup's not-found message instead of its own delete-specific template

> _Note: this issue was identified against the code base at commit `5e13030fba3716cb0d2a1354c811e28788b6a350`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDataflowEndpointGroup

## Description
One net-new root cause in groundstation.

Cause 1 of 1: a SECOND DeleteDataflowEndpointGroup on a dataflowEndpointGroupId that was already deleted answers the correct error code but with the READ path's not-found wording, not the wording Ground Station reserves for the delete.

AWS: Smithy (services/service-com-amazonaws-groundstation-groundstation/smithy-model/com-amazonaws-groundstation-groundstation.json): DeleteDataflowEndpointGroup.dataflowEndpointGroupId is an @required @httpLabel Uuid (@length 36..36, pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$); the operation is @idempotent; modeled errors are InvalidParameterException (HTTP 431), ResourceNotFoundException (HTTP 434) and DependencyException (HTTP 531). GetDataflowEndpointGroup carries the identical input shape and the identical error set. Recorded behavior distinguishes the two operations' not-found MESSAGES even though both answer ResourceNotFoundException (434): GetDataflowEndpointGroup answers the fixed text `The specified group does not exist.` (no value interpolated at all), while DeleteDataflowEndpointGroup answers `No Dataflow Endpoint Group found with id {dataflowEndpointGroupId}` — here `No Dataflow Endpoint Group found with id ` is the fixed text and the trailing component is a value taken from the request, namely the dataflowEndpointGroupId that was asked for. The two templates are different, and the delete's is the one that names the offending id.

Emulator: DeleteDataflowEndpointGroup on a dead id returns ResourceNotFoundException with the read path's template, `The specified group does not exist.` — src/ops/delete_dataflow_endpoint_group.rs:153-159 builds the error from the shared literal `crate::state::DATAFLOW_ENDPOINT_GROUP_NOT_FOUND`, the same constant the get path uses, and its own comment states the intent explicitly ("it does not disclose the id that was asked for"). That constant is returned both from the pre-check at lines 161-166 and from the post-removal `ok_or_else(not_found)` at lines 198-200, so every not-found exit of the delete carries the read's wording. The emulator is correct on the code and the status here; only the message text diverges.

Why: the message is part of the contract a client reports and an operator greps. Because the emulator emits one identical sentence for both operations, a caller can no longer tell a failed READ from a failed MUTATION by message — exactly the distinction AWS preserves by giving the delete its own template. Any assertion, log-grep or user-facing error mapping keyed on AWS's delete sentence never matches, and the diagnostic value of AWS's delete message is lost outright: AWS names the dataflowEndpointGroupId that failed to resolve, the emulator names nothing, so an operator reconciling a batch of deletes cannot tell WHICH id was already gone.

Fix: give DeleteDataflowEndpointGroup its own not-found message template in its handler (src/ops/delete_dataflow_endpoint_group.rs), or a delete-specific helper alongside the DATAFLOW_ENDPOINT_GROUP_NOT_FOUND constant in src/state.rs, interpolating the requested dataflowEndpointGroupId. Both not-found exits of that handler must use it. Leave GetDataflowEndpointGroup's existing wording alone — this same check asserts it and it passes today.

Note: this is NOT covered by the open issue 20261029T182737Z-groundstation-four-ground-station-refusals-carry-invented-message-text.md. That issue's Cause 4 is the structurally identical defect on DeleteConfig vs GetConfig, and its Cause 2 is the securityDetails message in this very check — whose assertion the check now reaches past, failing instead at the later re-delete assertion (line 139) that the issue explicitly recorded as passing. Nor is it covered by 20261029T202218Z-groundstation-unknown-identifier-and-missing-parameter-refusals-answer-resourcenotfoundexception.md, which concerns agent- and satellite-id refusals answering the wrong error CLASS, not dataflow-endpoint-group message text.

Traces: groundstation.dataflow-endpoint-group-lifecycle.

## Evidence
Cause 1 — check groundstation.dataflow-endpoint-group-lifecycle, at the re-delete assertion (check_groundstation_dataflow_endpoint_group_lifecycle.py line 139):
  expected: ("ResourceNotFoundException", "No Dataflow Endpoint Group found with id {dataflowEndpointGroupId}")
  actual:   ("ResourceNotFoundException", "The specified group does not exist.")

Harness assertion text: `AssertionError: expected ResourceNotFoundException on re-delete of f2edf291-9ce1-403f-bd3f-058071a615ef, got ('ResourceNotFoundException', 'The specified group does not exist.')`

Message anatomy: in the EXPECTED message, `No Dataflow Endpoint Group found with id ` is fixed text and the trailing component is the dataflowEndpointGroupId the request supplied. The uuid f2edf291-9ce1-403f-bd3f-058071a615ef is the per-run group id this particular replay minted — it FILLS the template and is not part of it, so a fix must interpolate whatever id the request carries rather than any literal value. The ACTUAL message is entirely fixed text and interpolates nothing.

The error code (ResourceNotFoundException) and the HTTP status (434) are correct on this path; the divergence is the template's fixed text.

The same check confirms the surrounding behavior is right, so the defect is narrowly the delete's not-found wording:
  - GetDataflowEndpointGroup after delete correctly answers ("ResourceNotFoundException", "The specified group does not exist.") — passes.
  - The create/get round-trip, including the persisted address {"name": "10.0.0.5", "port": 55888} and the endpoint name, passes.
  - The securityDetails-omitted refusal earlier in the check now passes, which is how execution reaches line 139.

Source: services/service-com-amazonaws-groundstation-groundstation/src/ops/delete_dataflow_endpoint_group.rs:153-159 constructs the ResourceNotFoundException from `crate::state::DATAFLOW_ENDPOINT_GROUP_NOT_FOUND`, the constant shared with the get path; it is returned at lines 161-166 (presence pre-check) and lines 198-200 (post-removal ok_or_else).

## Steps to Reproduce
1. Create the network the endpoint group's securityDetails reference: an EC2 VPC (CidrBlock 10.0.0.0/16), a subnet in an available availability zone (CidrBlock 10.0.0.0/24), and a security group in that VPC; plus an IAM role trusted by groundstation.amazonaws.com.
2. Call CreateDataflowEndpointGroup with endpointDetails=[{"endpoint": {"name": <name>, "address": {"name": "10.0.0.5", "port": 55888}}, "securityDetails": {"subnetIds": [<subnet>], "securityGroupIds": [<sg>], "roleArn": <role arn>}}] and keep the returned dataflowEndpointGroupId.
3. Call DeleteDataflowEndpointGroup(dataflowEndpointGroupId=<that id>) once — it succeeds.
4. Call DeleteDataflowEndpointGroup(dataflowEndpointGroupId=<the same, now-deleted id>) again and read Error.Code and Error.Message.
   AWS: ResourceNotFoundException with `No Dataflow Endpoint Group found with id <that id>`.
   Emulator: ResourceNotFoundException with `The specified group does not exist.`
5. For contrast, call GetDataflowEndpointGroup on the same dead id and read Error.Message: it is `The specified group does not exist.` on both AWS and the emulator — i.e. the emulator serves one message where AWS serves two distinct ones.
