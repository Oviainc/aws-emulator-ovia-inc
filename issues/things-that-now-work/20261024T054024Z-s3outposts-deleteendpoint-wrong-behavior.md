Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# DeleteEndpoint removes the endpoint synchronously, so the modelled Deleting status is never reported

_Filed: 2026-10-24T05:40:24Z_
_Operation: DeleteEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: DeleteEndpoint is asynchronous — the model's own documentation says "Deletes an endpoint. It can take up to 5 minutes for this action to finish." and `EndpointStatus` enumerates `Deleting` (and `Delete_Failed`) precisely because an endpoint under deletion is still reported by the endpoint listings, with `Status: "Deleting"`, until the teardown completes and the record disappears.

Actual: the handler calls `store.remove_endpoint(&key)` while holding the lock and answers 200, so the record is gone the instant the call returns. A client that deletes and then calls ListEndpoints / ListSharedEndpoints never sees `Deleting`; it sees the endpoint already absent. Consequences a client can observe: (a) `Status: "Deleting"` is unreachable in this emulator even though it is a modelled enum value, so a poll loop that waits for an endpoint to leave `Deleting` has nothing to observe; (b) a second DeleteEndpoint issued inside the real service's up-to-5-minute window answers 404 ResourceNotFoundException here, where the real service still has the record; (c) the CIDR the endpoint committed and the subnet→VPC mapping are released immediately, so a create issued right after a delete can be handed a range the deleting endpoint still holds in the real service.

The service already has the machinery for this: CreateEndpoint stores a `PendingTransition { due, status }` so a new endpoint reports `Pending` until the clock owes it `Available`, and the listings materialise it with `settle_all(now)`. The delete side is simply missing the symmetric transition (mark `Deleting` with a due instant at which the record is removed by the next caller that reports on it).

## Evidence

Spec: smithy_query(query="doc", shape="DeleteEndpoint") — "Deletes an endpoint. It can take up to 5 minutes for this action to finish."; smithy_query(query="shape", shape="EndpointStatus") — values Pending | Available | Deleting | Create_Failed | Delete_Failed; `Endpoint$FailedReason` is documented as "the failure reason, if any, for a create or delete endpoint operation".
Handler: services/service-com-amazonaws-s3outposts-s3outposts/src/ops/delete_endpoint.rs:208 `store.remove_endpoint(&key);` then `Ok(DeleteEndpointOutput)` — no status change, no deferred removal.
State: services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:130-134 (`PendingTransition`), :235-244 (`EndpointRecord::settle`), :425-429 (`settle_all`) — the deferred-transition machinery exists and is used only for Pending→Available; src/state.rs:267-273 states no path reaches `Delete_Failed`/`Deleting`.
Tests: tests/all/endpoint_lifecycle.rs (~lines 870-895) asserts the deleted endpoint is absent from the very next ListEndpoints, i.e. the current synchronous behaviour is enshrined; no test asserts a `Deleting` status, so nothing covers the gap.

## Steps to Reproduce

1. Start the emulator with an Outpost provisioned (S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-0a1b2c3d4e5f60718:52776558133248")
2. CreateEndpoint(OutpostId="op-0a1b2c3d4e5f60718", SubnetId="subnet-0a1b2c3d4e5f60718", SecurityGroupId="sg-0a1b2c3d4e5f60718") and note the EndpointId in the returned EndpointArn
3. Wait for ListEndpoints to report Status="Available" for it
4. DeleteEndpoint(OutpostId=..., EndpointId=...) → 200
5. ListEndpoints immediately afterwards: expected one endpoint with Status="Deleting" (the real service keeps reporting it for up to 5 minutes); observed an empty Endpoints list
6. DeleteEndpoint again immediately: expected the still-deleting endpoint to be accepted/reported as deleting; observed 404 ResourceNotFoundException
