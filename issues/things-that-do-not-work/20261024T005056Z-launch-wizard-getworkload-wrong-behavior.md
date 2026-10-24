Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# GetWorkload never reports documentationUrl or iconUrl for a catalogue workload

_Filed: 2026-10-24T00:50:56Z_
_Operation: GetWorkload | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`WorkloadData` carries `documentationUrl` ("The URL of a workload document") and `iconUrl` ("The URL of a workload icon") in addition to the members `WorkloadDataSummary` shares. For a real AWS-published, ACTIVE Launch Wizard workload (SAP, SQL Server, Active Directory, …) GetWorkload returns both URLs — they are the catalogue metadata the Launch Wizard console renders when it lists workloads and links to their deployment guide, and they are static per-workload values of AWS's own service-owned catalogue (no API creates a workload, so these are not caller state the emulator could be missing).

Expected: GetWorkload('SAP') reports a non-empty `documentationUrl` (the workload's Launch Wizard documentation page) and a non-empty `iconUrl` for every seeded catalogue entry, alongside `workloadName`/`displayName`/`description`/`status`.

Actual: the projection hard-codes both members to absent, so a client that reads either one gets nothing for every workload in the catalogue, while the sibling members of the SAME shape are reported. The result is a `WorkloadData` that is structurally never complete, regardless of which workload is asked for.

## Evidence

Spec: Smithy model `com.amazonaws.launchwizard#WorkloadData` declares `documentationUrl` and `iconUrl` (both String; docs "The URL of a workload document" / "The URL of a workload icon"); they are the two members `WorkloadData` adds over `WorkloadDataSummary` apart from `description`/`statusMessage`, and `description` IS populated.
Handler: services/service-com-amazonaws-launchwizard-launchwizard/src/ops/get_workload.rs handle() lines 153-166 returns `workload.to_data()`; the projection is src/state.rs `Workload::to_data` lines 342-353, which sets `documentation_url: None` and `icon_url: None` unconditionally (the doc comment at lines 339-341 states the choice: "this emulator hosts no workload document and no icon"). The seeded catalogue (src/state.rs `seeded_workloads`, lines 767-868) stores no URL fields on `Workload` (struct at lines 302-315), so there is nothing for a fix to read until the seed carries them.
Tests: tests/all/workload_catalogue.rs::a_reported_workload_carries_only_members_read_off_the_stored_entry (lines 84-90+) asserts these members are ABSENT, i.e. the current behavior is pinned by a test that encodes the emulator's choice rather than AWS's response — so the gap is unasserted in the AWS direction and the test must move with the fix.
Parallel: the already-filed GetDeployment/wrong-behavior issue (20261023T230915Z-launch-wizard-getdeployment-wrong-behavior) is the same class of omission for `DeploymentData$resourceGroup`; this is the workload-catalogue half and is a different operation.

## Steps to Reproduce

1. Boot the emulator and point a launch-wizard client at it.
2. Call ListWorkloads and note the seeded workload names (SAP, SQLServer, ActiveDirectory).
3. Call GetWorkload(workloadName='SAP') and inspect the `workload` member.
4. Expect `documentationUrl` and `iconUrl` to be present and non-empty (as real Launch Wizard returns for a published workload); observe both are absent from the JSON while `displayName`, `description` and `status` are populated.
5. Repeat for every other workload the catalogue reports and observe the same two members are always absent.
