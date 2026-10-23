Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-aqua-configuration-modify-transition
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-aqua-configuration-modify-transition redshift-create-cluster-lifecycle
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.AquaConfiguration
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.AvailabilityZoneRelocation
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.AvailabilityZoneRelocationStatus
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.AzDisasterRecoveryStatus
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.ClusterNodes
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.ClusterParameterGroups.ClusterParameterGroup.ParameterGroupName
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.ClusterPublicKey
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.ClusterRevisionNumber
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.ClusterSubnetGroupName
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.DeferredMaintenanceWindows
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.Endpoint.VpcEndpoints
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.IsMaintenanceWindowActivated
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.KmsKeyId
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.NextMaintenanceWindowStartTime
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.PendingModifiedValues
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.VpcId
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.VpcSecurityGroups
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.AquaConfiguration
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.ClusterAvailabilityStatus
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.ClusterParameterGroups.ClusterParameterGroup.ParameterGroupName
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.ClusterSubnetGroupName
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.DeferredMaintenanceWindows
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.IsMaintenanceWindowActivated
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.KmsKeyId
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.NextMaintenanceWindowStartTime
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.PendingModifiedValues
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.VpcId
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.VpcSecurityGroups
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# Redshift Cluster structure omits modeled members on CreateCluster and DescribeClusters

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusters

## Description
Two causes, one shared fault: the emulator's Cluster serializer emits only a subset of the modeled Cluster members, so both the read path and the create path return a structurally incomplete cluster. Two of the emitted members also carry wrong values.

