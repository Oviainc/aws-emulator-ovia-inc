Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront.deploy-cfn-cloudfront-standalone-resources-lifecycle cloudformation__cloudfront.deploy-cloudfront-connection-group-round-trip-and-delete cloudformation__cloudfront.deploy-cloudfront-distribution-and-monitoring-subscription-round-trip-and-delete cloudformation__cloudfront.deploy-cfn-cloudfront-cache-policy-invalid-ttl-order-create-rejected cloudformation__cloudfront.deploy-cloudfront-publickey-invalid-encoded-key-rejected cloudformation__cloudfront.deploy-origin-access-control-invalid-signing-protocol-rejected cloudformation__cloudfront.deploy-cloudfront-publickey-and-keygroup-round-trip-and-delete
Issue-Key: cloudformation__cloudfront|CreateStack|ValidationError
Issue-Key: cloudformation__cloudfront.deploy-cfn-cloudfront-cache-policy-invalid-ttl-order-create-rejected|run#10|AssertionError|assert listed == [policy_id], f"custom cache policies named {good_name!r}: {listed!r}"
Issue-Key: cloudformation__cloudfront.deploy-cloudfront-publickey-invalid-encoded-key-rejected|run#12|AssertionError|assert listed == [(good_ref, good_pem)], f"ListPublicKeys found {listed!r} for the control"
Issue-Key: cloudformation__cloudfront.deploy-origin-access-control-invalid-signing-protocol-rejected|run#10|AssertionError|assert listed == [oac_id], f"origin access controls named {good_name!r}: {listed!r}"
Issue-Key: cloudformation__cloudfront.deploy-cloudfront-publickey-and-keygroup-round-trip-and-delete|stack_outputs#10|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# cloudformation__cloudfront: 3 causes — CFN refuses four AWS::CloudFront::* resource types as unrecognized, CFN-provisioned CloudFront resources never appear in CloudFront's list APIs, and a stack Output is serialized without OutputValue

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three distinct root causes keep every cloudformation__cloudfront check failing. A fixer must close all three.

