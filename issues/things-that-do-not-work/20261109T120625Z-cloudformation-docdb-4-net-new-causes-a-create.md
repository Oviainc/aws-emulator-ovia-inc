Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__docdb.deploy-docdb-dbclusterparametergroup-and-dbsubnetgroup-round-trip-and-delete cloudformation__docdb.deploy-docdb-dbsubnetgroup-single-az-rejected-and-rolled-back cloudformation__docdb.deploy-docdb-eventsubscription-round-trip-and-delete cloudformation__docdb.deploy-docdb-globalcluster-round-trip-and-delete
Issue-Key: cloudformation__docdb.deploy-docdb-dbclusterparametergroup-and-dbsubnetgroup-round-trip-and-delete|run#15|AssertionError|assert len(groups) == 1, (
Issue-Key: cloudformation__docdb.deploy-docdb-dbsubnetgroup-single-az-rejected-and-rolled-back|run#6|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: cloudformation__docdb|CreateStack|ValidationError
Issue-Commit: 70b7c68145c683ae49b96a2a3a7ef8edad56294d

# cloudformation__docdb: 4 net-new causes — a CREATE_COMPLETE AWS::DocDB::DBClusterParameterGroup does not exist in DocDB, a single-AZ AWS::DocDB::DBSubnetGroup is not refused, and AWS::DocDB::EventSubscription / ::GlobalCluster are unregistered resource types

