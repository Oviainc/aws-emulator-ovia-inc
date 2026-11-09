Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__connect.deploy-connect-instance-and-core-resource-types-round-trip-and-delete cloudformation__connect.deploy-connect-instance-duplicate-alias-rejected-leaving-the-original
Issue-Key: cloudformation__connect|CreateStack|ValidationError
Issue-Key: cloudformation__connect.deploy-connect-instance-duplicate-alias-rejected-leaving-the-original|run#9|AssertionError|assert [i["Arn"] for i in listed] == [instance_arn], f"ListInstances with {alias!r}: {listed!r}"
Issue-Commit: 45bd6d867b1a7177de267d075c8182289448ffad

# cloudformation__connect: 2 net-new causes — five AWS::Connect::* resource types are unregistered, and AWS::Connect::Instance is stub-provisioned (the stack's instance never exists in Connect)

> _Note: this issue was identified against the code base at commit `45bd6d867b1a7177de267d075c8182289448ffad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes in the CloudFormation emulator's Amazon Connect resource coverage. Both were found by replaying the two cloudformation__connect deploy checks; neither is marked ALREADY FILED or OWNED. Related but NOT covering issues: 20261109T113637Z-cloudformation-3-net-new-causes-createstack-rejects-aws.md and 20261109T120136Z-cloudformation-directconnect-createstack-refuses-aws-directconnect-directconnectgateway.md report the same shape of defect for AWS::NetworkManager::* and AWS::DirectConnect::*; 20261103T120437Z-connect-instance-plane-5-net-new-causes-listinstances.md covers Connect's own instance-plane refusals, not CloudFormation provisioning. No open issue names any AWS::Connect::* resource type.

Cause 1 of 2: CreateStack refuses five modelled AWS::Connect::* resource types as "Unrecognized resource types".
AWS: recorded behavior only for the CloudFormation registry (CloudFormation resource types are not described by a Smithy model). CloudFormation publicly registers AWS::Connect::SecurityProfile, AWS::Connect::UserHierarchyStructure, AWS::Connect::ApprovedOrigin, AWS::Connect::ContactFlowModule and AWS::Connect::AgentStatus, so a template declaring them is accepted by CreateStack and each resource is provisioned through the corresponding Connect API (CreateSecurityProfile, UpdateUserHierarchyStructure, AssociateApprovedOrigin, CreateContactFlowModule, CreateAgentStatus — all present in the Connect Smithy model). The recorded run drives such a template to CREATE_COMPLETE and then reads the resources back through Connect (ListSecurityProfiles GET /security-profiles-summary/{InstanceId}, ListApprovedOrigins GET /instance/{InstanceId}/approved-origins, ListAgentStatuses GET /agent-status/{InstanceId}, ListContactFlowModules, DescribeUserHierarchyStructure).
Emulator: CreateStack is refused 400 with error code ValidationError and message "Template format error: Unrecognized resource types: [AWS::Connect::SecurityProfile, AWS::Connect::UserHierarchyStructure, AWS::Connect::ApprovedOrigin, AWS::Connect::ContactFlowModule, AWS::Connect::AgentStatus]". In that message the fixed text is "Template format error: Unrecognized resource types: [" ... "]"; the bracketed list is the set of template resource types the emulator's registry does not know, i.e. a value derived from the request. The refusal happens at template parse time, so no stack, no event and no resource is ever created.
Why: a client that deploys a Connect estate by CloudFormation — which is how security profiles, approved origins, contact-flow modules, agent statuses and the user-hierarchy structure are normally managed — cannot create any stack at all. The whole template is rejected, not degraded, so the deploy, the update-driven child teardown and the delete paths are all unreachable, and nothing downstream of the stack (Refs, GetAtts, Outputs) can be exercised.
Fix: the CloudFormation service crate's resource-type registry (the table consulted by template validation during CreateStack) plus one resource handler per type, each calling the already-modelled Connect operation and recording the Connect-side state.
Traces: cloudformation__connect.deploy-connect-instance-and-core-resource-types-round-trip-and-delete.

Cause 2 of 2: an AWS::Connect::Instance in a CREATE_COMPLETE stack is never created in Connect — ListInstances does not report it.
AWS: Smithy: Connect ListInstances → GET /instance, output member InstanceSummaryList (paginated on NextToken/MaxResults, MaxResult10 range 1..10), each summary carrying Id, Arn, InstanceAlias, InstanceStatus, IdentityManagementType; DescribeInstance → GET /instance/{InstanceId} returning Instance, with ResourceNotFoundException (404) when the instance does not exist. A CloudFormation stack that provisions AWS::Connect::Instance creates a real instance, so immediately after CREATE_COMPLETE the instance whose ARN the stack publishes via Fn::GetAtt Instance.Arn appears exactly once in ListInstances with the template's InstanceAlias, and DescribeInstance on its Id answers 200 with InstanceStatus ACTIVE, IdentityManagementType CONNECT_MANAGED and the templated InboundCallsEnabled/OutboundCallsEnabled.
Emulator: the stack reaches CREATE_COMPLETE and its Outputs do carry an InstanceArn for Fn::GetAtt ["Instance", "Arn"], but Connect's own state never learns about the instance: ListInstances paged to exhaustion contains no entry whose InstanceAlias is the template's alias, so the replay fails with `AssertionError: ListInstances with '<alias>': []` (the alias is a per-run generated name; the requirement is that the list contains exactly the one instance the stack created, not that specific string). The resource is therefore stub-provisioned: the handler mints a physical id/ARN without creating the Connect-side resource.
Why: every client that deploys a Connect instance by CloudFormation and then drives it through the Connect API is broken — the ARN/Id handed back by Ref/GetAtt refers to nothing, DescribeInstance on it cannot succeed, and the alias-collision contract (a second stack with an alias Connect already holds must roll back with the handler reason "Invalid Input. Instance alias is already used. (Service: Connect, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" and HandlerErrorCode AlreadyExists, where only the request id and attempt count are per-run values) cannot hold, because Connect holds no instance to collide with.
Fix: the AWS::Connect::Instance resource handler in the CloudFormation service crate — it must call Connect's CreateInstance and write the instance into the Connect crate's state (and delete it on stack delete) rather than synthesizing an id, so Connect's ListInstances/DescribeInstance observe it.
Traces: cloudformation__connect.deploy-connect-instance-duplicate-alias-rejected-leaving-the-original.

## Evidence
Cause 1 — check cloudformation__connect.deploy-connect-instance-and-core-resource-types-round-trip-and-delete:
  Expected: CreateStack accepts the template and the stack proceeds to CREATE_COMPLETE.
  Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Connect::SecurityProfile, AWS::Connect::UserHierarchyStructure, AWS::Connect::ApprovedOrigin, AWS::Connect::ContactFlowModule, AWS::Connect::AgentStatus]

Cause 2 — check cloudformation__connect.deploy-connect-instance-duplicate-alias-rejected-leaving-the-original:
  The stack with a single AWS::Connect::Instance reached CREATE_COMPLETE and its Outputs returned InstanceArn (Fn::GetAtt ["Instance","Arn"]).
  Expected: [i["Arn"] for i in ListInstances entries whose InstanceAlias == the template alias] == [that InstanceArn] — exactly one instance, the stack's.
  Actual: AssertionError: ListInstances with 'sim2real-cxn-dup-7ed1a359f381': []  (ListInstances, paged to exhaustion, reports no instance with the alias; the alias value is per-run).

## Steps to Reproduce
Cause 1:
1. CreateStack with a template whose Resources include AWS::Connect::Instance plus AWS::Connect::SecurityProfile, AWS::Connect::UserHierarchyStructure, AWS::Connect::ApprovedOrigin, AWS::Connect::ContactFlowModule and AWS::Connect::AgentStatus.
2. Observe the 400 ValidationError "Template format error: Unrecognized resource types: [...]" instead of an accepted stack.

Cause 2:
1. CreateStack with a template containing one AWS::Connect::Instance (IdentityManagementType CONNECT_MANAGED, a unique InstanceAlias, Attributes InboundCalls/OutboundCalls false) and an Output Fn::GetAtt ["Instance","Arn"].
2. Poll DescribeStacks until CREATE_COMPLETE; read the InstanceArn Output.
3. Call Connect ListInstances (all pages) and filter on InstanceAlias == the template alias.
4. Observe an empty list instead of exactly the stack's instance; DescribeInstance on the ARN's resource id likewise finds nothing.
5. (Follow-on contract this blocks) CreateStack a second stack with the same alias and observe ROLLBACK_COMPLETE with the Instance resource's CREATE_FAILED reason 'Resource handler returned message: "Invalid Input. Instance alias is already used. (Service: Connect, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)'.