Cause 1 of 3: CloudFormation does not recognize four CloudFront resource types.
AWS: AWS::CloudFront::KeyValueStore, AWS::CloudFront::ConnectionFunction, AWS::CloudFront::ConnectionGroup and AWS::CloudFront::MonitoringSubscription are supported CloudFormation resource types; CreateStack/UpdateStack accept a template declaring them (HTTP 200 with StackId) and provision the matching CloudFront resource (recorded behavior only — CloudFormation's Smithy model carries the ValidationError class but not the type registry).
Emulator: CreateStack is refused 400 ValidationError with "Template format error: Unrecognized resource types: [<Type>, ...]" — the fixed text is "Template format error: Unrecognized resource types: [" + the list of declared types the registry does not know + "]". Observed lists: [AWS::CloudFront::KeyValueStore, AWS::CloudFront::ConnectionFunction], [AWS::CloudFront::ConnectionGroup], [AWS::CloudFront::MonitoringSubscription].
Why: no IaC-deployed CloudFront key-value store, connection function, connection group or monitoring subscription is possible at all; the stack never exists, so nothing downstream (round-trip read, delete, drift) can run.
Fix: CloudFormation's resource-type registry plus a provisioner per type, wired to the CloudFront service's create/update/delete operations.
Failing checks: cloudformation__cloudfront.deploy-cfn-cloudfront-standalone-resources-lifecycle, cloudformation__cloudfront.deploy-cloudfront-connection-group-round-trip-and-delete, cloudformation__cloudfront.deploy-cloudfront-distribution-and-monitoring-subscription-round-trip-and-delete.

Cause 2 of 3: a CloudFront resource provisioned by an accepted stack is not visible in CloudFront's own list operations.
AWS: once a stack reaches CREATE_COMPLETE, the resource it created is a normal CloudFront resource: ListCachePolicies(Type="custom") contains the created cache policy (and GetCachePolicy returns its config), ListPublicKeys contains the created public key with its encoded key, ListOriginAccessControls contains the created origin access control (recorded behavior only).
Emulator: the stack reaches CREATE_COMPLETE and its Ref output yields an id, but the corresponding CloudFront list is empty — ListCachePolicies(Type="custom") returns no item named by the template, ListPublicKeys returns [], ListOriginAccessControls returns []. Hypothesis: the CFN provisioner records the resource in stack state and mints an id without registering it in the CloudFront service's state (or the list filter never sees CFN-created items).
Why: a client that deploys a cache policy / public key / OAC with CloudFormation and then reads it back, or attaches it by id to a distribution, finds no such resource — the IaC and API views of the same account disagree.
Fix: the CloudFormation provisioner for AWS::CloudFront::CachePolicy, AWS::CloudFront::PublicKey and AWS::CloudFront::OriginAccessControl must create the resource through the CloudFront service's own state (the same path CreateCachePolicy/CreatePublicKey/CreateOriginAccessControl use), so the list/get operations see it.
Failing checks: cloudformation__cloudfront.deploy-cfn-cloudfront-cache-policy-invalid-ttl-order-create-rejected (control stack CREATE_COMPLETE, PolicyId output present, ListCachePolicies(custom) == []), cloudformation__cloudfront.deploy-cloudfront-publickey-invalid-encoded-key-rejected (ListPublicKeys == []), cloudformation__cloudfront.deploy-origin-access-control-invalid-signing-protocol-rejected (ListOriginAccessControls == []).

Cause 3 of 3: DescribeStacks serializes a stack Output with no OutputValue.
AWS: every Output of a stack in a terminal CREATE_COMPLETE state carries both OutputKey and OutputValue (the resolved Ref/Fn::GetAtt value); CloudFormation's Output member OutputValue is populated for a completed stack (recorded behavior only).
Emulator: DescribeStacks returns an Outputs entry that has OutputKey but no OutputValue member at all, so a client indexing the output map raises KeyError: 'OutputValue'.
Why: any consumer of stack outputs — the standard way one stack hands an id to a caller or another stack — gets a malformed entry instead of the value, and cannot resolve the resource the stack created.
Fix: the CloudFormation DescribeStacks output serializer / output-resolution step must resolve the Ref/GetAtt and always emit OutputValue for a terminal stack (empty string only where AWS emits one), never omit the member.
Failing check: cloudformation__cloudfront.deploy-cloudfront-publickey-and-keygroup-round-trip-and-delete.

## Evidence
Cause 1 — expected: CreateStack 200 + StackId, stack reaches CREATE_COMPLETE. Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CloudFront::KeyValueStore, AWS::CloudFront::ConnectionFunction] / [AWS::CloudFront::ConnectionGroup] / [AWS::CloudFront::MonitoringSubscription].
Cause 2 — expected: after CREATE_COMPLETE, list contains exactly the created resource. Actual: AssertionError: custom cache policies named '<name>': [] ; AssertionError: ListPublicKeys found [] for the control ; AssertionError: origin access controls named '<name>': [] — in each case the stack status asserted CREATE_COMPLETE first and the stack Output supplied the resource id.
Cause 3 — expected: DescribeStacks Outputs == [{OutputKey: ..., OutputValue: ...}]. Actual: KeyError: 'OutputValue' at {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}.

## Steps to Reproduce
Cause 1: CreateStack with a template whose Resources declare AWS::CloudFront::KeyValueStore (or ConnectionFunction / ConnectionGroup / MonitoringSubscription) — the call is refused 400 ValidationError "Template format error: Unrecognized resource types: [...]".
Cause 2: CreateStack with {"Resources": {"Policy": {"Type": "AWS::CloudFront::CachePolicy", "Properties": {"CachePolicyConfig": {"Name": "<n>", "MinTTL": 1, "DefaultTTL": 50, "MaxTTL": 100, "ParametersInCacheKeyAndForwardedToOrigin": {...}}}}}, "Outputs": {"PolicyId": {"Value": {"Ref": "Policy"}}}}; wait for CREATE_COMPLETE; then call cloudfront ListCachePolicies(Type="custom") — the policy is absent. Repeat with AWS::CloudFront::PublicKey (ListPublicKeys) and AWS::CloudFront::OriginAccessControl (ListOriginAccessControls).
Cause 3: CreateStack with an AWS::CloudFront::PublicKey + AWS::CloudFront::KeyGroup template whose Outputs export their ids; wait for CREATE_COMPLETE; call DescribeStacks and read Stacks[0].Outputs — an entry has OutputKey but no OutputValue.