Cause 1 of 2 — DescribeClusters omits most Cluster members (fixture redshift-aqua-configuration-modify-transition, step task#3; same cluster also covers redshift-aqua-configuration-modify-transition#task#5 and redshift-create-cluster-lifecycle#task#2).
AWS: Smithy: DescribeClusters output ClustersMessage.Clusters is a ClusterList of Cluster. The Cluster structure models, among others, ClusterPublicKey (String, "The public key for the cluster"), ClusterNodes (ClusterNodesList of ClusterNode{NodeRole, PrivateIPAddress, PublicIPAddress}, "The nodes in the cluster"), VpcId, ClusterSubnetGroupName, VpcSecurityGroups (VpcSecurityGroupMembershipList), ClusterParameterGroups (ClusterParameterGroupStatusList), KmsKeyId, ClusterRevisionNumber, PendingModifiedValues, DeferredMaintenanceWindows, NextMaintenanceWindowStartTime, AvailabilityZoneRelocationStatus, AquaConfiguration, and Endpoint (Endpoint, whose VpcEndpoints carry VpcEndpoint{VpcEndpointId, VpcId, NetworkInterfaces}). The recording shows a VPC-resident provisioned cluster populating all of these.
Emulator: status matches (200) but these members are absent from the serialized Cluster: ClusterPublicKey, ClusterNodes, VpcId, ClusterSubnetGroupName, VpcSecurityGroups, KmsKeyId, ClusterRevisionNumber, PendingModifiedValues, DeferredMaintenanceWindows, NextMaintenanceWindowStartTime, AvailabilityZoneRelocation/AvailabilityZoneRelocationStatus, AzDisasterRecoveryStatus, AquaConfiguration, IsMaintenanceWindowActivated, Endpoint.VpcEndpoints, and ClusterParameterGroups.ClusterParameterGroup.ParameterGroupName.
Why: a client cannot do anything that depends on where or what the cluster is. Connecting through a VPC endpoint, computing node topology from ClusterNodes, checking encryption via KmsKeyId, loading the SSH public key, or deciding whether a modify is still pending via PendingModifiedValues all read None from a 200 response — an SDK deserializes this as a Cluster whose fields are simply null, so the failure surfaces far from the call as a null dereference rather than as an API error.
Fix: the Cluster serializer / cluster state model in the redshift service crate — populate the full modeled Cluster member set (and its nested Endpoint.VpcEndpoints, ClusterNodes, VpcSecurityGroups, ClusterParameterGroups members) rather than the current subset. Generated ids, IP addresses, keys and timestamps are environment-specific and are not requirements; their presence and shape are.

Cause 2 of 2 — CreateCluster returns the same truncated Cluster plus two wrong values (fixture redshift-create-cluster-lifecycle, step task#1).
AWS: Smithy: CreateCluster output CreateClusterResult.Cluster is the same Cluster shape, so the create response carries the same members. The recording shows a freshly created cluster with ClusterAvailabilityStatus 'Modifying', ClusterSubnetGroupName, KmsKeyId, VpcId, VpcSecurityGroups (a VpcSecurityGroup with Status 'active'), NextMaintenanceWindowStartTime, DeferredMaintenanceWindows, PendingModifiedValues (MasterUserPassword, redacted), AquaConfiguration (AquaConfigurationStatus 'auto', AquaStatus 'disabled'), and ClusterParameterGroups.ClusterParameterGroup.ParameterGroupName 'default.redshift-2.0'.
Emulator: ClusterAvailabilityStatus is 'Unavailable' instead of 'Modifying'; the default parameter group name is 'default.redshift-1.0' instead of 'default.redshift-2.0'; VpcSecurityGroups is serialized as an empty string rather than a list of VpcSecurityGroup members; and ClusterSubnetGroupName, KmsKeyId, VpcId, NextMaintenanceWindowStartTime, DeferredMaintenanceWindows, PendingModifiedValues, AquaConfiguration and IsMaintenanceWindowActivated are all absent.
Why: ClusterAvailabilityStatus is the member clients poll to decide when a new cluster is usable; reporting 'Unavailable' for a cluster AWS reports as 'Modifying' makes a normal create look like a failed or degraded one, and a waiter keyed on the AWS value never advances. The empty-string VpcSecurityGroups is a type divergence, not just a missing value: a client expecting a list gets a scalar. The default parameter group family name is what a client uses to look the group up in a follow-up DescribeClusterParameters, so the wrong family sends that call to a group that does not exist.
Fix: the CreateCluster handler plus the shared Cluster serializer — seed the new cluster's state with the full modeled member set, set the initial ClusterAvailabilityStatus to the value AWS reports for a cluster still coming up, emit empty lists as lists, and derive the default cluster parameter group name from the modeled default family.

Traces: redshift-aqua-configuration-modify-transition#task#3 (cause 1; cluster also covers #task#5 and redshift-create-cluster-lifecycle#task#2), redshift-create-cluster-lifecycle#task#1 (cause 2). Both causes must be closed before submitting.

## Evidence
Cause 1 — redshift-aqua-configuration-modify-transition#task#3, DescribeClusters, expected 200 / actual 200, verdict mismatch, 3 further affected steps (redshift-aqua-configuration-modify-transition#task#5, redshift-create-cluster-lifecycle#task#2). Replay detail (excerpt): "$...Clusters.Cluster.ClusterPublicKey: missing (expected 'ssh-rsa AAAA... Amazon-Redshift', got None)"; "...AvailabilityZoneRelocation: missing (expected 'enabled', got None)"; "...IsMaintenanceWindowActivated: missing (expected 'true', got None)"; "...ClusterSubnetGroupName: missing (expected 'default', got None)"; "...ClusterNodes: missing (expected {'member': {'PrivateIPAddress': ..., 'NodeRole': 'SHARED', 'PublicIPAddress': ...}}, got None)"; "...KmsKeyId: missing (expected 'AWS_OWNED_KMS_KEY', got None)"; "...ClusterRevisionNumber: missing (expected '436211', got None)"; "...Endpoint.VpcEndpoints: missing (expected {'VpcEndpoint': {'NetworkInterfaces': {...}, 'VpcId': ..., 'VpcEndpointId': ...}}, got None)"; "...VpcId: missing (expected 'vpc-...', got None)". Divergence key also lists AquaConfiguration, AvailabilityZoneRelocationStatus, AzDisasterRecoveryStatus, ClusterParameterGroups.ClusterParameterGroup.ParameterGroupName, DeferredMaintenanceWindows, NextMaintenanceWindowStartTime, PendingModifiedValues, VpcSecurityGroups as missing. The specific ids/keys/IPs above are environment-specific examples; the missing members are the defect.

Cause 2 — redshift-create-cluster-lifecycle#task#1, CreateCluster, expected 200 / actual 200, verdict mismatch. Replay detail: "$...Cluster.ClusterAvailabilityStatus: value_mismatch (expected 'Modifying', got 'Unavailable')"; "$...Cluster.ClusterParameterGroups.ClusterParameterGroup.ParameterGroupName: value_mismatch (expected 'default.redshift-2.0', got 'default.redshift-1.0')"; "$...Cluster.VpcSecurityGroups: type_mismatch (expected {'VpcSecurityGroup': {'VpcSecurityGroupId': '<shaped-not-asserted>', 'Status': 'active'}}, got '')"; plus missing IsMaintenanceWindowActivated, ClusterSubnetGroupName, KmsKeyId, VpcId, NextMaintenanceWindowStartTime, DeferredMaintenanceWindows, PendingModifiedValues, AquaConfiguration.

Model citations: Smithy Cluster members ClusterPublicKey, ClusterNodes (ClusterNodesList of ClusterNode{NodeRole, PrivateIPAddress, PublicIPAddress}), VpcId, ClusterSubnetGroupName, VpcSecurityGroups, ClusterParameterGroups, KmsKeyId, ClusterRevisionNumber, PendingModifiedValues, DeferredMaintenanceWindows, NextMaintenanceWindowStartTime, AvailabilityZoneRelocationStatus, AquaConfiguration, Endpoint; DescribeClusters output ClustersMessage.Clusters: ClusterList; CreateCluster output CreateClusterResult.Cluster: Cluster.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 2: replay fixture redshift-create-cluster-lifecycle step task#1 — CreateCluster for a VPC-resident provisioned cluster. Compare CreateClusterResult.Cluster against the recording: ClusterAvailabilityStatus, ClusterParameterGroups.ClusterParameterGroup.ParameterGroupName, VpcSecurityGroups (list vs empty string), and the absent ClusterSubnetGroupName/KmsKeyId/VpcId/NextMaintenanceWindowStartTime/DeferredMaintenanceWindows/PendingModifiedValues/AquaConfiguration members.
3. Cause 1: replay fixture redshift-aqua-configuration-modify-transition through step task#3 (CreateCluster, ModifyCluster, then DescribeClusters) and compare DescribeClustersResult.Clusters.Cluster against the recording — ClusterPublicKey, ClusterNodes, VpcId, ClusterSubnetGroupName, KmsKeyId, ClusterRevisionNumber, Endpoint.VpcEndpoints, VpcSecurityGroups, ClusterParameterGroups, PendingModifiedValues, AquaConfiguration and the maintenance-window members are all None.
4. The same Cluster gap reproduces at redshift-aqua-configuration-modify-transition#task#5 and redshift-create-cluster-lifecycle#task#2.
