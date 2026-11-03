Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.create-resource-unsupported-type-target-rejected
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.create-resource-unsupported-type-target-rejected|run#4|AssertionError|raise AssertionError(
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# cloudcontrol: no NON_PROVISIONABLE public resource type exists in the registry, so CreateResource/GetResource can never answer UnsupportedActionException (405)

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
Cause 1 of 1: the emulator's CloudFormation extension registry publishes NO public RESOURCE type whose ProvisioningType is NON_PROVISIONABLE, so (a) a client cannot discover one with cloudformation ListTypes(Visibility=PUBLIC, Type=RESOURCE, ProvisioningType=NON_PROVISIONABLE, DeprecatedStatus=LIVE), and (b) Cloud Control's modeled UnsupportedActionException refusal for CREATE and READ against such a type is unreachable in this emulator.

AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model) models UnsupportedActionException on BOTH CreateResource and GetResource — errorKind client, HTTP 405, awsQueryError code UnsupportedActionException. The CloudFormation model documents ProvisioningType NON_PROVISIONABLE as "The resource type doesn't include create, read, and delete handlers, and therefore can't actually be provisioned", and ListTypes "[r]eturns summary information about ... all public extensions from Amazon Web Services and third-party publishers". On real AWS the public registry contains at least one LIVE public RESOURCE extension in that provisioning class, and Cloud Control refuses an action against it with UnsupportedActionException and the messages 'Resource type <TypeName> does not support CREATE action' (CreateResource) and 'Resource type <TypeName> does not support READ action' (GetResource). In those messages the fixed text is "Resource type " + " does not support CREATE action" / " does not support READ action"; the only variable part is <TypeName>, taken verbatim from the request's TypeName member — no id, token or timestamp appears in them. Beyond the modeled error shape and its 405 status, the catalog content is recorded behavior only: the check discovers the type through ListTypes rather than hard-coding a name, precisely because the registry snapshot is AWS-managed.

Emulator: cloudformation ListTypes answers 200 with an EMPTY TypeSummaries list for that filter combination, so the check aborts before it ever calls Cloud Control: "AssertionError: no NON_PROVISIONABLE public RESOURCE type is visible via cloudformation.list_types(ProvisioningType='NON_PROVISIONABLE') in this account/region right now". The cause is in the registry's published catalog, not in the listing filter: every AWS-published resource-type record is stamped provisioning_type FULLY_MUTABLE, and published hooks/modules carry provisioning_type None, so the ProvisioningType filter in ListTypes (which matches the reported default version's own provisioning_type) can never admit a record. Note: the stub issue 20261025T015306Z-cloudformation-listtypes-stub.md is marked FIXED and ListTypes now answers 200 — this is NOT a refusal left behind by that fix, it is a missing catalog class behind the now-implemented listing.

Why: a client that enumerates the public registry to find non-provisionable extensions gets an empty set where AWS returns entries, so registry-driven tooling (and any code that must handle a type with no create/read handlers) sees a registry that claims every public resource type is fully mutable. Worse, Cloud Control's 405 UnsupportedActionException branch is dead code in practice: an application that handles that refusal can never be exercised against the emulator, and a caller that targets such a type gets some other outcome instead of the modeled 405.

Fix: the CloudFormation registry's AWS-published catalog and its provisioning classification — services/service-com-amazonaws-cloudformation-cloudformation/src/state/registry.rs, where the published resource-type record's StoredTypeVersion is built with provisioning_type FULLY_MUTABLE for every type (and None for published hook records); plus the Cloud Control CreateResource/GetResource handlers in services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/, which must map a type in that provisioning class to UnsupportedActionException (405) with the CREATE/READ messages above rather than attempting the action.

Traces: cloudcontrol.create-resource-unsupported-type-target-rejected (mismatch).

## Evidence
Expected (AWS): cloudformation.list_types(Visibility='PUBLIC', Type='RESOURCE', ProvisioningType='NON_PROVISIONABLE', DeprecatedStatus='LIVE', MaxResults=100) returns at least one TypeSummary; cc.create_resource(TypeName=<that type>, DesiredState='{}') is refused with HTTP 405, Error.Code 'UnsupportedActionException', Error.Message 'Resource type <TypeName> does not support CREATE action'; cc.get_resource(TypeName=<that type>, Identifier=<anything>) is refused with HTTP 405, Code 'UnsupportedActionException', Message 'Resource type <TypeName> does not support READ action'; while the same CreateResource shape on AWS::SQS::Queue succeeds and is readable through GetResource.

Actual (emulator): ListTypes answers 200 with TypeSummaries == [] (and no NextToken) for that filter set, across up to 20 pages. The check therefore fails at check_cloudcontrol_create_resource_unsupported_type_target_rejected.py:31 with "AssertionError: no NON_PROVISIONABLE public RESOURCE type is visible via cloudformation.list_types(ProvisioningType='NON_PROVISIONABLE') in this account/region right now; this category is a live AWS-managed registry snapshot this check cannot itself provision, so there is nothing to target for CreateResource in this environment". Source confirmation: services/service-com-amazonaws-cloudformation-cloudformation/src/state/registry.rs builds every AWS-published resource-type version with `provisioning_type: Some(provisioning_type::FULLY_MUTABLE.to_string())` and every published (hook) record with `provisioning_type: None`, and ops/list_types.rs admits a record only when `version.provisioning_type.as_deref() == Some(provisioning)`.

Smithy evidence: CreateResource errors include {shape: UnsupportedActionException, errorKind: client, httpError: 405}; GetResource errors include the same. CloudFormation ListTypes input member ProvisioningType is the enum FULLY_MUTABLE | IMMUTABLE | NON_PROVISIONABLE.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cfn = client('cloudformation'); cfn.list_types(Visibility='PUBLIC', Type='RESOURCE', ProvisioningType='NON_PROVISIONABLE', DeprecatedStatus='LIVE', MaxResults=100) — observe TypeSummaries == [] (paging through NextToken yields nothing either). On AWS this returns at least one public resource type.
3. Because no such type can be discovered, the follow-on assertions never run. Once a NON_PROVISIONABLE public resource type exists in the registry, verify: cc = client('cloudcontrol'); cc.create_resource(TypeName=<that type>, DesiredState='{}') must raise ClientError with HTTP 405, Code 'UnsupportedActionException', Message 'Resource type <that type> does not support CREATE action'; and cc.get_resource(TypeName=<that type>, Identifier='does-not-matter') must raise HTTP 405 'UnsupportedActionException' with 'Resource type <that type> does not support READ action'.
4. Positive control that must keep working: cc.create_resource(TypeName='AWS::SQS::Queue', DesiredState=json.dumps({'QueueName': <name>})) reaches OperationStatus SUCCESS and GetResource on the resulting identifier reports QueueName.
5. Replay check cloudcontrol.create-resource-unsupported-type-target-rejected.