> _Note: this issue was identified against the code base at commit `70b7c68145c683ae49b96a2a3a7ef8edad56294d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four net-new root causes in CloudFormation's AWS::DocDB::* resource providers. All four are reported together because they share the cloudformation__docdb service; each is numbered and carries its own failing check ids.

---
Cause 1 of 4: a stack's AWS::DocDB::DBClusterParameterGroup reaches CREATE_COMPLETE without the group ever existing in DocDB.
Failing check: cloudformation__docdb.deploy-docdb-dbclusterparametergroup-and-dbsubnetgroup-round-trip-and-delete

AWS: the AWS::DocDB::DBClusterParameterGroup resource provider provisions the group through DocDB's own CreateDBClusterParameterGroup. Smithy (com.amazonaws.docdb#AmazonRDSv19) binds CreateDBClusterParameterGroup with required input {DBClusterParameterGroupName, DBParameterGroupFamily, Description} plus Tags, returning a DBClusterParameterGroup; DescribeDBClusterParameterGroups (paginated on Marker, items DBClusterParameterGroups) then lists it, and DescribeDBClusterParameters (required DBClusterParameterGroupName, items Parameters) returns its parameters with ParameterName/ParameterValue. A group CloudFormation reports CREATE_COMPLETE is therefore readable through DocDB: it appears exactly once in the DescribeDBClusterParameterGroups listing under the template's Name, with DBParameterGroupFamily and Description from the template, a DBClusterParameterGroupArn whose ListTagsForResource carries the template's Tags plus the stack's propagated tags, and each template Parameters entry readable through DescribeDBClusterParameters at its templated value. Resource-type reference: https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-docdb-dbclusterparametergroup.html

Emulator: the stack (AWS::EC2::VPC + two AWS::EC2::Subnet + AWS::DocDB::DBClusterParameterGroup + AWS::DocDB::DBSubnetGroup) reaches CREATE_COMPLETE and DescribeStacks returns the Outputs, but the full DescribeDBClusterParameterGroups listing contains no group with the template's Name: the check's filtered listing is the empty list. The resource is tracked by CloudFormation but never materialized in DocDB state, so nothing downstream of it (Family, Description, tags on DBClusterParameterGroupArn, the template's Parameters value via DescribeDBClusterParameters, and the post-delete DBParameterGroupNotFound/'DBClusterParameterGroup not found: <name>' refusal) is observable either.

Why: a client that deploys its DocumentDB cluster parameter group as IaC and then reads or attaches it by name through DocDB's API finds nothing, even though the stack reports success. Ref returns the name, so the template looks healthy while the parameter group it was supposed to create does not exist — the worst failure mode for infrastructure-as-code.

Fix: the CloudFormation AWS::DocDB::DBClusterParameterGroup resource provider must perform the create against DocDB's own state (CreateDBClusterParameterGroup with the template's Name/Family/Description, then the template's Parameters applied as a ModifyDBClusterParameterGroup) and the delete against it as well, so DocDB's describe handlers and the docdb state store are the single source of truth; it must not report CREATE_COMPLETE for a group that was not written.

---
Cause 2 of 4: an AWS::DocDB::DBSubnetGroup naming subnets in a single Availability Zone is accepted, so the stack reaches CREATE_COMPLETE instead of failing the resource and rolling back.
Failing check: cloudformation__docdb.deploy-docdb-dbsubnetgroup-single-az-rejected-and-rolled-back

AWS: DocDB models DBSubnetGroupDoesNotCoverEnoughAZs (client error, httpError 400) documented as "Subnets in the subnet group should cover at least two Availability Zones unless there is only one Availability Zone", and CreateDBSubnetGroup resolves the supplied SubnetIds against EC2 to evaluate it. A CloudFormation stack whose AWS::DocDB::DBSubnetGroup names subnets covering one AZ therefore fails that resource and rolls the stack back: the stack ends ROLLBACK_COMPLETE, DescribeEvents reports no VALIDATION_ERROR OperationEvents rows (the refusal comes from DocDB at provision time, not from early template validation), and the single CREATE_FAILED StackEvent for the resource carries a resource-handler ResourceStatusReason whose message is the fixed text "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: " + the AZ names the requested subnets actually cover + ". Add subnets to cover at least 2 AZs. (Service: Rds, Status Code: 400, Request ID: " + the request id + ") (SDK Attempt Count: " + the attempt count + ")", with HandlerErrorCode "InvalidRequest". The AZ name, request id and attempt count are values from the request/state and the run; the rest is fixed text. After the rollback DocDB must have no group by that name: DescribeDBSubnetGroups(DBSubnetGroupName=<name>) answers DBSubnetGroupNotFoundFault with "DB Subnet group '<name>' not found."

Emulator: the single-AZ stack reaches CREATE_COMPLETE — "AssertionError: rejected stack ended 'CREATE_COMPLETE'". No resource is refused, no CREATE_FAILED row with the AZ-coverage handler reason is emitted, and no rollback happens, so neither the handler reason/HandlerErrorCode nor the post-rollback DocDB absence can be observed. The underlying DocDB-level precondition is the one already reported on the docdb service by the open issue 20261029T172617Z-docdb-createdbsubnetgroup-never-resolves-subnets-against-ec2-state (CreateDBSubnetGroup never resolves SubnetIds against EC2 state, so it cannot evaluate AZ coverage) and by 20261029T193448Z-docdb-subnet-groups-never-resolve-subnet-ids-against; neither covers this check, which is the CloudFormation-side consequence: the resource provider must surface that refusal as a CREATE_FAILED row with the resource-handler reason shape above and roll the stack back.

Why: a client relying on CloudFormation to catch a mis-specified SubnetIds list before a DocumentDB cluster is built gets a stack that reports success over an invalid subnet group, and any automation branching on ROLLBACK_COMPLETE or on HandlerErrorCode InvalidRequest never fires.

Fix: the docdb CreateDBSubnetGroup handler's validation must resolve the SubnetIds against EC2 state and raise DBSubnetGroupDoesNotCoverEnoughAZs (400) as described, and the CloudFormation AWS::DocDB::DBSubnetGroup resource provider must map that service refusal into a CREATE_FAILED resource event whose ResourceStatusReason is the resource-handler wrapper (message as templated, HandlerErrorCode InvalidRequest) and drive the stack to ROLLBACK_COMPLETE, leaving no group in docdb state.

---
Cause 3 of 4: AWS::DocDB::EventSubscription is not a registered CloudFormation resource type, so CreateStack refuses the whole template.
Failing check: cloudformation__docdb.deploy-docdb-eventsubscription-round-trip-and-delete

AWS: AWS::DocDB::EventSubscription is a supported CloudFormation resource type backed by DocDB's CreateEventSubscription, which the model binds with required input {SubscriptionName, SnsTopicArn} plus SourceType/EventCategories/SourceIds/Enabled/Tags and the errors SNSTopicArnNotFound (404), SNSInvalidTopic (400), SubscriptionAlreadyExist (400), SourceNotFound (404), SubscriptionCategoryNotFound (404) and EventSubscriptionQuotaExceeded (400). CreateStack over a template declaring it is accepted (HTTP 200, a StackId returned) and the stack provisions the subscription, which then reads back through DocDB's DescribeEventSubscriptions.

Emulator: CreateStack is refused — HTTP 400, Error.Code "ValidationError", Error.Message "Template format error: Unrecognized resource types: [AWS::DocDB::EventSubscription]". The fixed text here is "Template format error: Unrecognized resource types: [" + the type names + "]"; the type name is the value from the request. The stack is never created, so no part of the event-subscription round trip or its delete is exercised.

Why: no DocumentDB event subscription can be provisioned as IaC at all; a template that is valid on AWS is rejected outright, and clients cannot deploy DocumentDB event notifications.

Fix: register AWS::DocDB::EventSubscription in CloudFormation's resource-type registry and implement its provider (create/read/update/delete) over DocDB's CreateEventSubscription / DescribeEventSubscriptions / ModifyEventSubscription / DeleteEventSubscription, including Ref and the template's Tags.

---
Cause 4 of 4: AWS::DocDB::GlobalCluster is not a registered CloudFormation resource type, so CreateStack refuses the whole template.
Failing check: cloudformation__docdb.deploy-docdb-globalcluster-round-trip-and-delete

AWS: AWS::DocDB::GlobalCluster is a supported CloudFormation resource type backed by DocDB's CreateGlobalCluster, which the model binds with required GlobalClusterIdentifier (length 1-255, pattern ^[A-Za-z][0-9A-Za-z-:._]*$) plus SourceDBClusterIdentifier/Engine/EngineVersion/DeletionProtection/DatabaseName/StorageEncrypted and the errors DBClusterNotFoundFault (404), GlobalClusterAlreadyExistsFault (400), GlobalClusterQuotaExceededFault (400) and InvalidDBClusterStateFault (400). CreateStack over a template declaring it is accepted (HTTP 200 with a StackId) and the global cluster reads back through DescribeGlobalClusters.

Emulator: CreateStack is refused — HTTP 400, Error.Code "ValidationError", Error.Message "Template format error: Unrecognized resource types: [AWS::DocDB::GlobalCluster]" (same fixed text as Cause 3, with the requested type name as the variable part). The stack is never created, so the global-cluster round trip and delete are unexercised.

Why: no DocumentDB global cluster can be declared in a template; multi-region DocumentDB topologies are undeployable through CloudFormation even though the underlying DocDB operation is modeled.

Fix: register AWS::DocDB::GlobalCluster in CloudFormation's resource-type registry and implement its provider over DocDB's CreateGlobalCluster / DescribeGlobalClusters / ModifyGlobalCluster / DeleteGlobalCluster (with RemoveFromGlobalCluster on delete where members remain), including Ref and GetAtt.

---
Traces (examples of the four causes): cloudformation__docdb.deploy-docdb-dbclusterparametergroup-and-dbsubnetgroup-round-trip-and-delete (Cause 1), cloudformation__docdb.deploy-docdb-dbsubnetgroup-single-az-rejected-and-rolled-back (Cause 2), cloudformation__docdb.deploy-docdb-eventsubscription-round-trip-and-delete (Cause 3), cloudformation__docdb.deploy-docdb-globalcluster-round-trip-and-delete (Cause 4).

## Evidence
Cause 1 — cloudformation__docdb.deploy-docdb-dbclusterparametergroup-and-dbsubnetgroup-round-trip-and-delete [mismatch]:
  File "check_cloudformation__docdb_deploy_docdb_dbclusterparametergroup_and_dbsubnetgroup_round_trip_and_delete.py", line 235, in run
    assert len(groups) == 1, (...)
  AssertionError: DescribeDBClusterParameterGroups named 'sim2real-docdbgrp-pg-fd4c53316ecf': []
The stack had already passed `assert status == "CREATE_COMPLETE"` and its Outputs were read, so CloudFormation reported the AWS::DocDB::DBClusterParameterGroup (Name=<templated>, Family='docdb5.0', Description='sim2real docdb cluster parameter group', Parameters={'audit_logs':'enabled'}) as created.
Expected (AWS): exactly one entry for that name across all DescribeDBClusterParameterGroups pages, with DBParameterGroupFamily 'docdb5.0' and the template's Description; its ARN's ListTagsForResource carrying the template and stack tags; DescribeDBClusterParameters(<name>) containing audit_logs='enabled'; after DeleteStack, DescribeDBClusterParameterGroups(DBClusterParameterGroupName=<name>) answering DBParameterGroupNotFound / "DBClusterParameterGroup not found: <name>".
Actual (emulator): the listing contains no such group — [].
The generated group name is a per-run value.

Cause 2 — cloudformation__docdb.deploy-docdb-dbsubnetgroup-single-az-rejected-and-rolled-back [mismatch]:
  File "check_cloudformation__docdb_deploy_docdb_dbsubnetgroup_single_az_rejected_and_rolled_back.py", line 246, in run
    assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
  AssertionError: rejected stack ended 'CREATE_COMPLETE'
Template: VPC + SubnetA (AZ[0]) + SubnetB (AZ[1]) + AWS::DocDB::DBSubnetGroup with SubnetIds=[SubnetA] only (one AZ).
Expected (AWS): stack ends ROLLBACK_COMPLETE; DescribeEvents has no VALIDATION_ERROR rows; the single CREATE_FAILED StackEvent for logical id 'SubnetGroup' carries a resource-handler reason whose (message, HandlerErrorCode) is ("The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: <az>. Add subnets to cover at least 2 AZs. (Service: Rds, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)", "InvalidRequest"); then DescribeDBSubnetGroups(DBSubnetGroupName=<name>) answers DBSubnetGroupNotFoundFault / "DB Subnet group '<name>' not found."; and the same template with SubnetIds=[SubnetA, SubnetB] reaches CREATE_COMPLETE with SubnetGroupStatus 'Complete' and both (SubnetIdentifier, SubnetAvailabilityZone.Name) pairs.
Actual (emulator): CREATE_COMPLETE — the invalid group was accepted; <az>, <id> and <n> above are per-run values, the surrounding sentence is fixed text.

Cause 3 — cloudformation__docdb.deploy-docdb-eventsubscription-round-trip-and-delete [error]:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::DocDB::EventSubscription]
Expected (AWS): CreateStack returns 200 with a StackId and the stack provisions the subscription.
Actual (emulator): HTTP 400, Error.Code 'ValidationError', the message above.

Cause 4 — cloudformation__docdb.deploy-docdb-globalcluster-round-trip-and-delete [error]:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::DocDB::GlobalCluster]
Expected (AWS): CreateStack returns 200 with a StackId and the stack provisions the global cluster.
Actual (emulator): HTTP 400, Error.Code 'ValidationError', the message above.

## Steps to Reproduce
Boot the emulator from this checkout, then:

Cause 1: cloudformation CreateStack with a template declaring AWS::EC2::VPC (10.121.0.0/16), two AWS::EC2::Subnet in Fn::Select[0] and [1] of Fn::GetAZs, an AWS::DocDB::DBClusterParameterGroup {Name:<unique>, Family:'docdb5.0', Description:'sim2real docdb cluster parameter group', Parameters:{audit_logs:'enabled'}, Tags:[...]}, and an AWS::DocDB::DBSubnetGroup over both subnets; Outputs Ref both groups. Wait for CREATE_COMPLETE. Then docdb DescribeDBClusterParameterGroups across all pages and filter for the templated Name. Observed: no match ([]). Expected: exactly one entry, with the template's Family and Description; its ARN's ListTagsForResource carries the template + stack tags; DescribeDBClusterParameters(<name>) contains audit_logs='enabled'; after DeleteStack both groups are gone and DocDB refuses them by name (DBParameterGroupNotFound / DBSubnetGroupNotFoundFault).

Cause 2: cloudformation CreateStack with a template declaring VPC (10.130.0.0/16), SubnetA in Fn::Select[0] of Fn::GetAZs, SubnetB in Fn::Select[1], and an AWS::DocDB::DBSubnetGroup whose SubnetIds is [{Ref: SubnetA}] only. Wait for a terminal status. Observed: CREATE_COMPLETE. Expected: ROLLBACK_COMPLETE, no VALIDATION_ERROR OperationEvents, one CREATE_FAILED event on 'SubnetGroup' with the resource-handler AZ-coverage message and HandlerErrorCode InvalidRequest, and DescribeDBSubnetGroups(<name>) refusing with DBSubnetGroupNotFoundFault "DB Subnet group '<name>' not found.". Control: the same template with SubnetIds=[SubnetA, SubnetB] must reach CREATE_COMPLETE and read back through DescribeDBSubnetGroups with SubnetGroupStatus 'Complete' and both subnets' AZ names.

Cause 3: cloudformation CreateStack with a template declaring an AWS::DocDB::EventSubscription (SubscriptionName, SnsTopicArn of a stack-declared SNS topic). Observed: 400 ValidationError "Template format error: Unrecognized resource types: [AWS::DocDB::EventSubscription]". Expected: 200 with a StackId; the subscription then reads back through docdb DescribeEventSubscriptions and is removed by DeleteStack.

Cause 4: cloudformation CreateStack with a template declaring an AWS::DocDB::GlobalCluster (GlobalClusterIdentifier=<unique>, Engine='docdb'). Observed: 400 ValidationError "Template format error: Unrecognized resource types: [AWS::DocDB::GlobalCluster]". Expected: 200 with a StackId; the global cluster then reads back through docdb DescribeGlobalClusters and is removed by DeleteStack.
