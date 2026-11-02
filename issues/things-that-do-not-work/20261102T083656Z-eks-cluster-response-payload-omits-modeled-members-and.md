Issue-Source: trace-sweep-divergence
Issue-Trace: eks-ekscc-traj-reference-create-retry-and-delete
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-ekscc-traj-reference-create-retry-and-delete eks-ekscc-traj-lenient-scalar-coercion
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.certificateAuthority.active
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.certificateAuthority.data
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.clientRequestToken
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.controlPlaneScalingConfig
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.endpoint
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.identity
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.kubeApiServerConfig
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.kubeControllerManagerConfig
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.kubeSchedulerConfig
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.kubernetesNetworkConfig.elasticLoadBalancing
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.logging
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.resourcesVpcConfig.clusterSecurityGroupId
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.resourcesVpcConfig.controlPlaneEgressMode
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.resourcesVpcConfig.securityGroupIds
Issue-Key: eks|CreateCluster|2xx->2xx|$.cluster.vendedLogs
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.accessConfig.bootstrapClusterCreatorAdminPermissions
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.certificateAuthority.active
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.certificateAuthority.data
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.clientRequestToken
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.controlPlaneScalingConfig
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.endpoint
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.identity
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.kubernetesNetworkConfig.elasticLoadBalancing
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.logging
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.resourcesVpcConfig.clusterSecurityGroupId
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.resourcesVpcConfig.controlPlaneEgressMode
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.resourcesVpcConfig.securityGroupIds
Issue-Key: eks|DeleteCluster|2xx->2xx|$.cluster.vendedLogs
Issue-Key: eks|CreateCluster|2xx->4xx|$.cluster
Issue-Key: eks|CreateCluster|2xx->4xx|status
Issue-Key: eks|DeleteCluster|2xx->4xx|$.cluster
Issue-Key: eks|DeleteCluster|2xx->4xx|status
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# EKS Cluster response payload omits modeled members and invents values AWS leaves null

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Four net-new root causes share one fault: the emulator's serialization of the Cluster shape does not match EKS. It populates members EKS leaves null on a freshly created (still CREATING) cluster, omits members EKS always populates, and the request-side JSON reader is stricter than EKS about scalar coercion so a request EKS accepts is refused outright.

Cause 1 of 4: CreateCluster's Cluster payload is wrong in both directions.
  AWS: CreateCluster returns 200 with `cluster` of shape Cluster (Smithy: CreateCluster -> CreateClusterResponse.cluster : Cluster). For a cluster that is still CREATING, EKS leaves endpoint, identity, certificateAuthority.data, certificateAuthority.active and clientRequestToken null — none of those exist until the control plane is provisioned — while it DOES populate resourcesVpcConfig.securityGroupIds (an empty list when none were requested), resourcesVpcConfig.controlPlaneEgressMode, kubernetesNetworkConfig.elasticLoadBalancing, logging.clusterLogging, controlPlaneScalingConfig, vendedLogs, kubeApiServerConfig, kubeSchedulerConfig and kubeControllerManagerConfig. All of those are modeled members of Cluster (Smithy: Cluster.endpoint, .identity, .certificateAuthority, .clientRequestToken, .resourcesVpcConfig -> VpcConfigResponse, .kubernetesNetworkConfig, .logging, .controlPlaneScalingConfig, .kubeApiServerConfig, .kubeSchedulerConfig, .kubeControllerManagerConfig). The recorded values for the populated ones are structural defaults, not per-run data: securityGroupIds [], controlPlaneEgressMode 'AWS_MANAGED', elasticLoadBalancing {enabled:false}, logging.clusterLogging a single entry listing the five control-plane log types with enabled false, controlPlaneScalingConfig {tier:'standard'}, vendedLogs false, kubeApiServerConfig {eventTtl:'60m', serviceNodePortRange:{minPort:30000,maxPort:32767}}, and the kube-scheduler / kube-controller-manager default configs.
  Emulator: inverts this. It fabricates an endpoint, an identity.oidc.issuer, a certificateAuthority.data placeholder (base64 of a literal 'EMULATOR PLACEHOLDER: this emulator runs no certificate authority' note), a certificateAuthority.active object, a clusterSecurityGroupId and a clientRequestToken, while omitting securityGroupIds, controlPlaneEgressMode, elasticLoadBalancing, logging, controlPlaneScalingConfig, vendedLogs, kubeApiServerConfig, kubeSchedulerConfig and kubeControllerManagerConfig entirely.
  Why: a client that reads cluster.endpoint or cluster.certificateAuthority.data to build a kubeconfig gets a non-functional endpoint and a placeholder certificate that it accepts as real, instead of the null that tells it to keep waiting. Conversely a client that reads cluster.logging, cluster.controlPlaneScalingConfig or cluster.kubeApiServerConfig to report or diff the cluster's configuration finds nothing and concludes the cluster is unconfigured. (This is not the hostname/ID values themselves — those legitimately differ per environment — but which members are present versus null.)
  Fix: CreateCluster's serializer and the Cluster projection in the EKS state store, which must defer the provisioning-derived members until the cluster is ACTIVE and must emit the configuration defaults.
  Traces: eks-ekscc-traj-reference-create-retry-and-delete#task#5.

