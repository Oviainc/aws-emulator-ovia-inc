Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# CreateVirtualCluster never resolves securityConfigurationId (nor the EKS namespace it maps), so unknown references are accepted

_Filed: 2026-10-24T02:18:27Z_
_Operation: CreateVirtualCluster | Class: validation_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateVirtualCluster models `ResourceNotFoundException` (HTTP 400) and the only resource reference its input carries is `securityConfigurationId`. EMR on EKS resolves that id against the account's security configurations and refuses a create that names one which does not exist. The emulator stores the submitted value verbatim and never looks at the security-configuration store, so a cluster can be created referencing a security configuration id that was never created (or was deleted), and DescribeVirtualCluster then reports a dangling reference as if it were valid. `ResourceNotFoundException` is therefore unreachable on this operation.

A second symptom of the same missing resolution: a virtual cluster maps 1:1 onto a Kubernetes namespace (`containerProvider.info.eksInfo.namespace`), and EMR on EKS refuses a second virtual cluster for a namespace that already has one. The emulator keeps no namespace index, so N clusters can be created over the same containerProvider id + namespace, all RUNNING at once.

Expected: 400 ResourceNotFoundException for an unknown `securityConfigurationId`; a refusal for a duplicate containerProvider/namespace pair.
Actual: 200 for both, with the unresolved values persisted and echoed back.

## Evidence

Smithy model: `CreateVirtualCluster` errors include `ResourceNotFoundException` (HTTP 400) - "The specified resource was not found" - and its input declares `securityConfigurationId` (ResourceIdString) while the service also stores security configurations (CreateSecurityConfiguration/DescribeSecurityConfiguration are implemented in this crate, so the store to resolve against exists). `VirtualCluster` doc: "A virtual cluster is a Kubernetes namespace that Amazon EMR is registered with ... each virtual cluster maps to one namespace". Handler: src/ops/create_virtual_cluster.rs lines 203-226 - the comment at lines 216-219 states the id is "Persisted as submitted" and no lookup against `state.security_configurations` is performed; nothing indexes the namespace either. Test tests/all/virtual_cluster_job_run_loop.rs line 308 deliberately creates a cluster with `"securityConfigurationId": "sec123"` (no such configuration exists) and asserts HTTP 200, so the current test suite locks in the permissive behavior.

## Steps to Reproduce

1. POST /virtualclusters with body {"name":"vc1","clientToken":"t1","containerProvider":{"id":"eks1","type":"EKS","info":{"eksInfo":{"namespace":"emr"}}},"securityConfigurationId":"sec123"} where no security configuration sec123 was ever created
2. Observe HTTP 200 instead of 400 ResourceNotFoundException
3. GET /virtualclusters/{id} -> `securityConfiguration` id sec123 is reported as a live reference
4. POST /virtualclusters again with a different clientToken but the same containerProvider id and namespace -> a second RUNNING cluster is created for the same namespace
