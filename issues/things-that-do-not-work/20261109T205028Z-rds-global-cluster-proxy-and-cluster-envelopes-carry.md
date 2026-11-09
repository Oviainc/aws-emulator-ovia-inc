Issue-Source: trace-sweep-divergence
Issue-Trace: rds-already-exists-conflict-class
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-already-exists-conflict-class rds-global-cluster-mutation-envelopes rds-identifier-validation-and-lowercasing rds-global-cluster-lifecycle rds-db-cluster-state-roles-and-capacity-operations rds-db-proxy-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateGlobalCluster|2xx->2xx|$.CreateGlobalClusterResponse.CreateGlobalClusterResult.GlobalCluster.Endpoint
Issue-Key: rds|DescribeGlobalClusters|2xx->2xx|$.DescribeGlobalClustersResponse.DescribeGlobalClustersResult.GlobalClusters.GlobalClusterMember.Endpoint
Issue-Key: rds|ModifyGlobalCluster|2xx->2xx|$.ModifyGlobalClusterResponse.ModifyGlobalClusterResult.GlobalCluster.Endpoint
Issue-Key: rds|DeleteGlobalCluster|2xx->2xx|$.DeleteGlobalClusterResponse.DeleteGlobalClusterResult.GlobalCluster.Endpoint
Issue-Key: rds|DescribeDBProxies|2xx->2xx|$.DescribeDBProxiesResponse.DescribeDBProxiesResult.DBProxies.member.Endpoint
Issue-Related: 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md
Issue-Related: 20261107T151558Z-rds-global-cluster-mutation-envelopes-carry-a-placeholder.md
Issue-Related: 20261109T205028Z-rds-db-cluster-envelopes-publish-a-random-listener.md
Issue-Commit: fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7

# rds: global-cluster, proxy and cluster envelopes carry placeholder ids, unserved endpoints and omit modeled members (7 causes)

