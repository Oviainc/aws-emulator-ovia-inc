Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-compute-registration-and-access
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-compute-registration-and-access gamelift-multi-compute-isolation
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|RegisterCompute|2xx->2xx|$.Compute.GameLiftServiceSdkEndpoint
Issue-Key: gamelift|DescribeCompute|2xx->2xx|$.Compute.GameLiftServiceSdkEndpoint
Issue-Key: gamelift|ListCompute|2xx->2xx|$.ComputeList[*].GameLiftServiceSdkEndpoint
Issue-Related: 20261104T000529Z-gamelift-anywhere-compute-read-path-reports-container-computestatus.md
Issue-Commit: 3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8

# GameLift Compute projection serves uppercase ComputeStatus, omits GameLiftAgentEndpoint and advertises a non-local SDK endpoint

> _Note: this issue was identified against the code base at commit `3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterCompute

## Description
Five causes, all in the single `Compute` structure projection that every GameLift operation serving a compute (RegisterCompute, DescribeCompute, ListCompute) shares. All five reproduce with HTTP 200 -> 200: the status line and envelope are right and only the body fields diverge, so every one of them is silent to a client.

Related open issue: 20261104T000529Z-gamelift-anywhere-compute-read-path-reports-container-computestatus.md describes the ComputeStatus/GameLiftAgentEndpoint pair, but the replay reports these four clusters as UNOWNED (no issue carries their replay key: that issue's Issue-Trace-Ids list bare fixture ids with no `#step` half, so its replay pointer does not bind these steps), and it explicitly asserts the SDK endpoint 'already match[es]', so it does not cover Cause 5 at all. This issue re-files the clusters with a binding pointer. 20261029T183132Z-gamelift-responses-omit-modeled-fields-matchmaking-defaults-terminationtime.md covers omitted modeled fields on other GameLift responses (matchmaking defaults, TerminationTime, managed locations) and does not touch the Compute structure.

