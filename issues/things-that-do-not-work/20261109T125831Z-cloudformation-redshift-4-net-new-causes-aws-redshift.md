Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__redshift.cfn-redshift-clusterparametergroup-and-snapshotschedule-round-trip-and-delete cloudformation__redshift.deploy-redshift-usagelimit-scheduledaction-snapshot-round-trip-then-delete cloudformation__redshift__sns.cfn-redshift-eventsubscription-invalid-source-type-rejected cloudformation__ec2__redshift.cfn-redshift-clustersubnetgroup-invalid-subnet-rejected cloudformation__ec2__redshift.cfn-redshift-clustersubnetgroup-round-trip-and-delete cloudformation__redshift.cfn-redshift-clusterparametergroup-invalid-family-rejected
Issue-Key: cloudformation__redshift|CreateStack|ValidationError
Issue-Key: cloudformation__ec2__redshift.cfn-redshift-clustersubnetgroup-invalid-subnet-rejected|run#8|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Key: cloudformation__ec2__redshift.cfn-redshift-clustersubnetgroup-round-trip-and-delete|run#11|AssertionError|assert got == [_want(out)], f"subnet groups tagged {stack!r}: {got!r}, expected {[_want(out)]!r}"
Issue-Key: cloudformation__redshift.cfn-redshift-clusterparametergroup-invalid-family-rejected|run#7|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# cloudformation__redshift: 4 net-new causes — AWS::Redshift::SnapshotSchedule/UsageLimit/ScheduledAction/Snapshot/EventSubscription are unregistered types, subnet-group failure reasons and names diverge, and an invalid ParameterGroupFamily reaches CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four net-new root causes in CloudFormation's provisioning of Redshift resources. Each carries its own failing check ids; all four must be closed.

Cause 1 of 4 — AWS::Redshift::SnapshotSchedule, ::UsageLimit, ::ScheduledAction, ::Snapshot and ::EventSubscription are not registered CloudFormation resource types.
AWS: recorded behavior only (the resource-type registry is not part of the Smithy model): CreateStack accepts templates declaring these types, returns a StackId, provisions each through its Redshift API (CreateSnapshotSchedule, CreateUsageLimit, CreateScheduledAction, CreateClusterSnapshot, CreateEventSubscription), resolves Ref/GetAtt to the created resource, and rejects a bad property value at the resource handler (stack ROLLBACK_COMPLETE with a resource failure reason) rather than at template parse time.
Emulator: CreateStack refuses each template with HTTP 400, code ValidationError and message 'Template format error: Unrecognized resource types: [<types>]' — the fixed text is 'Template format error: Unrecognized resource types: [' + the comma-separated list of the template's unregistered types + ']'; the observed lists are [AWS::Redshift::SnapshotSchedule], [AWS::Redshift::UsageLimit, AWS::Redshift::ScheduledAction, AWS::Redshift::Snapshot] and [AWS::Redshift::EventSubscription].
Why: no Redshift schedule, usage limit, scheduled action, snapshot or event subscription can be deployed from a template at all, and a client whose template deploys on AWS gets a template-format error.
Fix: the CloudFormation resource-type registry plus a create/read/delete handler per type, mapped onto the corresponding Redshift operations, with the Redshift refusal surfaced as the resource's failure reason.
Traces: cloudformation__redshift.cfn-redshift-clusterparametergroup-and-snapshotschedule-round-trip-and-delete, cloudformation__redshift.deploy-redshift-usagelimit-scheduledaction-snapshot-round-trip-then-delete, cloudformation__redshift__sns.cfn-redshift-eventsubscription-invalid-source-type-rejected.

