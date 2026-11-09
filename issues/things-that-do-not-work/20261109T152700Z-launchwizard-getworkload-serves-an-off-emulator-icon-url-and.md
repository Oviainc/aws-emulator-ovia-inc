Issue-Source: trace-sweep-divergence
Issue-Trace: launch-wizard-workload-catalog-discovery
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: launch-wizard-workload-catalog-discovery
Issue-Shape-Id: com.amazonaws.launchwizard#LaunchWizard
Issue-Key: launchwizard|GetWorkload|2xx->2xx|$.workload.iconUrl
Issue-Related: 20261029T234833Z-launchwizard-seeded-workload-catalog-is-missing-real-aws-deployment.md
Issue-Commit: e19d7761cdba7dfb41b7452b9b8e275a18501f91

# GetWorkload serves an off-emulator icon URL and omits accountConstraints

> _Note: this issue was identified against the code base at commit `e19d7761cdba7dfb41b7452b9b8e275a18501f91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkload

## Description
GetWorkload answers 200 but the WorkloadData it serializes is not self-consistent with the emulator it came from: the iconUrl member points at a host the emulator does not serve, and the accountConstraints member is dropped instead of serialized as an empty list. 2 causes.

--- Cause 1 of 2: iconUrl announces a host other than the emulator's own (fixture launch-wizard-workload-catalog-discovery, step task#2) ---
AWS: Smithy: GetWorkload, POST /getWorkload, success 200, output member workload: WorkloadData; member iconUrl: String, documented as "The URL of a workload icon." The model places no constraint on the host, so: recorded behavior only - the recording asserts iconUrl's SHAPE, not its literal value (the corpus records it as '<shaped-not-asserted>'), because the host and path of a generated URL are environment-specific. The requirement an emulator must meet is that every absolute URL it hands a client resolves on the endpoint the client is already talking to: the icon is an artifact the emulator is serving, so its URL must name the emulator's own data listener, with the workload-specific part (the workload name / icon file) taken from the catalog entry being returned.
Emulator: 200, $.workload.iconUrl = 'https://d1.awsstatic.com/launchwizard/workloads/SAP.png' - a literal public AWS CDN host baked into the seeded catalog entry, flagged host_not_own by the replay because it is not an address the emulator is reached on.
Why: a client that follows iconUrl leaves the emulator entirely. In a sandboxed or offline test environment the fetch fails on name resolution or egress policy, and in a connected one it silently reaches real AWS - so a console-like client rendering the workload catalog either errors or displays content the emulator never served, and no test can assert on the icon the emulator claims to host. It also makes the emulator's responses non-hermetic: the same body behaves differently depending on whether the runner has internet access.
Fix: the launchwizard workload-catalog seed/state and the GetWorkload serializer in the launchwizard crate - build iconUrl from the emulator's own request/endpoint context plus the catalog entry's icon identifier, rather than storing an absolute third-party URL in the seed.
Traces: launch-wizard-workload-catalog-discovery#task#2.

--- Cause 2 of 2: accountConstraints is omitted instead of serialized as an empty list (fixture launch-wizard-workload-catalog-discovery, step task#2) ---
AWS: Smithy: GetWorkload output workload: WorkloadData, member accountConstraints: AccountConstraintsList, @length 0..10, @uniqueItems. For a workload with no account constraints the service serializes the member as an empty JSON array; the member is present either way.
Emulator: 200 with $.workload.accountConstraints absent from the body entirely (expected [], got None).
Why: accountConstraints is how a client decides whether the calling account may deploy a workload. An absent member is not an empty list: code reading the field without a null guard faults, and code treating absence as "unknown" raises a false constraint warning.
Fix: the GetWorkload serializer in the launchwizard crate - emit accountConstraints as an empty list rather than skipping the member when the catalog entry has none.
Traces: launch-wizard-workload-catalog-discovery#task#2.

Relation to open issues: open issue 20261029T234833Z-launchwizard-seeded-workload-catalog-is-missing-real-aws-deployment.md carries a replay key for the accountConstraints half of this step (Cause 2 there), but its keys do not cover the iconUrl host divergence, so this step still diverges after that issue's accountConstraints fix and is reported here. Cause 2 is restated only so the whole step passes; Cause 1 is the net-new defect. Open issue 20261029T234833Z-launch-wizard-catalog-returns-placeholder-workload-data-listworkloads.md covers the list-shaped catalog divergences on the launch-wizard token and does not cover GetWorkload.

## Evidence
Replayed step launch-wizard-workload-catalog-discovery#task#2, operation GetWorkload (service launchwizard), verdict mismatch, expected status 200 / actual status 200:
- $.workload.iconUrl: host_not_own - expected a URL on the emulator's own data listener (corpus records the value as '<shaped-not-asserted>' because host/path are environment-specific), got 'https://d1.awsstatic.com/launchwizard/workloads/SAP.png'. The fixed part of the requirement is "an absolute URL whose host is the endpoint the client called"; the variable part is the per-workload icon path derived from the catalog entry. The recorded CDN host and the 'SAP.png' filename are one environment's values, not the requirement.
- $.workload.accountConstraints: missing - expected [] (an empty JSON array), got None (member absent).
No error code or error message is involved: both sides answer HTTP 200 and the divergence is confined to the serialized workload body.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a Launch Wizard client at its data listener.
2. POST /getWorkload with body {"workloadName":"SAP"} (any seeded workload reproduces it).
3. Observe HTTP 200 whose $.workload.iconUrl is an absolute https URL on a public AWS CDN host rather than on the emulator's own listener, and whose $.workload.accountConstraints member is absent rather than [].
4. Follow the returned iconUrl from the client: it does not resolve to the emulator, so with egress blocked the fetch fails and with egress open it reaches content the emulator never served.
5. Replay launch-wizard-workload-catalog-discovery#task#2 to confirm both fixes.
