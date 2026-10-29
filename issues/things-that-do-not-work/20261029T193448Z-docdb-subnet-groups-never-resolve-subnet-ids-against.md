Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-db-subnet-group-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-db-subnet-group-lifecycle docdb-cluster-create-validation-errors
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.Subnets.Subnet[*]
Issue-Key: docdb|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.SupportedNetworkTypes
Issue-Key: docdb|CreateDBSubnetGroup|2xx->2xx|$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.VpcId
Issue-Key: docdb|ModifyDBSubnetGroup|2xx->2xx|$.ModifyDBSubnetGroupResponse.ModifyDBSubnetGroupResult.DBSubnetGroup.Subnets.Subnet[*]
Issue-Key: docdb|ModifyDBSubnetGroup|2xx->2xx|$.ModifyDBSubnetGroupResponse.ModifyDBSubnetGroupResult.DBSubnetGroup.SupportedNetworkTypes
Issue-Key: docdb|ModifyDBSubnetGroup|2xx->2xx|$.ModifyDBSubnetGroupResponse.ModifyDBSubnetGroupResult.DBSubnetGroup.VpcId
Issue-Key: docdb|DescribeDBSubnetGroups|2xx->2xx|$.DescribeDBSubnetGroupsResponse.DescribeDBSubnetGroupsResult.DBSubnetGroups.DBSubnetGroup.Subnets.Subnet[*]
Issue-Key: docdb|DescribeDBSubnetGroups|2xx->2xx|$.DescribeDBSubnetGroupsResponse.DescribeDBSubnetGroupsResult.DBSubnetGroups.DBSubnetGroup.SupportedNetworkTypes
Issue-Key: docdb|DescribeDBSubnetGroups|2xx->2xx|$.DescribeDBSubnetGroupsResponse.DescribeDBSubnetGroupsResult.DBSubnetGroups.DBSubnetGroup.VpcId
Issue-Key: docdb|CreateDBSubnetGroup|4xx->2xx|$.ErrorResponse
Issue-Key: docdb|CreateDBSubnetGroup|4xx->2xx|status
Issue-Commit: b357a994596866b4c3c5b5df5566beb9dd5436ae

# DocDB subnet groups never resolve subnet ids against EC2 state