Cause 1 of 5: RegisterCompute serves the container-fleet enum casing for an Anywhere compute (fixture gamelift-compute-registration-and-access, step task#3; cluster also affects gamelift-multi-compute-isolation#task#3 and #task#4).
AWS: Smithy: RegisterCompute output -> Compute; Compute.ComputeStatus targets enum ComputeStatus (PENDING | ACTIVE | TERMINATING | IMPAIRED), whose member documentation states "While the ComputeStatus enum type is valid for Container based servers, the result may also include other non-enumerated string values such as \"Active\" for fleets which are not Container-based." These fixtures register computes into an Anywhere (custom-location) fleet, i.e. a non-container fleet, and the recording shows AWS serving the non-enumerated mixed-case string `Active` for it. The rule, not the recorded literal, is the requirement: status rendering depends on the fleet's compute type - `Active` for non-container/Anywhere fleets, the uppercase enum constant for container fleets.
Emulator: serves $.Compute.ComputeStatus = 'ACTIVE' unconditionally.
Why: a client that gates "is this compute usable" on the documented Anywhere value `Active` sees an unrecognized status and never proceeds; SDK enum deserializers that map the string to an enum constant classify it differently from AWS. Nothing in the response signals the problem.
Fix: the shared Compute projection behind the RegisterCompute handler's serializer - make the status rendering fleet-compute-type aware rather than emitting the enum constant.

Cause 2 of 5: RegisterCompute omits the modeled agent endpoint (same fixture gamelift-compute-registration-and-access, step task#3).
AWS: Smithy: Compute.GameLiftAgentEndpoint -> GameLiftAgentEndpointOutput, @length 1..128, documented as "The endpoint of the Amazon GameLift Servers Agent." AWS populates it on a compute registered into an Anywhere fleet; the recording shows the regional agent websocket endpoint of the form wss://<region>.process-manager-api.amazongamelift.com, where <region> is the region resolved for the request/fleet (the recorded `us-west-2` is that run's value, not part of the requirement).
Emulator: the member is absent from the response body entirely (got None).
Why: an agent/bootstrap client reads GameLiftAgentEndpoint to open its process-manager websocket; with the field null it has no address to dial and cannot bring the compute up, while the call itself reports success.
Fix: populate GameLiftAgentEndpoint from the resolved region in the same Compute projection/serializer.

Cause 3 of 5: DescribeCompute read-back repeats the status casing (fixture gamelift-compute-registration-and-access, step task#4; cluster also affects gamelift-multi-compute-isolation#task#6, #task#7, #task#12).
AWS: Smithy: DescribeCompute output -> Compute, same ComputeStatus rule as Cause 1; the recorded read-back of an Anywhere compute carries `Active`.
Emulator: serves 'ACTIVE'.
Why: the read-back is the path a client polls, so a wait-for-`Active` loop never terminates and eventually times out against an emulator whose compute is in fact ready.
Fix: same shared Compute projection, reached through the DescribeCompute handler's serializer.

Cause 4 of 5: ListCompute elements repeat the status casing, for one and for several computes (fixture gamelift-compute-registration-and-access, step task#5, single-element page; and fixture gamelift-multi-compute-isolation, step task#5, two-element page).
AWS: Smithy: ListCompute output ComputeList -> list<Compute>; every element follows the Cause 1 rule, so each listed Anywhere compute carries `Active`.
Emulator: every element carries ComputeStatus 'ACTIVE'. All other listed members (FleetId, FleetArn, ComputeName, ComputeArn, IpAddress, Location) match, and the element-level diff the replay prints is driven only by this field plus CreationTime, which is a per-run timestamp and is NOT a requirement.
Why: list consumers filter and display on status, and ListCompute also accepts a ComputeStatus filter, so a catalogue built from this page disagrees with AWS for every compute in it.
Fix: same shared Compute projection, applied per element in the ListCompute list serializer; one fleet-type-aware status rendering closes causes 1, 3 and 4 together.

Cause 5 of 5: GameLiftServiceSdkEndpoint advertises a host the emulator does not serve (all four clusters above: RegisterCompute task#3, DescribeCompute task#4, ListCompute task#5, ListCompute gamelift-multi-compute-isolation#task#5).
AWS: Smithy: Compute.GameLiftServiceSdkEndpoint -> GameLiftServiceSdkEndpointOutput, @length 1..128, documented as the endpoint "the game servers on the compute use ... to connect to" the GameLift service. Recorded behavior only for the host part: a real account is told the real regional websocket host, and the requirement is that the value is an endpoint the caller can actually reach for this deployment.
Emulator: emits the literal upstream AWS host (verdict host_not_own: the served value is a websocket URL pointing at the real amazongamelift.com regional endpoint rather than at the address on which the emulator is reached).
Why: a game server that takes this endpoint from the response dials out of the emulated environment instead of back to the emulator, so the SDK handshake either fails to resolve/connect or escapes to the real service - the one field whose entire purpose is to be dialed is the one field pointing somewhere the client cannot use.
Fix: the Compute projection must derive the SDK (and agent) endpoint from the emulator's own resolved service endpoint/listener rather than hard-coding the upstream AWS host; the scheme/shape (wss://<regional-host>) stays as modeled.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout; four unowned clusters (stub_path empty, owned_by empty), all HTTP 200 expected -> 200 actual, bodies diverge:

1. RegisterCompute - gamelift-compute-registration-and-access#task#3 (affected: gamelift-multi-compute-isolation#task#3, #task#4):
   $.Compute.ComputeStatus: value_mismatch (expected 'Active', got 'ACTIVE')
   $.Compute.GameLiftAgentEndpoint: missing (expected 'wss://us-west-2.process-manager-api.amazongamelift.com', got None)
   $.Compute.GameLiftServiceSdkEndpoint: host_not_own ('wss://us-west-2.api.amazongamelift.com' served; not an address the emulator serves)

2. DescribeCompute - gamelift-compute-registration-and-access#task#4 (affected: gamelift-multi-compute-isolation#task#6, #task#7, #task#12):
   $.Compute.ComputeStatus: value_mismatch (expected 'Active', got 'ACTIVE')
   $.Compute.GameLiftServiceSdkEndpoint: host_not_own

3. ListCompute - gamelift-compute-registration-and-access#task#5:
   $.ComputeList[0]: mismatch; expected element carries 'ComputeStatus': 'Active', served element carries 'ComputeStatus': 'ACTIVE'. Every other member (ComputeArn, ComputeName, FleetArn, FleetId, IpAddress, Location) matches; CreationTime differs only as a per-run timestamp and is not a requirement.
   $.ComputeList[0].GameLiftServiceSdkEndpoint: host_not_own

4. ListCompute - gamelift-multi-compute-isolation#task#5:
   $.ComputeList[0] and $.ComputeList[1]: mismatch, each differing only in ComputeStatus ('Active' expected vs 'ACTIVE' served) plus the per-run CreationTime.
   $.ComputeList[0].GameLiftServiceSdkEndpoint, $.ComputeList[1].GameLiftServiceSdkEndpoint: host_not_own

Model evidence (Smithy com.amazonaws.gamelift#GameLift):
- enum ComputeStatus = [PENDING, ACTIVE, TERMINATING, IMPAIRED]; Compute.ComputeStatus doc note: non-enumerated values such as "Active" are returned for fleets that are not container-based.
- Compute.GameLiftAgentEndpoint -> GameLiftAgentEndpointOutput @length 1..128 ("The endpoint of the Amazon GameLift Servers Agent") - a modeled output member the emulator never populates.
- Compute.GameLiftServiceSdkEndpoint -> GameLiftServiceSdkEndpointOutput @length 1..128 - the endpoint game servers connect to.
- RegisterCompute / DescribeCompute output -> Compute; ListCompute output ComputeList -> list<Compute>, so one projection feeds all three.

## Steps to Reproduce
1. Create a custom location and an Anywhere fleet in that location (as fixture gamelift-compute-registration-and-access does in its earlier steps).
2. RegisterCompute with FleetId, ComputeName, IpAddress and the custom Location. Inspect $.Compute.ComputeStatus (emulator: 'ACTIVE'; AWS for a non-container fleet: 'Active'), $.Compute.GameLiftAgentEndpoint (emulator: absent) and $.Compute.GameLiftServiceSdkEndpoint (emulator: upstream amazongamelift.com host, not the emulator's own).
3. DescribeCompute for the same FleetId/ComputeName and inspect $.Compute.ComputeStatus and $.Compute.GameLiftServiceSdkEndpoint.
4. ListCompute for the fleet and inspect ComputeStatus and GameLiftServiceSdkEndpoint on each ComputeList element; repeat with two registered computes as gamelift-multi-compute-isolation does.
5. Re-replay to confirm: gamelift-compute-registration-and-access steps task#3, task#4, task#5 and gamelift-multi-compute-isolation step task#5 (plus the clustered steps gamelift-multi-compute-isolation task#3, task#4, task#6, task#7, task#12).
