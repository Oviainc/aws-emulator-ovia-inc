Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 43c3b52fd22125f2604949f65e29a8c5612a98db

# Every deployment pattern publishes the same six-field specification schema, so patterns are indistinguishable and real-world specification keys are rejected

_Filed: 2026-10-24T07:07:13Z_
_Operation: GetWorkloadDeploymentPattern | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `43c3b52fd22125f2604949f65e29a8c5612a98db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The seeded catalogue computes ONE specification schema per workload and clones it into every pattern of that workload (`specifications.clone()` for each pattern), so GetWorkloadDeploymentPattern reports a byte-identical `specifications` list for SapHanaSingle, SapHanaMultiNode and SapNWOnHanaHA (and for SqlServerSingle vs SqlServerHA, AWSManagedAD vs SelfManagedAD) — only displayName/description differ. The whole published vocabulary is six common fields (KeyPairName, VpcId, CreateSecurityGroup, SecurityGroupId, DisableDeploymentRollback, SaveDeploymentArtifacts) plus two SAP extras, with KeyPairName and VpcId marked required for every pattern of every workload.

Expected: the schema a pattern publishes is the pattern's own — a highly-available pattern requires settings a single-instance pattern does not (that is why `required`, `allowedValues` and `conditionals` are modeled per field and per pattern, and why CreateDeployment's docs point at the per-workload "SAP deployment specifications" list). Actual: one schema for all patterns of a workload.

This is observable beyond the catalogue read: CreateDeployment judges the submitted map against this schema and REFUSES any key the schema does not publish ("'<key>' is not a specification of deployment pattern '<pattern>'") as well as any missing required field. A client that sends the specification keys a pattern really needs is answered HTTP 400 ValidationException, and a client that discovers settings through GetWorkloadDeploymentPattern cannot tell two patterns apart.

## Evidence

Spec: Smithy model — WorkloadDeploymentPatternData$specifications is DeploymentSpecificationsData (len 1..100) of DeploymentSpecificationsField{name, description, required, allowedValues, conditionals}, documented as "The settings specified for the deployment ... To retrieve the specifications required to create a deployment for other workloads, use the GetWorkloadDeploymentPattern operation" — i.e. the schema is the pattern's own contract, and CreateDeployment's doc links the per-workload SAP specification list, which is far larger and pattern-dependent.
Handler/state: services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs — `common_specification_fields()` (around lines 722-774) plus `workload_specification_fields()` (lines 782-800) are built once per workload in `seeded_workloads()` and handed to every pattern via `specifications: specifications.clone()` (around line 914); `DeploymentPattern::to_data` reports that list verbatim (around lines 326-331), and `specification_warnings` (around lines 1271-1305) is what CreateDeployment turns into a ValidationException for any key outside it (src/ops/create_deployment.rs, around lines 250-258).
Tests: tests/all/workload_deployment_patterns.rs::the_listed_summary_matches_the_get_detail_for_every_pattern and ::the_reported_specification_schema_drives_a_create_deployment only check self-consistency (a create built FROM the reported schema succeeds); no test asserts that two patterns of one workload must publish the same schema, so nothing pins the current sharing as intended.

## Steps to Reproduce

1. Call GetWorkloadDeploymentPattern(workloadName='SAP', deploymentPatternName='SapHanaSingle') and record the `specifications` list.
2. Call GetWorkloadDeploymentPattern(workloadName='SAP', deploymentPatternName='SapNWOnHanaHA') and compare — expect a different, pattern-specific set of fields (different required fields for a highly-available NetWeaver pattern); observe an identical list.
3. Call CreateDeployment for either pattern with a specification key a real deployment of that pattern needs but the emulator does not publish (any valid DeploymentSpecifications key, e.g. 'SapSid' or 'InstanceType').
4. Observe HTTP 400 ValidationException "'<key>' is not a specification of deployment pattern '<pattern>'", so no realistic specification map can be submitted.
