Issue-Source: trace-sweep-divergence
Issue-Trace: launch-wizard-catalog-list-workload-deployment-patterns
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: launch-wizard-catalog-list-workload-deployment-patterns launch-wizard-catalog-list-workloads
Issue-Shape-Id: com.amazonaws.launchwizard#LaunchWizard
Issue-Key: launch-wizard|ListWorkloadDeploymentPatterns|2xx->4xx|$.workloadDeploymentPatterns
Issue-Key: launch-wizard|ListWorkloadDeploymentPatterns|2xx->4xx|status
Issue-Key: launch-wizard|ListWorkloadDeploymentPatterns|2xx->2xx|$.workloadDeploymentPatterns[*]
Issue-Key: launch-wizard|ListWorkloads|2xx->2xx|$.workloads[*]
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# launch-wizard catalog returns placeholder workload data: ListWorkloads and ListWorkloadDeploymentPatterns diverge in items and fields

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListWorkloadDeploymentPatterns

## Description
The emulator's Launch Wizard catalog is backed by a hand-written placeholder seed instead of the AWS workload/deployment-pattern catalog. That one defect shows up three ways on the wire: a workload whose patterns AWS lists is not found at all, the patterns that are listed are different items at different versions, and the catalog summaries are serialized with most of their modeled members dropped. 3 causes.

