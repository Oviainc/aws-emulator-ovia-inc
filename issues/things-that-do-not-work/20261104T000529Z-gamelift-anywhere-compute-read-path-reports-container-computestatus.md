Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-compute-registration-and-access
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-compute-registration-and-access gamelift-multi-compute-isolation
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|RegisterCompute|2xx->2xx|$.Compute.ComputeStatus
Issue-Key: gamelift|RegisterCompute|2xx->2xx|$.Compute.GameLiftAgentEndpoint
Issue-Key: gamelift|DescribeCompute|2xx->2xx|$.Compute.ComputeStatus
Issue-Key: gamelift|ListCompute|2xx->2xx|$.ComputeList[*]
Issue-Commit: e7422c982dfb77c1099e81b60a45c65279a5e5d9

# GameLift Anywhere compute read path reports container ComputeStatus casing and omits GameLiftAgentEndpoint

> _Note: this issue was identified against the code base at commit `e7422c982dfb77c1099e81b60a45c65279a5e5d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterCompute

## Description
Four causes, all in how the emulator builds the `Compute` structure for a compute registered into an Anywhere fleet. Every operation that serves a `Compute` (RegisterCompute, DescribeCompute, ListCompute) reproduces the same two field defects, so the fix belongs in the shared Compute projection rather than in each handler.

Cause 1 of 4: RegisterCompute (fixture gamelift-compute-registration-and-access, step task#3).
AWS: Smithy: RegisterCompute output -> Compute; Compute.ComputeStatus targets enum ComputeStatus (PENDING | ACTIVE | TERMINATING | IMPAIRED), whose own documentation states "While the ComputeStatus enum type is valid for Container based servers, the result may also include other non-enumerated string values such as \"Active\" for fleets which are not Container-based." The recording is an Anywhere (custom-location) fleet, and AWS serves the non-enumerated string form `Active` for it. AWS also populates Compute.GameLiftAgentEndpoint (Smithy: Compute.GameLiftAgentEndpoint -> GameLiftAgentEndpointOutput, @length 1..128, "The endpoint of the Amazon GameLift Servers Agent") for a registered Anywhere compute; the recorded value is the regional agent websocket endpoint of the form wss://<region>.process-manager-api.amazongamelift.com (region taken from the request's endpoint/fleet region, not a literal).
Emulator: returns ComputeStatus `ACTIVE` (the container-fleet enum casing) and omits GameLiftAgentEndpoint entirely (field absent).
Why: a client that gates "is this compute usable" on the documented Anywhere value `Active` sees an unrecognized status and never proceeds; an agent/bootstrap client reads GameLiftAgentEndpoint to open its process-manager websocket and gets null, so it cannot connect at all. Both are silent — HTTP 200 with a well-formed body.
Fix: the Compute projection used by the RegisterCompute handler's serializer (status rendering must depend on fleet compute type: `Active` for non-container/Anywhere fleets, the uppercase enum for container fleets) plus population of GameLiftAgentEndpoint from the resolved region.

Cause 2 of 4: DescribeCompute 200 read-back (fixture gamelift-compute-registration-and-access, step task#4; same cluster also covers gamelift-multi-compute-isolation task#6, task#7, task#12).
AWS: Smithy: DescribeCompute output -> Compute; same ComputeStatus rule as Cause 1. Recorded response carries ComputeStatus `Active`.
Emulator: serves `ACTIVE`.
Why: the read-back path disagrees with AWS for the same reason as Cause 1, so polling loops that wait for `Active` never terminate.
Fix: DescribeCompute handler's Compute serializer (shared projection).

Cause 3 of 4: ListCompute single-compute listing (fixture gamelift-compute-registration-and-access, step task#5).
AWS: Smithy: ListCompute output ComputeList -> list of Compute. Recorded entry carries ComputeStatus `Active`.
Emulator: the listed entry carries `ComputeStatus: ACTIVE`; the remaining members (FleetId, FleetArn, ComputeName, ComputeArn, IpAddress, Location, GameLiftServiceSdkEndpoint) match.
Why: list consumers filter/display on status and mis-handle the unexpected value.
Fix: ListCompute handler's Compute list serializer (shared projection).

Cause 4 of 4: ListCompute multi-compute listing (fixture gamelift-multi-compute-isolation, step task#5).
AWS: as Cause 3; every element of ComputeList carries the Anywhere status string `Active`.
Emulator: both listed computes carry `ACTIVE`.
Why: same as Cause 3, across all elements of a page.
Fix: same shared Compute projection; a single correction closes causes 2-4 once the status rendering is fleet-type aware.

Note: only the status/endpoint fields diverge — ARNs, names, IPs, locations and the SDK endpoint already match. Per-run values in the recordings (fleet ids, compute names, CreationTime) are environment-specific and are NOT requirements. An existing open issue, 20261029T183132Z-gamelift-responses-omit-modeled-fields-matchmaking-defaults-terminationtime.md, covers omitted modeled fields on other GameLift responses (matchmaking defaults, TerminationTime, managed locations); it does not cover the Compute structure or these steps.

## Evidence
Replay (trace set simparity-wire-aws-v1), all four clusters HTTP 200 -> 200, bodies diverge:
- RegisterCompute, gamelift-compute-registration-and-access#task#3: $.Compute.ComputeStatus value_mismatch (expected 'Active', got 'ACTIVE'); $.Compute.GameLiftAgentEndpoint missing (expected 'wss://us-west-2.process-manager-api.amazongamelift.com', got None). Cluster also affects gamelift-multi-compute-isolation#task#3, #task#4.
- DescribeCompute, gamelift-compute-registration-and-access#task#4: $.Compute.ComputeStatus value_mismatch (expected 'Active', got 'ACTIVE'). Cluster also affects gamelift-multi-compute-isolation#task#6, #task#7, #task#12.
- ListCompute, gamelift-compute-registration-and-access#task#5: $.ComputeList[0] mismatch, differing only in ComputeStatus ('Active' expected vs 'ACTIVE' served).
- ListCompute, gamelift-multi-compute-isolation#task#5: $.ComputeList[0] and $.ComputeList[1] mismatch, each differing only in ComputeStatus ('Active' vs 'ACTIVE').
Model evidence: Compute.ComputeStatus doc note on non-enumerated 'Active' for non-container fleets; Compute.GameLiftAgentEndpoint is a modeled output member.

## Steps to Reproduce
1. Create a custom location and an Anywhere fleet in it (as fixture gamelift-compute-registration-and-access does).
2. RegisterCompute with FleetId, ComputeName, IpAddress and the custom Location -> inspect $.Compute.ComputeStatus and $.Compute.GameLiftAgentEndpoint.
3. DescribeCompute for the same FleetId/ComputeName -> inspect $.Compute.ComputeStatus.
4. ListCompute for the fleet (one compute, then two as in gamelift-multi-compute-isolation) -> inspect ComputeStatus on each ComputeList element.
5. Replay fixtures gamelift-compute-registration-and-access (task#3, task#4, task#5) and gamelift-multi-compute-isolation (task#5).
