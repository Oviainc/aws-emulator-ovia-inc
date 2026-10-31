Issue-Source: trace-sweep-divergence
Issue-Trace: instance-profile-and-subnet-group-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: instance-profile-and-subnet-group-lifecycle
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms|CreateReplicationSubnetGroup|2xx->2xx|$.ReplicationSubnetGroup.ReplicationSubnetGroupArn
Issue-Key: dms|CreateReplicationSubnetGroup|2xx->2xx|$.ReplicationSubnetGroup.SupportedNetworkTypes
Issue-Commit: d731633d9e655898fab102cd9a4c0063529f6263

# DMS CreateReplicationSubnetGroup response omits ReplicationSubnetGroupArn and SupportedNetworkTypes

> _Note: this issue was identified against the code base at commit `d731633d9e655898fab102cd9a4c0063529f6263`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationSubnetGroup

## Description
One cause.

Cause 1 of 1: DMS CreateReplicationSubnetGroup returns a ReplicationSubnetGroup structure that is missing two of the members AWS populates — the subnet group's ARN and its supported network types. Fixture `instance-profile-and-subnet-group-lifecycle`, step `task#7`.

AWS: Smithy (com.amazonaws.databasemigrationservice#AmazonDMSv20160101, awsJson1_1, arnNamespace `dms`): CreateReplicationSubnetGroup takes required ReplicationSubnetGroupIdentifier, ReplicationSubnetGroupDescription and SubnetIds, and returns CreateReplicationSubnetGroupResponse with a single member `ReplicationSubnetGroup` (shape ReplicationSubnetGroup). The modeled ReplicationSubnetGroup member `SupportedNetworkTypes` (target StringList) is documented as "The IP addressing protocol supported by the subnet group"; on a successful create of an IPv4-only subnet group AWS returns it populated — the recording shows `["IPV4"]`. `ReplicationSubnetGroupArn` is not a member of the ReplicationSubnetGroup shape in this on-disk model version, so for that field this is recorded behavior only: the recorded 200 response carries `ReplicationSubnetGroup.ReplicationSubnetGroupArn` with the DMS subnet-group ARN for the created resource, of the form `arn:aws:dms:<region>:<account-id>:subgrp:<identifier>` — fixed text `arn:aws:dms:`, `:subgrp:` and the partition/namespace, with region, account id and the trailing identifier taken from the request's ReplicationSubnetGroupIdentifier and the caller's credentials/endpoint (the recording's `us-west-2`, `000000000000` and `sim2real-probe-dms-sng-31aec5ecbf` are that run's values filling the template, not requirements).

Emulator: the emulator answers HTTP 200 with a ReplicationSubnetGroup object, but both `$.ReplicationSubnetGroup.ReplicationSubnetGroupArn` and `$.ReplicationSubnetGroup.SupportedNetworkTypes` are absent (replay reports `got None` for both against expected ARN and `["IPV4"]`). Status, error code and the rest of the structure match; only these two members are never emitted.

Why: the ARN is the resource handle for a replication subnet group. A client that creates a subnet group and then tags it, references it from a replication instance/config by ARN, or stores it for later lookup gets a null/absent ARN and either crashes on deserialization of a non-optional field in strongly typed SDKs or silently propagates an empty ARN into subsequent AddTagsToResource/ListTagsForResource/Describe* calls. The missing SupportedNetworkTypes likewise breaks clients that branch on IPv4 vs dual-stack capability before creating a replication instance in the group, since absent is indistinguishable from "unknown".

Fix: in the DMS CreateReplicationSubnetGroup handler/serializer and the subnet-group state record — the created subnet group must be stored with a derived ARN (`arn:aws:dms:<region>:<account-id>:subgrp:<ReplicationSubnetGroupIdentifier>`, built from the request identifier plus the emulator's configured region and account) and with SupportedNetworkTypes defaulted to the IPv4-only list, and both members must be serialized on the create response as well as on the corresponding DescribeReplicationSubnetGroups/ModifyReplicationSubnetGroup responses that return the same shape, so the value is stable across the resource's lifecycle.

Traces: fixture `instance-profile-and-subnet-group-lifecycle`, step `task#7` (cluster member: instance-profile-and-subnet-group-lifecycle#task#7).

## Evidence
Divergence cluster service=dms|CreateReplicationSubnetGroup|status=200->200.
Expected status 200, actual status 200 (status matches; body diverges).

Expected (recorded AWS response body, 200):
  $.ReplicationSubnetGroup.ReplicationSubnetGroupArn = "arn:aws:dms:us-west-2:000000000000:subgrp:sim2real-probe-dms-sng-31aec5ecbf"
  $.ReplicationSubnetGroup.SupportedNetworkTypes   = ["IPV4"]

Actual (emulator, 200):
  $.ReplicationSubnetGroup.ReplicationSubnetGroupArn = missing (None)
  $.ReplicationSubnetGroup.SupportedNetworkTypes     = missing (None)

Replay detail verbatim:
  "$.ReplicationSubnetGroup.ReplicationSubnetGroupArn: missing (expected 'arn:aws:dms:us-west-2:000000000000:subgrp:sim2real-probe-dms-sng-31aec5ecbf', got None)"
  "$.ReplicationSubnetGroup.SupportedNetworkTypes: missing (expected ['IPV4'], got None)"

Model reference: ReplicationSubnetGroup members per on-disk Smithy model are ReplicationSubnetGroupIdentifier, ReplicationSubnetGroupDescription, VpcId, SubnetGroupStatus, Subnets, SupportedNetworkTypes, IsReadOnly; service awsService.arnNamespace = "dms", apiVersion 2016-01-01, protocol awsJson1_1.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire fixture `instance-profile-and-subnet-group-lifecycle` (trace set simparity-wire-aws-v1).
2. Reach step `task#7`: an awsJson1_1 POST to the DMS endpoint with X-Amz-Target AmazonDMSv20160101.CreateReplicationSubnetGroup and a body supplying ReplicationSubnetGroupIdentifier, ReplicationSubnetGroupDescription and SubnetIds (the fixture creates subnet group `sim2real-probe-dms-sng-31aec5ecbf`).
3. Inspect the 200 response body: `ReplicationSubnetGroup.ReplicationSubnetGroupArn` and `ReplicationSubnetGroup.SupportedNetworkTypes` are absent, where AWS returns the subgrp ARN for the new group and ["IPV4"].