Cause 2 of 4 — a failed AWS::Redshift::ClusterSubnetGroup create reports the wrong HandlerErrorCode and an undecorated, rewritten message.
AWS: Smithy: CreateClusterSubnetGroup models InvalidSubnet (400, awsQueryError InvalidSubnet) for a subnet it cannot use. Recorded behavior: CloudFormation wraps that service refusal in the resource status reason as 'Resource handler returned message: "<service message> (Service: Redshift, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: GeneralServiceException)' — the service message for an unusable subnet is 'Some input subnets in :[<subnet-id>] are invalid.', with the subnet id taken verbatim from the template, and the SDK decoration plus HandlerErrorCode GeneralServiceException are fixed for a passed-through service exception.
Emulator: the reason carries message 'Some input subnets in :[<rewritten-subnet-id>] are invalid.' with no '(Service: Redshift, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)' decoration, HandlerErrorCode InvalidRequest instead of GeneralServiceException, and the echoed subnet id is not the one the template supplied (the request's 'subnet-0000000000000000' comes back as 'subnet-<account>0000').
Why: a client reading the stack event cannot tell that Redshift itself refused the subnet (GeneralServiceException vs InvalidRequest changes retry/rollback classification) and the id it is told is invalid is not the id it submitted, so the reason does not identify the offending template value.
Fix: the AWS::Redshift::ClusterSubnetGroup resource handler's error mapping (pass the Redshift exception through with the SDK decoration and GeneralServiceException) and the CreateClusterSubnetGroup InvalidSubnet message, which must echo the requested SubnetIds unmodified.
Traces: cloudformation__ec2__redshift.cfn-redshift-clustersubnetgroup-invalid-subnet-rejected.

Cause 3 of 4 — the name a CFN-provisioned cluster subnet group is stored under is lower-cased, so Ref/GetAtt and the Redshift read disagree.
AWS: recorded behavior only: CloudFormation generates the physical name for an AWS::Redshift::ClusterSubnetGroup as <stack-name>-<LogicalId>-<suffix>, preserving the logical id's casing, and that exact string is what Ref and Fn::GetAtt ClusterSubnetGroupName return AND what DescribeClusterSubnetGroups reports as ClusterSubnetGroupName — the three agree.
Emulator: Ref/GetAtt report the generated mixed-case name while DescribeClusterSubnetGroups reports the same name lower-cased, so a read-back by the Ref value does not match the listed group's ClusterSubnetGroupName.
Why: a client that provisions a subnet group in a stack and then addresses it by the stack's Ref/GetAtt output reads a name Redshift does not report (and a not-found lookup by the described name is a different string again), so templates that chain a subnet group into a cluster cannot resolve it reliably.
Fix: the AWS::Redshift::ClusterSubnetGroup handler and Redshift's cluster-subnet-group state must agree on one stored identifier — the name reported by Ref/GetAtt must be the name DescribeClusterSubnetGroups echoes. Note the tension with open issue 20261108T062410Z-redshift-cluster-subnet-group-identifiers-are-not-normalized.md, which asks for lowercase normalization in the ClusterSubnetGroupNotFoundFault message; whichever normalization is adopted must be applied consistently so the create, the Ref and the read all report the same string.
Traces: cloudformation__ec2__redshift.cfn-redshift-clustersubnetgroup-round-trip-and-delete.

Cause 4 of 4 — an AWS::Redshift::ClusterParameterGroup declaring an invalid ParameterGroupFamily is provisioned successfully instead of failing the stack.
AWS: recorded behavior only: CreateClusterParameterGroup refuses an unknown family, so the stack's resource fails and the stack settles ROLLBACK_COMPLETE with reason 'Resource handler returned message: "ParameterGroupFamily <family> is not a valid parameter group family (Service: Redshift, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: GeneralServiceException)' — fixed text 'ParameterGroupFamily ' + the requested family + ' is not a valid parameter group family' — and the declared group does not exist afterwards (DescribeClusterParameterGroups answers ClusterParameterGroupNotFound).
Emulator: the stack reaches CREATE_COMPLETE; the family is never validated, so the invalid-value leg produces a provisioned group instead of a rollback and no failure reason exists to read.
Why: a template carrying a typo'd or unsupported engine family deploys green and leaves an unusable parameter group behind, and a client relying on CloudFormation to reject bad values at create time gets no signal.
Fix: Redshift's CreateClusterParameterGroup validation (the ParameterGroupFamily domain) and the AWS::Redshift::ClusterParameterGroup handler that must surface that refusal as a resource failure.
Traces: cloudformation__redshift.cfn-redshift-clusterparametergroup-invalid-family-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 (cloudformation__redshift: 0/6 passed).

Cause 1: 'An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Redshift::SnapshotSchedule]' (cfn-redshift-clusterparametergroup-and-snapshotschedule-round-trip-and-delete); '... Unrecognized resource types: [AWS::Redshift::UsageLimit, AWS::Redshift::ScheduledAction, AWS::Redshift::Snapshot]' (deploy-redshift-usagelimit-scheduledaction-snapshot-round-trip-then-delete); '... Unrecognized resource types: [AWS::Redshift::EventSubscription]' (cfn-redshift-eventsubscription-invalid-source-type-rejected).

Cause 2: "AssertionError: handler reason ('Some input subnets in :[subnet-<account>0000] are invalid.', 'InvalidRequest'), expected ('Some input subnets in :[subnet-0000000000000000] are invalid. (Service: Redshift, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'GeneralServiceException')" — the request id / attempt count are placeholders the check normalizes; the divergences are the missing SDK decoration, the HandlerErrorCode and the rewritten subnet id.

Cause 3: "AssertionError: subnet groups tagged 'sim2real-cfnrs-csg-1f013aedcc4f': [('sim2real-cfnrs-csg-1f013aedcc4f-subnetgroup-93c7f129f5a8', 'sim2real behavioral cluster subnet group', 'vpc-...', ['subnet-...'])], expected [('sim2real-cfnrs-csg-1f013aedcc4f-SubnetGroup-93C7F129F5A8', ...)]" — the expected name is the stack's own Ref output, i.e. Ref and DescribeClusterSubnetGroups disagree in case. The stack name and hex suffix are this run's values.

Cause 4: "AssertionError: rejected stack ended 'CREATE_COMPLETE'" for a template whose ParameterGroupFamily is 'redshift-9.9' (the check then expects the resource reason 'ParameterGroupFamily redshift-9.9 is not a valid parameter group family (Service: Redshift, Status Code: 400, ...)' with HandlerErrorCode GeneralServiceException, and ClusterParameterGroupNotFound for the declared name); the positive control uses family 'redshift-2.0'.

## Steps to Reproduce
1. CreateStack with a template declaring AWS::Redshift::SnapshotSchedule (then one declaring AWS::Redshift::UsageLimit + ::ScheduledAction + ::Snapshot, then one declaring AWS::Redshift::EventSubscription): each is refused with 400 ValidationError 'Template format error: Unrecognized resource types: [...]' (cause 1).
2. CreateStack with a VPC, a subnet and an AWS::Redshift::ClusterSubnetGroup whose SubnetIds name an unusable subnet; wait for ROLLBACK_COMPLETE and read the resource's ResourceStatusReason: the handler message lacks the '(Service: Redshift, Status Code: 400, ...)' decoration, HandlerErrorCode is InvalidRequest, and the echoed subnet id is not the submitted one (cause 2).
3. CreateStack with a VPC, a subnet and an AWS::Redshift::ClusterSubnetGroup with no explicit name; at CREATE_COMPLETE compare Ref/Fn::GetAtt ClusterSubnetGroupName with the ClusterSubnetGroupName in DescribeClusterSubnetGroups (tag-filtered): the described name is lower-cased (cause 3).
4. CreateStack with an AWS::Redshift::ClusterParameterGroup whose ParameterGroupFamily is 'redshift-9.9': the stack reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE (cause 4).
5. Re-replay the checks listed in trace_ids.
