Issue-Source: trace-sweep-divergence
Issue-Trace: gameliftstreams-stream-group-application-association-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gameliftstreams-stream-group-application-association-lifecycle
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.ActiveCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.DesiredCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.IdleCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.LocationStates[*].AlwaysOnCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.LocationStates[*].InternalVpcIpv4CidrBlock
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.LocationStates[*].OnDemandCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.LocationStates[*].RequestedCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.LocationStates[*].TargetIdleCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.AssociatedApplications[*]
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.DefaultApplication
Issue-Key: gameliftstreams|TagResource|2xx->2xx|status
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# gameliftstreams stream-group reads omit capacity and application members, and TagResource answers 200 instead of 204

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStreamGroup

## Description
Three net-new causes on the stream-group lifecycle: GetStreamGroup serves a response that drops the group's capacity projection and (after an association) its application members, and TagResource answers the wrong success status. All three are success-path response-fidelity faults in the same service and must all be closed before this issue is submitted.

Cause 1 of 3: GetStreamGroup omits the capacity projection of the group and of each location state.
AWS: Smithy model (services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json) — GetStreamGroupOutput.LocationStates targets LocationStates, a list of LocationState, whose members include AlwaysOnCapacity, OnDemandCapacity, TargetIdleCapacity, IdleCapacity, RequestedCapacity (all integer @range(min: 0)) and InternalVpcIpv4CidrBlock (Ipv4CidrBlock, documented as "the CIDR block of the service VPC for this location"). A read of a group created with always-on capacity reports the requested capacity it was created with on the matching location, plus the allocated/idle/on-demand counters and the location's service-VPC CIDR. The group-level roll-up members the recording also shows (ActiveCapacity, DesiredCapacity, IdleCapacity at the top level of the GetStreamGroup body) are recorded behavior only — they are not present in the on-disk model's GetStreamGroupOutput, but the recording shows AWS serving them on every stream-group read, with DesiredCapacity equal to the capacity requested at creation and the active/idle counters present (0 when nothing is streaming). Requirement: the members are present with the group's own values; the specific numbers in any one recording are that run's values, not part of the contract.
Emulator: serves 200 but the body has no ActiveCapacity, DesiredCapacity or IdleCapacity at all, and its single LocationStates entry has no AlwaysOnCapacity, OnDemandCapacity, TargetIdleCapacity, IdleCapacity or InternalVpcIpv4CidrBlock, while RequestedCapacity is 0 instead of the capacity the group was created with — i.e. the created capacity is never stored or never projected.
Why: capacity is the whole observable state of a stream group. A client polling GetStreamGroup to decide whether a location has provisioned enough always-on capacity before starting a session sees no capacity members and a RequestedCapacity of 0, so it concludes the group was created empty and either waits forever or scales it again; anything routing traffic by the location's service-VPC CIDR has no CIDR to route to. Absent integer members also differ from zero-valued ones for SDK consumers.
Fix: the GetStreamGroup handler's output projection and the stream-group state it reads from — the creation path must persist the requested per-location capacity, and the serializer must emit the location-state capacity members and the group-level capacity roll-up.
Traces: gameliftstreams-stream-group-application-association-lifecycle#task#6 (also task#16 in the same cluster).

Cause 2 of 3: after AssociateApplications, GetStreamGroup reports neither the associated applications nor the default application.
AWS: Smithy model — GetStreamGroupOutput.AssociatedApplications targets ArnList and is documented as "a set of applications that this stream group is associated to"; GetStreamGroupOutput.DefaultApplication targets DefaultApplication (Arn + Id) and is the application associated with the group. AssociateApplications returns the group Arn plus ApplicationArns, and a subsequent read of the group lists each associated application ARN in AssociatedApplications and reports the group's default application as {Arn, Id}. The values are the application's own ARN and id from the request/state — not any literal from one recording.
Emulator: serves 200 with AssociatedApplications[0] missing and DefaultApplication missing entirely (on top of the same capacity omissions as cause 1), so the association the preceding step performed is invisible on the read.
Why: the association is a precondition for streaming an application through a group. A client that associates an application and then reads the group back to confirm before calling StartStreamSession sees an unassociated group and never proceeds; tooling that reconciles desired vs actual associations will re-associate endlessly, and anything resolving the group's default application has nothing to resolve.
Fix: the AssociateApplications handler's state write and/or the GetStreamGroup output projection in the same component as cause 1 — the association and the resulting default application must be persisted on the group and serialized on reads.
Traces: gameliftstreams-stream-group-application-association-lifecycle#task#8.

Cause 3 of 3: TagResource answers HTTP 200 where AWS answers 204.
AWS: TagResource is POST /tags/{ResourceArn} with an empty output structure (TagResourceResponse has no members). The on-disk model carries no explicit @http code for this operation (the digest reports code 200 with codeDefaulted: true), so the model says nothing authoritative here; recorded behavior: AWS answers HTTP 204 with an empty body for a successful tag write on a gameliftstreams resource.
Emulator: answers HTTP 200 for the same successful TagResource call.
Why: 204 vs 200 is the difference between "no content" and "a body follows". Clients, proxies and gateways that branch on the success status — or that attempt to read/parse a body on 200 — behave differently against the emulator than against AWS, and any contract test asserting the documented status fails against a correct client.
Fix: the TagResource route/handler's success-status selection for this service's empty-output operation (it currently falls back to the protocol default instead of the service's 204).
Traces: gameliftstreams-stream-group-application-association-lifecycle#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 (fixture gameliftstreams-stream-group-application-association-lifecycle):

- task#6 GetStreamGroup (cluster also covers task#16), status 200 vs 200: $.ActiveCapacity missing (expected 0), $.DesiredCapacity missing (expected 2), $.IdleCapacity missing (expected 0), $.LocationStates[0].AlwaysOnCapacity missing (expected 2), $.LocationStates[0].InternalVpcIpv4CidrBlock missing (expected '10.0.0.0/16'), $.LocationStates[0].OnDemandCapacity missing (expected 0), $.LocationStates[0].TargetIdleCapacity missing (expected 0), $.LocationStates[0].RequestedCapacity value_mismatch (expected 2, got 0).
- task#8 GetStreamGroup, status 200 vs 200: the same eight capacity divergences PLUS $.AssociatedApplications[0] missing (expected the associated application ARN) and $.DefaultApplication missing (expected {Arn, Id} of that application).
- task#11 TagResource: status expected 204, got 200.

Model evidence: LocationState members AlwaysOnCapacity / OnDemandCapacity / TargetIdleCapacity / IdleCapacity / RequestedCapacity / InternalVpcIpv4CidrBlock; GetStreamGroupOutput members AssociatedApplications (ArnList) and DefaultApplication; TagResource output TagResourceResponse is empty and its @http code is defaulted in the on-disk model (recorded AWS status 204).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture gameliftstreams-stream-group-application-association-lifecycle.
3. Step task#6 (and task#16): GET /streamgroups/{Identifier} for a group created with always-on capacity on one location — observe a 200 body with no ActiveCapacity/DesiredCapacity/IdleCapacity, a LocationStates entry missing AlwaysOnCapacity/OnDemandCapacity/TargetIdleCapacity/IdleCapacity/InternalVpcIpv4CidrBlock, and RequestedCapacity 0 instead of the requested capacity.
4. Step task#8: after POST /streamgroups/{Identifier}/associations succeeds, GET /streamgroups/{Identifier} again — observe AssociatedApplications and DefaultApplication absent from the 200 body.
5. Step task#11: POST /tags/{ResourceArn} with a valid tag map on the stream group — observe HTTP 200 where AWS answers 204.