Cause 2 of 4: DeleteCluster's Cluster payload has the same defect, plus accessConfig.bootstrapClusterCreatorAdminPermissions.
  AWS: DeleteCluster returns 200 with `cluster` (Smithy: DeleteCluster -> DeleteClusterResponse.cluster : Cluster, DELETE /clusters/{name}). In the delete response EKS leaves accessConfig.bootstrapClusterCreatorAdminPermissions null — it is a create-time request flag, not cluster state — alongside the same null endpoint/identity/certificateAuthority/clientRequestToken, and populates the same configuration members.
  Emulator: echoes bootstrapClusterCreatorAdminPermissions back as true and repeats every presence/absence inversion from Cause 1.
  Why: a client diffing the pre- and post-delete cluster records sees fields change that AWS never reports, and still cannot read the configuration members it needs.
  Fix: DeleteCluster's serializer / the shared Cluster projection.
  Traces: eks-ekscc-traj-reference-create-retry-and-delete#task#7.

Cause 3 of 4: CreateCluster rejects a request whose scalars EKS coerces.
  AWS: EKS's JSON reader coerces scalars in CreateCluster's body — a numeric member supplied as a string, or a boolean supplied as a string — and the request succeeds with HTTP 200 and the full Cluster payload (status CREATING).
  Emulator: HTTP 400 and no `cluster` member; the cluster is never created.
  Why: clients and SDK versions that serialise a scalar as a string hit a hard 400 against the emulator for a request AWS accepts, so an otherwise valid provisioning call fails only in the emulator.
  Fix: CreateCluster's deserializer, which must apply AWS's scalar coercion rules rather than strict type matching.
  Traces: eks-ekscc-traj-lenient-scalar-coercion#task#5.