--- Cause 1 of 3: ListWorkloadDeploymentPatterns answers 404 for a workload that exists in the catalog (fixture launch-wizard-catalog-list-workload-deployment-patterns, step task#1; same cluster also covers task#2, task#4, task#5, task#6, task#7) ---
AWS: Smithy: ListWorkloadDeploymentPatterns, restJson1, POST /listWorkloadDeploymentPatterns, success http code 200; input workloadName is @required (@length 1..100, @pattern ^[A-Za-z][a-zA-Z0-9-_]*$); output member workloadDeploymentPatterns targets WorkloadDeploymentPatternDataSummaryList. The operation's only client errors are ResourceNotFoundException (404) and ValidationException (400); 404 is reserved for a workload name that is not in the catalog at all. For a workload that IS in the catalog the service answers 200 and a list with one WorkloadDeploymentPatternDataSummary per pattern, each carrying workloadName, deploymentPatternName, workloadVersionName, deploymentPatternVersionName, displayName, description, status and accountConstraints. The recording shows this for a solution workload: two patterns (an ECS-architecture pattern and a Lambda-architecture pattern) at a matching workloadVersionName/deploymentPatternVersionName pair, status ACTIVE, accountConstraints serialized as an empty array.
Emulator: HTTP 404 with no workloadDeploymentPatterns member at all ($.workloadDeploymentPatterns missing, expected the two-element list). The workload name requested is simply absent from the emulator's seeded catalog, so the handler takes its not-found path.
Why: the documented discovery flow is ListWorkloads -> ListWorkloadDeploymentPatterns -> GetWorkloadDeploymentPattern -> CreateDeployment. A client that enumerates workloads and then asks for one workload's patterns gets ResourceNotFoundException for a workload the emulator itself just listed, so the flow dead-ends and no deployment can be composed. Code 404 vs 200 also changes SDK error handling: the client raises instead of iterating an empty or populated page.
Fix: the launchwizard crate's workload-catalog seed/state (the table that backs workload and deployment-pattern lookups) must carry the real AWS catalog entries, and ListWorkloadDeploymentPatterns' handler must only return ResourceNotFoundException for a workload absent from that catalog.
Traces: launch-wizard-catalog-list-workload-deployment-patterns#task#1 (plus task#2, task#4, task#5, task#6, task#7).

--- Cause 2 of 3: ListWorkloadDeploymentPatterns returns different patterns, at different versions, with accountConstraints omitted (fixture launch-wizard-catalog-list-workload-deployment-patterns, step task#3) ---
AWS: Smithy: WorkloadDeploymentPatternDataSummary members workloadName, deploymentPatternName (@pattern ^[A-Za-z0-9][a-zA-Z0-9-]*$), workloadVersionName and deploymentPatternVersionName (@length 5..30, @pattern ^(([A-Za-z0-9][a-zA-Z0-9-]*)|(\d+\.\d+\.\d+))$), displayName, description, status (WorkloadDeploymentPatternStatus enum), statusMessage, accountConstraints (AccountConstraintsList, @length 0..10, @uniqueItems). For the SAP workload the recording returns the SAP HANA high-availability, multi-instance and single-instance patterns, every element carrying the SAME workloadVersionName and deploymentPatternVersionName (the workload's catalog version), human-readable displayName/description per pattern, status ACTIVE, and accountConstraints present as an empty array.
Emulator: 200, but the list is the placeholder seed: different deploymentPatternName values than the catalog's, workloadVersionName and deploymentPatternVersionName that disagree with each other and with the workload's catalog version, displayName/description text that is not the catalog's, and no accountConstraints member on any element. Element-by-element the replay reports every index of $.workloadDeploymentPatterns as mismatched.
Why: deploymentPatternName and deploymentPatternVersionName are the keys a client feeds to GetWorkloadDeploymentPattern, ListDeploymentPatternVersions, GetDeploymentPatternVersion and CreateDeployment. Names and versions that do not exist in AWS make every downstream call unreproducible against the real service, and a missing accountConstraints member breaks clients that read the field without a null guard.
Fix: same catalog seed/state as Cause 1, plus the ListWorkloadDeploymentPatterns serializer, which must emit accountConstraints (an empty list when there are no constraints) rather than omitting the member.
Traces: launch-wizard-catalog-list-workload-deployment-patterns#task#3.

--- Cause 3 of 3: ListWorkloads returns placeholder workload names and drops most summary members (fixture launch-wizard-catalog-list-workloads, step task#1) ---
AWS: Smithy: ListWorkloads, POST /listWorkloads, success 200, output workloads: WorkloadDataSummaryList, nextToken; paginated (items=workloads). WorkloadDataSummary models workloadName (@pattern ^[A-Za-z][a-zA-Z0-9-_]*$), displayName, status and accountConstraints. The recording returns one element per catalog workload, each with its catalog workloadName (the Microsoft Active Directory, SAP, and solution-catalog workloads), displayName, status ACTIVE, accountConstraints as an empty array, a prose description, a documentationUrl pointing at that workload's AWS documentation page, and - recorded behavior only, this member is not in the on-disk WorkloadDataSummary - a workloadOwnerDisplayName naming the owning team.
Emulator: 200, but every element is a three-member stub carrying only workloadName, displayName and status, and the workloadName values are the placeholder seed's short names rather than the catalog's (for example the Active Directory workload is listed under a short name instead of its catalog name), with catalog workloads missing entirely. The replay reports every index of $.workloads as mismatched.
Why: workloadName from this call is the required input to GetWorkload, ListWorkloadDeploymentPatterns and CreateDeployment, so a renamed workload makes the whole chain 404 (exactly what Cause 1 shows). Omitting description/documentationUrl/accountConstraints/workloadOwnerDisplayName means a catalog UI or script built on the real response renders empty fields or null-dereferences.
Fix: the launchwizard crate's workload-catalog seed/state and the ListWorkloads serializer (emit accountConstraints as an empty list, and the description/documentationUrl/owner fields the recording shows).
Traces: launch-wizard-catalog-list-workloads#task#1.

## Evidence
Cause 1 - fixture launch-wizard-catalog-list-workload-deployment-patterns step task#1: expected status 200, got 404; $.workloadDeploymentPatterns expected a 2-element list of WorkloadDeploymentPatternDataSummary (ECS and Lambda architecture patterns, accountConstraints [], status ACTIVE), got None. Cluster also covers task#2, task#4, task#5, task#6, task#7 (affected_count 6).
Cause 2 - same fixture step task#3: status 200 on both sides, but $.workloadDeploymentPatterns[0..7] all mismatch: expected {deploymentPatternName: SapHanaHA, deploymentPatternVersionName: 1.0.27, workloadVersionName: 1.0.27, accountConstraints: [], status: ACTIVE, displayName/description from the SAP catalog}, got {deploymentPatternName: SapHanaMultiNode, workloadVersionName: 1.0.0, deploymentPatternVersionName: 2.0.0, no accountConstraints}; expected index 1 SapHanaMulti / got SapHanaSingle; expected index 2 SapHanaSingle / got SapNWOnHanaHA.
Cause 3 - fixture launch-wizard-catalog-list-workloads step task#1: status 200 on both sides, but $.workloads[0..6] all mismatch: expected {workloadName: MicrosoftActiveDirectory, displayName: 'Microsoft Active Directory', status: ACTIVE, accountConstraints: [], description: ..., documentationUrl: https://docs.aws.amazon.com/launchwizard/..., workloadOwnerDisplayName: 'AWS Launch Wizard'}, got {workloadName: ActiveDirectory, displayName: 'Microsoft Active Directory', status: ACTIVE} only; expected index 1 the SO0023 solution workload / got SAP; expected index 2 SAP / got SQLServer.
No open issue covers the catalog contents: the open launch-wizard issues concern ListDeploymentPatternVersions filter-pattern text (20261028T114957Z), UpdateDeployment update-target rules (20261028T210759Z), ListDeploymentPatternVersions nextToken scoping (20261029T045748Z) and TagResource modifiedAt (20261029T072906Z).

## Steps to Reproduce
1. Boot the emulator from mainline and point a Launch Wizard client at its data listener.
2. Cause 3: POST /listWorkloads with {} -> observe 200 with three-member workload summaries under placeholder workloadNames, no accountConstraints/description/documentationUrl. Replay launch-wizard-catalog-list-workloads#task#1.
3. Cause 1: POST /listWorkloadDeploymentPatterns with the solution workload's catalog workloadName (SO0023) -> observe 404 ResourceNotFoundException instead of 200 and the two architecture patterns. Replay launch-wizard-catalog-list-workload-deployment-patterns#task#1.
4. Cause 2: POST /listWorkloadDeploymentPatterns with {"workloadName":"SAP"} -> observe 200 with placeholder pattern names/versions and no accountConstraints. Replay launch-wizard-catalog-list-workload-deployment-patterns#task#3.
