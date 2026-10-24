Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# ManagedInstance is stored unvalidated: an empty configuration and an uncatalogued InstanceType both create an instance that reaches ALLOCATED

_Filed: 2026-10-24T01:58:46Z_
_Operation: CreateWorkspaceInstance | Class: validation_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateWorkspaceInstance persists `ManagedInstance` verbatim and performs no cross-field or catalog validation, so configurations the service cannot launch are accepted with 200 and then reported as a healthy, fully ALLOCATED instance.

Accepted today but not launchable on the real service:
* `ManagedInstance: {}` — no ImageId, no InstanceType, no SubnetId/NetworkInterfaces. A WorkSpace Instance is an EC2 launch, and a launch with no image and no instance type is rejected with ValidationException (the modelled 400) rather than provisioned.
* An InstanceType that satisfies the modelled pattern `^([a-z0-9-]+)\.([a-z0-9]+)$` but is not in the catalog the service's own ListInstanceTypes publishes (e.g. "zz9.plural"), and a type/platform/tenancy/billing combination that ListInstanceTypes reports as unsupported (e.g. a `t`-family type with Placement.Tenancy dedicated, or MONTHLY billing on a type whose SupportedInstanceConfigurations exclude it).
* Tag keys in the reserved `aws:` namespace (the Tag pattern admits `:`), which AWS refuses on every tagged resource.

Expected: a ValidationException (HTTP 400) naming the offending member for a configuration the service cannot launch, or — if the real service defers the check — an instance that lands in ERROR_ALLOCATING with WorkspaceInstanceErrors/EC2InstanceErrors populated. Actual: 200, the record is stored, and GetWorkspaceInstance reports ProvisionState ALLOCATED with both error lists absent, so neither the synchronous refusal nor the asynchronous failure is observable.

## Evidence

Spec: ManagedInstanceRequest members ImageId (^ami-…), InstanceType (len 1..64, ^([a-z0-9-]+)\.([a-z0-9]+)$), SubnetId, Placement etc. are all optional in the model, so nothing but handler logic can reject an unlaunchable combination; ListInstanceTypes publishes the launchable types with their SupportedInstanceConfigurations (BillingMode/PlatformType/Tenancy), and ValidationException (400) plus ProvisionStateEnum ERROR_ALLOCATING and GetWorkspaceInstanceResponse$WorkspaceInstanceErrors/EC2InstanceErrors are all modelled. Handler: src/ops/create_workspace_instance.rs lines ~189-226 stores `input.managed_instance` with no inspection of any member, and src/ops/get_workspace_instance.rs lines ~176-187 hardcodes `workspace_instance_errors: None, ec2_instance_errors: None`. State: src/state.rs `instance_type_catalog()` exists but is only consulted by ListInstanceTypes. Tests assert the permissive behaviour (tests/all/workspace_instance_lifecycle.rs `minimal_create()` = {"ManagedInstance":{}} and create_rows() case "no configuration at all"), so no test covers the AWS-side refusal.

## Steps to Reproduce

1. Call CreateWorkspaceInstance with {"ManagedInstance":{}} — emulator answers 200 with a new WorkspaceInstanceId
2. Call GetWorkspaceInstance with that id — ProvisionState becomes ALLOCATED, no WorkspaceInstanceErrors, although nothing could have been launched
3. Call CreateWorkspaceInstance with {"ManagedInstance":{"InstanceType":"zz9.plural"}} — accepted, although the type appears in no ListInstanceTypes page
4. Call CreateWorkspaceInstance with {"ManagedInstance":{"InstanceType":"t3.medium"},"Tags":[{"Key":"aws:created-by","Value":"x"}]} then ListTagsForResource — the reserved aws: key is stored instead of refused