Cause 4 of 4: DeleteCluster then 404s, because the cluster Cause 3 refused was never created.
  AWS: DeleteCluster on that cluster returns 200 with `cluster` whose status is 'DELETING'.
  Emulator: HTTP 404 with no `cluster` member.
  Why: the client's teardown fails and it cannot tell a cluster that was never created from one already deleted, so it leaks or double-deletes.
  Fix: follows from Cause 3 — CreateCluster's deserializer; DeleteCluster's own not-found path is correct for a genuinely absent cluster.
  Traces: eks-ekscc-traj-lenient-scalar-coercion#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — eks-ekscc-traj-reference-create-retry-and-delete#task#5, CreateCluster, status 200->200. Members the emulator populated where the recording has null: $.cluster.endpoint (got a generated https://<id>.<x>.us-west-2.eks.amazonaws.com host), $.cluster.resourcesVpcConfig.clusterSecurityGroupId (got a generated sg-...), $.cluster.identity (got {'oidc': {'issuer': 'https://oidc.eks.us-west-2.amazonaws.com/id/<id>'}}), $.cluster.certificateAuthority.data (got base64 of 'EMULATOR PLACEHOLDER: this emulator runs no certificate authority...'), $.cluster.certificateAuthority.active (got {'id': <uuid>, 'activatedBy': 'EKS'}), $.cluster.clientRequestToken (got a uuid). Members the recording populates and the emulator omitted: $.cluster.resourcesVpcConfig.securityGroupIds (expected []), $.cluster.resourcesVpcConfig.controlPlaneEgressMode (expected 'AWS_MANAGED'), $.cluster.kubernetesNetworkConfig.elasticLoadBalancing (expected {'enabled': False}), $.cluster.logging (expected {'clusterLogging': [{'types': ['api','audit','authenticator','controllerManager','scheduler'], 'enabled': False}]}), $.cluster.controlPlaneScalingConfig (expected {'tier': 'standard'}), $.cluster.vendedLogs (expected False), $.cluster.kubeApiServerConfig (expected {'eventTtl': '60m', 'serviceNodePortRange': {'minPort': 30000, 'maxPort': 32767}}), $.cluster.kubeSchedulerConfig (expected the LeastAllocated nodeResourcesFit default), $.cluster.kubeControllerManagerConfig (expected the podGcController/HPA defaults).
Cause 2 — eks-ekscc-traj-reference-create-retry-and-delete#task#7, DeleteCluster, status 200->200: the same 13 presence/absence divergences plus $.cluster.accessConfig.bootstrapClusterCreatorAdminPermissions (expected None, got True).
Cause 3 — eks-ekscc-traj-lenient-scalar-coercion#task#5, CreateCluster: expected status 200 with $.cluster populated (status 'CREATING'); got status 400 and $.cluster missing.
Cause 4 — eks-ekscc-traj-lenient-scalar-coercion#task#6, DeleteCluster: expected status 200 with $.cluster populated (status 'DELETING'); got status 404 and $.cluster missing.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — shape Cluster (members name, arn, createdAt, version, endpoint, roleArn, resourcesVpcConfig: VpcConfigResponse, kubernetesNetworkConfig, logging, identity, status, certificateAuthority, clientRequestToken, platformVersion, tags, encryptionConfig, connectorConfig, id, health, outpostConfig, accessConfig, upgradePolicy, zonalShiftConfig, remoteNetworkConfig, computeConfig, storageConfig, deletionProtection, controlPlaneScalingConfig, kubeApiServerConfig, kubeSchedulerConfig, kubeControllerManagerConfig); CreateCluster (POST /clusters, 200, output cluster: Cluster); DeleteCluster (DELETE /clusters/{name}, 200, output cluster: Cluster).

## Steps to Reproduce
1. git clone the project repo and build the emulator from this checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Representative divergence: fixture eks-ekscc-traj-reference-create-retry-and-delete, step task#5; the other three at the fixture#step pairs listed per cause.
4. Manually: POST /clusters with a valid request and inspect the returned `cluster` — endpoint, identity, certificateAuthority.data/.active and clientRequestToken are populated although the cluster is CREATING, while securityGroupIds, controlPlaneEgressMode, elasticLoadBalancing, logging, controlPlaneScalingConfig, vendedLogs, kubeApiServerConfig, kubeSchedulerConfig and kubeControllerManagerConfig are absent. Then DELETE /clusters/<name> and observe the same inversion plus accessConfig.bootstrapClusterCreatorAdminPermissions echoed as true.
5. POST /clusters with a numeric or boolean body member supplied as a JSON string: the emulator answers 400 and creates nothing, and the follow-up DELETE /clusters/<name> answers 404.