> _Note: this issue was identified against the code base at commit `b357a994596866b4c3c5b5df5566beb9dd5436ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
This issue bundles FOUR causes that all follow from one fault: the docdb subnet-group handlers store the caller's subnet-id strings verbatim and never resolve them against the account/Region EC2 subnet state. Because nothing is resolved, the DBSubnetGroup projection cannot carry the VPC or the per-subnet Availability Zone, and an unknown subnet id cannot be refused. A fixer must close ALL FOUR causes below before submitting.

Cause 1 of 4: CreateDBSubnetGroup response omits VpcId, SupportedNetworkTypes and per-subnet AZ.
AWS: Smithy (com.amazonaws.docdb#AmazonRDSv19) CreateDBSubnetGroup returns CreateDBSubnetGroupResult{DBSubnetGroup}. The DBSubnetGroup shape models VpcId ("Provides the virtual private cloud (VPC) ID of the subnet group"), SupportedNetworkTypes (NetworkTypeList, "Valid Values: IPV4 | DUAL") and Subnets (SubnetList of Subnet{SubnetIdentifier, SubnetAvailabilityZone, SubnetStatus}, SubnetAvailabilityZone documented as "Specifies the Availability Zone for the subnet"). AWS resolves every requested subnet id, so the awsQuery envelope carries VpcId = the VPC that owns the requested subnets, SupportedNetworkTypes with one member per protocol family the subnets support (IPV4 for IPv4-only subnets), and for each subnet an SubnetAvailabilityZone/Name holding that subnet's zone. The recording additionally shows a SubnetOutpost element on each Subnet, empty for a non-Outpost subnet (recorded behavior only - SubnetOutpost is not in the on-disk docdb Subnet shape).
Emulator: the 200 response's DBSubnetGroup has no VpcId element and no SupportedNetworkTypes element, and every Subnets.Subnet entry carries only {SubnetIdentifier, SubnetStatus} - no SubnetAvailabilityZone and no SubnetOutpost.
Why: a client that picks security groups, asserts AZ spread, or decides IPv4-vs-DUAL networking from the created group reads nothing at all; the VPC and zone of the group are unrecoverable from the API.
Fix: the docdb CreateDBSubnetGroup handler's state record must hold the resolved VPC id and the resolved AZ of each subnet (looked up in the EC2 subnet store), and the DBSubnetGroup serializer must project VpcId, SupportedNetworkTypes and Subnet.SubnetAvailabilityZone/SubnetOutpost.

Cause 2 of 4: ModifyDBSubnetGroup response omits the same members.
AWS: ModifyDBSubnetGroup returns the same DBSubnetGroup shape, so VpcId, SupportedNetworkTypes and per-subnet SubnetAvailabilityZone/SubnetOutpost are present after a modify exactly as after a create.
Emulator: the 200 ModifyDBSubnetGroupResult.DBSubnetGroup again has no VpcId, no SupportedNetworkTypes, and Subnets entries of only {SubnetIdentifier, SubnetStatus}.
Why: the same client read breaks after any edit of the group, even if the create path were fixed in isolation.
Fix: same projection, applied on the modify path - the handler must re-resolve the new subnet list and rewrite the resolved VPC/AZ into state rather than overwriting it with raw ids.

Cause 3 of 4: DescribeDBSubnetGroups omits the same members on the read path.
AWS: DescribeDBSubnetGroups returns DBSubnetGroupMessage{Marker, DBSubnetGroups} of the identical DBSubnetGroup shape, so a described group carries VpcId, SupportedNetworkTypes and per-subnet zone.
Emulator: each described DBSubnetGroup lacks VpcId and SupportedNetworkTypes and its Subnets entries carry only {SubnetIdentifier, SubnetStatus}.
Why: the read path is how clients discover existing networking, so the gap persists for groups they did not create in the same session.
Fix: share one DBSubnetGroup serializer between the create/modify/describe handlers so the resolved fields are projected once.

Cause 4 of 4: CreateDBSubnetGroup accepts a subnet id that does not exist (200 instead of 400).
AWS: because the service resolves each SubnetIds member, a request naming an unknown subnet id is rejected. The recorded response is HTTP 400 with awsQuery ErrorResponse.Error{Type 'Sender', Code 'InvalidParameterValue'} and Message whose fixed text is "Some input subnets in :[" + "] are invalid." around the variable part: the comma-separated list of the requested subnet ids AWS considered invalid, taken from the request. (The model also binds InvalidSubnet for this operation; the recorded code for an unresolvable id is InvalidParameterValue.)
Emulator: returns HTTP 200 and creates the group, with no ErrorResponse at all.
Why: a typo'd or cross-VPC subnet id is silently accepted and the group looks usable; the client's error branch for InvalidParameterValue never fires and the bad group is later attached to a cluster.
Fix: CreateDBSubnetGroup input validation must look up every SubnetIds member in the EC2 subnet store and raise the 400 InvalidParameterValue above (without writing any group to state) when one or more cannot be resolved.

Related open issue: '20261029T172617Z-docdb-createdbsubnetgroup-never-resolves-subnets-against-ec2-state' reports the AZ-coverage precondition (DBSubnetGroupDoesNotCoverEnoughAZs) from the behavioral corpus. It does not carry these wire replay keys and does not cover the unknown-subnet refusal, the modify path, or the describe path, so these causes are filed here.

Traces: docdb-db-subnet-group-lifecycle#task#5 (Cause 1), docdb-db-subnet-group-lifecycle#task#9 (Cause 2), docdb-cluster-create-validation-errors#task#11 and docdb-db-subnet-group-lifecycle#task#6 (Cause 3), docdb-db-subnet-group-lifecycle#task#10 (Cause 4).

## Evidence
Cause 1 - fixture docdb-db-subnet-group-lifecycle step task#5, CreateDBSubnetGroup, expected 200 / actual 200, body mismatch: '$.CreateDBSubnetGroupResponse.CreateDBSubnetGroupResult.DBSubnetGroup.SupportedNetworkTypes: missing (expected {member: IPV4}, got None)'; '....DBSubnetGroup.VpcId: missing (expected a vpc id, got None)'; '....Subnets.Subnet[0]: expected {SubnetIdentifier, SubnetStatus: Active, SubnetOutpost: "", SubnetAvailabilityZone: {Name: <zone>}}, got {SubnetIdentifier, SubnetStatus: Active}' (same for Subnet[1]). The concrete vpc-/subnet- ids and zone names in the recording are per-environment values, not requirements.
Cause 2 - fixture docdb-db-subnet-group-lifecycle step task#9, ModifyDBSubnetGroup, 200/200: identical set of missing keys under $.ModifyDBSubnetGroupResponse.ModifyDBSubnetGroupResult.DBSubnetGroup (SupportedNetworkTypes, VpcId, Subnet[*].SubnetAvailabilityZone/SubnetOutpost).
Cause 3 - fixture docdb-cluster-create-validation-errors step task#11 (also docdb-db-subnet-group-lifecycle#task#6), DescribeDBSubnetGroups, 200/200: identical missing keys under $.DescribeDBSubnetGroupsResponse.DescribeDBSubnetGroupsResult.DBSubnetGroups.DBSubnetGroup.
Cause 4 - fixture docdb-db-subnet-group-lifecycle step task#10, CreateDBSubnetGroup: 'status: expected 400, got 200' and '$.ErrorResponse: missing (expected {Error: {Type: Sender, Code: InvalidParameterValue, Message: "Some input subnets in :[<requested subnet ids>] are invalid."}}, got None)'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture docdb-db-subnet-group-lifecycle, step task#5 - Action=CreateDBSubnetGroup with a description and two existing subnet ids in different AZs of one VPC. Observe the 200 DBSubnetGroup has no VpcId, no SupportedNetworkTypes, and no SubnetAvailabilityZone/SubnetOutpost on either Subnets entry.
3. Cause 2: same fixture, step task#9 - Action=ModifyDBSubnetGroup on that group; the returned DBSubnetGroup shows the same omissions.
4. Cause 3: fixture docdb-cluster-create-validation-errors, step task#11 (or docdb-db-subnet-group-lifecycle step task#6) - Action=DescribeDBSubnetGroups for that name; same omissions on the read path.
5. Cause 4: fixture docdb-db-subnet-group-lifecycle, step task#10 - Action=CreateDBSubnetGroup with SubnetIds containing one real subnet id and one id that does not exist. Observed: HTTP 200 and a created group. Expected: HTTP 400, Error.Type Sender, Error.Code InvalidParameterValue, Message "Some input subnets in :[<the requested subnet ids>] are invalid.", and no group written (a following DescribeDBSubnetGroups must not list the name).