> _Note: this issue was identified against the code base at commit `fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGlobalCluster

## Description
Seven divergence clusters share one root cause: the rds resource envelopes for global clusters, DB proxies and DB clusters are assembled from emulator-local placeholders instead of from the modeled resource state — identifiers are a shared zero-padded counter, endpoint hostnames are published on names the emulator does not answer on, and several modeled members are omitted or emitted as the wrong type. All seven are on the `rds` service, and the replay reported no owning issue and no blocking stub for any of them.

Cause 1 of 7: CreateGlobalCluster — GlobalClusterResourceId is a shared placeholder and Endpoint is not a served host.
AWS: Smithy `GlobalCluster.GlobalClusterResourceId` is "the Amazon Web Services partition-unique, immutable identifier for the global database cluster" — an opaque `cluster-`-prefixed value distinct for every global cluster. `GlobalCluster.Endpoint` is "the writer endpoint for the new global database cluster… always points to the writer DB instance in the current primary cluster", i.e. a name a client can actually connect through.
Emulator: `GlobalClusterResourceId` is the fixed zero-padded counter value `cluster-000…001`, reused across global clusters, and `Endpoint` is published on a `*.global.rds.amazonaws.com` host the emulator does not serve (verdict `host_not_own`).
Why: two global clusters are indistinguishable by their immutable resource id, so any client keyed on it collides; and the advertised writer endpoint cannot be connected to, so the global-database data path is unusable.
Fix: the global-cluster resource-id minting in rds state (one unique opaque id per global cluster) and the endpoint construction in the GlobalCluster serializer (publish a host the emulator answers on). Open issues 20261107T151558Z-rds-global-cluster-mutation-envelopes-carry-a-placeholder.md and 20261109T083259Z-rds-serves-emulator-authored-values-on-success-paths.md discuss the placeholder resource id but carry no replay key for this step.
Traces: rds-already-exists-conflict-class#task#5 (cluster also covers rds-global-cluster-mutation-envelopes#task#2, rds-identifier-validation-and-lowercasing#task#9).

Cause 2 of 7: DescribeGlobalClusters — the member Endpoint is likewise not a served host.
AWS: the read path projects the stored global cluster, so `Endpoint` is the same connectable writer endpoint.
Emulator: the `GlobalClusterMember.Endpoint` is published on a `*.global.rds.amazonaws.com` name the emulator does not serve.
Why: a client that discovers the global writer endpoint through DescribeGlobalClusters cannot reach it.
Fix: the DescribeGlobalClusters GlobalCluster serializer / shared endpoint construction.
Traces: rds-global-cluster-lifecycle#task#2.

Cause 3 of 7: ModifyGlobalCluster — the mutation envelope repeats the unserved Endpoint.
AWS: ModifyGlobalCluster returns the `GlobalCluster` envelope with the connectable writer endpoint.
Emulator: `Endpoint` is on a host the emulator does not serve.
Why: the mutation response is the canonical post-change record; a caller re-reading its endpoint from it gets an unreachable name.
Fix: the ModifyGlobalCluster handler's GlobalCluster projection.
Traces: rds-global-cluster-lifecycle#task#4 (cluster also covers rds-global-cluster-lifecycle#task#5).

Cause 4 of 7: ModifyGlobalCluster also returns the placeholder GlobalClusterResourceId.
AWS: as cause 1 — an opaque per-global-cluster immutable id, unchanged by a modify.
Emulator: `GlobalClusterResourceId` is the shared `cluster-000…001` placeholder; `Endpoint` is again unserved.
Why: the modify response cannot be used to correlate the resource.
Fix: same resource-id minting, projected by the ModifyGlobalCluster handler.
Traces: rds-global-cluster-mutation-envelopes#task#5 (cluster also covers rds-global-cluster-mutation-envelopes#task#7).

Cause 5 of 7: DeleteGlobalCluster reports Status 'deleting' where AWS returns the pre-delete state, plus the placeholder id and unserved Endpoint.
AWS: Smithy `GlobalCluster.Status` is "the current state of this global database cluster". `DeleteGlobalCluster` returns the global cluster as it stood when the delete request was accepted — the recording shows `available` — rather than a state the emulator has already advanced to.
Emulator: `Status` is `deleting`, `GlobalClusterResourceId` is the shared placeholder, and `Endpoint` is unserved.
Why: a caller that treats the delete response as the accepted-state snapshot (and polls afterwards for disappearance) sees a state transition AWS does not report synchronously, so state machines keyed on the delete response branch wrongly.
Fix: the DeleteGlobalCluster handler — serialize the envelope from pre-mutation state, then apply the deletion.
Traces: rds-global-cluster-mutation-envelopes#task#10.

Cause 6 of 7: DescribeDBClusters emits a ReaderEndpoint for a cluster that has none.
AWS: Smithy `DBCluster.ReaderEndpoint` "load-balances connections across the Aurora Replicas that are available in a DB cluster"; it is optional and AWS omits it entirely for a cluster that has no reader endpoint (the recording has no value for this cluster).
Emulator: a `*.cluster-ro-…rds.amazonaws.com` ReaderEndpoint is synthesized unconditionally, on a host the emulator does not serve.
Why: a client that branches on the presence of `ReaderEndpoint` to decide whether read traffic can be split wrongly concludes a reader endpoint exists, then fails to connect.
Fix: the DBCluster serializer — emit `ReaderEndpoint` only when the cluster actually has one, and point it at a host the emulator answers on.
Traces: rds-db-cluster-state-roles-and-capacity-operations#task#4.

Cause 7 of 7: DescribeDBProxies omits VpcId and Auth.ClientPasswordAuthType, emits VpcSecurityGroupIds as an empty scalar instead of a list, and publishes an unserved proxy Endpoint.
AWS: Smithy `DBProxy.VpcId` is "the VPC ID of the DB proxy"; `DBProxy.VpcSecurityGroupIds` is a `StringList` of "VPC security groups that the proxy belongs to" and is serialized as a member list in the awsQuery/XML envelope; `UserAuthConfigInfo.ClientPasswordAuthType` is "the type of authentication the proxy uses for connections from clients" (enum ClientPasswordAuthType, e.g. MYSQL_CACHING_SHA2_PASSWORD); `DBProxy.Endpoint` is "the endpoint that you can use to connect to the DB proxy".
Emulator: `VpcId` and `Auth[].ClientPasswordAuthType` are absent, `VpcSecurityGroupIds` is an empty string rather than a list of the security groups the proxy was created with, and `Endpoint` is on a `*.proxy-….rds.amazonaws.com` host the emulator does not serve.
Why: a client that read back its proxy's VPC placement, security-group attachment or client password auth type to validate a deployment finds nothing; an SDK that expects a list for `VpcSecurityGroupIds` sees a scalar and can fail to deserialize.
Fix: the DescribeDBProxies handler and the DBProxy serializer — persist and project VpcId, the requested VpcSecurityGroupIds (as a list) and each Auth entry's ClientPasswordAuthType, and build the proxy endpoint on a host the emulator serves. Related open issue 20261109T135215Z-lambda-rds-secretsmanager-db-proxy-endpoints-are-published.md discusses proxy endpoint hosts but carries no replay key for this step.
Traces: rds-db-proxy-lifecycle#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (98 fixtures, 993 steps, score 49.8%). All seven clusters replied HTTP 200 where HTTP 200 was expected; the divergences are in the serialized envelopes.

Cause 1 — rds-already-exists-conflict-class#task#5, CreateGlobalCluster:
  $...GlobalCluster.GlobalClusterResourceId: value_mismatch (expected a shaped unique id, got 'cluster-00000000000000000000000001')
  $...GlobalCluster.Endpoint: host_not_own (published on a *.global.rds.amazonaws.com name the emulator does not serve)

Cause 2 — rds-global-cluster-lifecycle#task#2, DescribeGlobalClusters:
  $...GlobalClusters.GlobalClusterMember.Endpoint: host_not_own

Cause 3 — rds-global-cluster-lifecycle#task#4, ModifyGlobalCluster:
  $...GlobalCluster.Endpoint: host_not_own

Cause 4 — rds-global-cluster-mutation-envelopes#task#5, ModifyGlobalCluster:
  $...GlobalCluster.GlobalClusterResourceId: value_mismatch (got 'cluster-00000000000000000000000001')
  $...GlobalCluster.Endpoint: host_not_own

Cause 5 — rds-global-cluster-mutation-envelopes#task#10, DeleteGlobalCluster:
  $...GlobalCluster.Status: value_mismatch (expected 'available', got 'deleting')
  $...GlobalCluster.GlobalClusterResourceId: value_mismatch (got 'cluster-00000000000000000000000001')
  $...GlobalCluster.Endpoint: host_not_own

Cause 6 — rds-db-cluster-state-roles-and-capacity-operations#task#4, DescribeDBClusters:
  $...DBCluster.ReaderEndpoint: host_not_own (expected None — AWS omitted the member — got a synthesized *.cluster-ro-… name)

Cause 7 — rds-db-proxy-lifecycle#task#7, DescribeDBProxies:
  $...DBProxies.member.VpcSecurityGroupIds: type_mismatch (expected a member list of the proxy's security groups, got '')
  $...DBProxies.member.Auth.member.ClientPasswordAuthType: missing (expected 'MYSQL_CACHING_SHA2_PASSWORD', got None)
  $...DBProxies.member.VpcId: missing (expected the proxy's VPC id, got None)
  $...DBProxies.member.Endpoint: host_not_own

## Steps to Reproduce
1. git_clone() then replay_wire_corpus() against the built emulator.
2. Cause 1: fixture rds-already-exists-conflict-class step task#5 — CreateGlobalCluster, then read GlobalCluster.GlobalClusterResourceId and GlobalCluster.Endpoint. Create a second global cluster and observe the same resource id.
3. Cause 2: fixture rds-global-cluster-lifecycle step task#2 — DescribeGlobalClusters and read the member Endpoint; try to connect to it.
4. Cause 3: same fixture step task#4 — ModifyGlobalCluster and read GlobalCluster.Endpoint.
5. Cause 4: fixture rds-global-cluster-mutation-envelopes step task#5 — ModifyGlobalCluster and read GlobalCluster.GlobalClusterResourceId.
6. Cause 5: same fixture step task#10 — DeleteGlobalCluster and read GlobalCluster.Status (emulator reports 'deleting'; AWS reports the pre-delete state).
7. Cause 6: fixture rds-db-cluster-state-roles-and-capacity-operations step task#4 — DescribeDBClusters on a cluster with no reader endpoint and observe ReaderEndpoint is present.
8. Cause 7: fixture rds-db-proxy-lifecycle step task#7 — CreateDBProxy with VpcSecurityGroupIds and an Auth entry carrying ClientPasswordAuthType, then DescribeDBProxies and read VpcId, VpcSecurityGroupIds, Auth[].ClientPasswordAuthType and Endpoint.
