Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected redshift.create-cluster-available-fields redshift.deploy-cluster-cfn-connectivity redshift.custom-domain-and-partner-ops-semantics redshift.idc-and-integration-ops-semantics redshift.create-hsm-client-certificate-duplicate-identifier-rejected redshift.hsm-client-certificate-public-key-blob-preserved redshift.create-snapshot-copy-grant-invalid-kms-key-rejected
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected|_error#9|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: redshift.create-cluster-available-fields|run#12|AssertionError|assert not created.get("Endpoint", {}).get("Address"), (
Issue-Key: redshift|DescribeClusters|ClusterNotFound
Issue-Key: redshift.custom-domain-and-partner-ops-semantics|run#10|AssertionError|assert observed == expected, f"custom-domain/partner contract drift:\n expected={expected}\n observed={observed}"
Issue-Key: redshift.idc-and-integration-ops-semantics|run#8|AssertionError|assert observed == expected, f"idc/integration contract drift:\n expected={expected}\n observed={observed}"
Issue-Key: redshift.create-hsm-client-certificate-duplicate-identifier-rejected|run#5|AssertionError|raise AssertionError(
Issue-Key: redshift.hsm-client-certificate-public-key-blob-preserved|run#7|AssertionError|assert code_1 == "InternalFailure", f"first attempt: {code_1!r}"
Issue-Key: redshift.create-snapshot-copy-grant-invalid-kms-key-rejected|run#7|AssertionError|assert (code, gone_code) == ("InvalidParameterValue", "SnapshotCopyGrantNotFoundFault"), (
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# Redshift CreateCluster skips request validation and publishes an endpoint while creating; custom-domain/partner, IDC/integration, HSM and snapshot-copy-grant families unimplemented; CFN-provisioned cluster invisible to the Redshift API (7 root causes)

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Seven distinct root causes, all in the redshift service. Each is a separate handler/subsystem and must be closed separately.

Cause 1 of 7: CreateCluster accepts requests AWS rejects — no shape/reference validation at all.
AWS: CreateCluster validates the node-count/cluster-type pairing, the referenced parameter group, the maintenance-window length and the maintenance track before provisioning anything, and returns a 400/404 error instead of a Cluster. Smithy: CreateCluster errors include ClusterParameterGroupNotFoundFault (httpError 404, awsQueryError code `ClusterParameterGroupNotFound`), InvalidClusterTrackFault (400, code `InvalidClusterTrack`), NumberOfNodesPerClusterLimitExceededFault, NumberOfNodesQuotaExceededFault; the node-count and maintenance-window rejections are recorded as code `InvalidParameterValue` (400). Recorded example messages: "Number of nodes must be 1 or not be supplied for cluster type single-node.", "Number of nodes for cluster type multi-node must be greater than or equal to 2.", "Maintenance window must be at least 30minutes.", "ClusterParameterGroup not found: <name>", "Invalid cluster track: <name>." — these are built from the request, so the rule (name the offending member and its constraint) is the requirement, not the exact text.
Emulator: all five malformed variants SUCCEED and return a Cluster, so the check has no error code to compare at all. Separately, DeleteClusterParameterGroup for a group a live cluster is a member of also succeeds where AWS answers `InvalidClusterParameterGroupState` (400).
Why: a client's negative tests all pass, invalid clusters get provisioned, and in-use parameter groups can be deleted out from under a running cluster — state the real service makes impossible.
Fix: request validation in the CreateCluster handler and the in-use reference check in the DeleteClusterParameterGroup handler, under services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/.
Traces: redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected.

Cause 2 of 7: a cluster in status `creating` already exposes an Endpoint.
AWS: DescribeClusters/CreateCluster for a cluster whose ClusterStatus is `creating` omits the Endpoint member entirely; Address/Port/VpcEndpoints appear only once the cluster becomes `available`. Smithy: Cluster.Endpoint is an optional structure, populated by the service (recorded behavior for the timing).
Emulator: the freshly created cluster reports ClusterStatus `creating` together with a fully populated Endpoint (Address, Port 5439 and a VpcEndpoints list with network interfaces).
Why: connection code that polls DescribeClusters and connects as soon as an Endpoint appears will dial a cluster that is not yet serving, instead of waiting for `available`.
Fix: the cluster-state projection/serializer for the Cluster shape in the redshift service — gate Endpoint on the lifecycle state.
Traces: redshift.create-cluster-available-fields.

Cause 3 of 7: a CloudFormation-provisioned AWS::Redshift::Cluster is not registered in the Redshift service's own state.
AWS: a stack that reaches CREATE_COMPLETE with an AWS::Redshift::Cluster resource has created a real cluster; redshift:DescribeClusters on its ClusterIdentifier returns it.
Emulator: after the stack reports CREATE_COMPLETE, DescribeClusters on the cluster identifier fails with `ClusterNotFound` (HTTP 404), message example "Cluster sim2real-cfn-rs-<id> not found." — the CFN resource provider never wrote the cluster into redshift state.
Why: every infrastructure-as-code workflow breaks: the stack claims success and the resource it claims to have created cannot be described, connected to or deleted through the service API.
Fix: the AWS::Redshift::Cluster resource provider in the cloudformation service must create the cluster through the redshift service's state, and the redshift cluster registry must own it.
Traces: redshift.deploy-cluster-cfn-connectivity.

Cause 4 of 7: the custom-domain and partner-integration operation family is not implemented.
AWS: for an unknown association DescribeCustomDomainAssociations fails `CustomDomainAssociationNotFoundFault`; CreateCustomDomainAssociation, ModifyCustomDomainAssociation, DeleteCustomDomainAssociation, DescribePartners, AddPartner, DeletePartner and UpdatePartnerStatus against an unknown cluster fail `ClusterNotFound` (recorded behavior, matching the modeled CustomDomainAssociationNotFoundFault / ClusterNotFoundFault shapes).
Emulator: all eight answer error code `NotImplemented` ("operation <X> is scaffolded but not implemented in this emulator").
Why: custom-domain and partner clients get an unmodelled code and cannot distinguish a missing cluster from a broken service.
Fix: the custom-domain and partner handlers plus their registries in the redshift service state.
Traces: redshift.custom-domain-and-partner-ops-semantics.

Cause 5 of 7: the IDC-application and zero-ETL integration operation family is not implemented.
AWS: for an unknown application ARN DescribeRedshiftIdcApplications, DeleteRedshiftIdcApplication and ModifyRedshiftIdcApplication fail `RedshiftIdcApplicationNotExists` (the modeled RedshiftIdcApplicationNotExistsFault is httpError 404); for an unknown integration ARN DescribeIntegrations, DeleteIntegration, ModifyIntegration and DescribeInboundIntegrations fail `IntegrationNotFoundFault`.
Emulator: all seven answer `NotImplemented`.
Why: identity-center and zero-ETL clients cannot probe for existence, so create-if-absent logic has no decidable branch.
Fix: the IDC-application and integration handlers plus their registries in the redshift service state.
Traces: redshift.idc-and-integration-ops-semantics.

Cause 6 of 7: the HSM client-certificate family is not implemented.
AWS: CreateHsmClientCertificate mints a certificate and returns HsmClientCertificate with its PublicKey blob preserved verbatim; a second create with the same HsmClientCertificateIdentifier fails HsmClientCertificateAlreadyExistsFault. Smithy: CreateHsmClientCertificate output = HsmClientCertificate; errors = HsmClientCertificateAlreadyExistsFault (httpError 400, awsQueryError code `HsmClientCertificateAlreadyExistsFault`), HsmClientCertificateQuotaExceededFault, InvalidTagFault, TagLimitExceededFault.
Emulator: the very first CreateHsmClientCertificate answers `NotImplemented`, so neither the stored PublicKey nor the duplicate rejection can be observed.
Why: HSM-encrypted cluster setup has no working path and the duplicate-identifier contract is unenforced.
Fix: the CreateHsmClientCertificate / DescribeHsmClientCertificates handlers and the certificate registry.
Traces: redshift.create-hsm-client-certificate-duplicate-identifier-rejected, redshift.hsm-client-certificate-public-key-blob-preserved.

Cause 7 of 7: CreateSnapshotCopyGrant does not validate KmsKeyId and the grant is never created.
AWS: a KmsKeyId that names no real key is rejected with `InvalidParameterValue` (400), and because the grant was refused, a subsequent DescribeSnapshotCopyGrants for that name fails `SnapshotCopyGrantNotFoundFault` (recorded behavior; Smithy models CreateSnapshotCopyGrant errors SnapshotCopyGrantAlreadyExistsFault, SnapshotCopyGrantQuotaExceededFault, LimitExceededFault, InvalidTagFault, TagLimitExceededFault, DependentServiceRequestThrottlingFault and does not model the KMS validation error, so its code comes from the recording).
Emulator: both the create and the follow-up describe answer `NotImplemented`.
Why: cross-region snapshot-copy setup cannot be validated or inspected, and a caller cannot tell a bad key from a missing grant.
Fix: the CreateSnapshotCopyGrant handler, its KMS key resolution, and the snapshot-copy-grant registry.
Traces: redshift.create-snapshot-copy-grant-invalid-kms-key-rejected.

## Evidence
Cause 1 — redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected:
  File "check_redshift_create_cluster_invalid_shape_and_in_use_parameter_group_delete_rejected.py", line 63, in run
    got = {label: _error(lambda o=override: rs.create_cluster(**{**valid, **o})) ...}
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code
The five variants the check breaks, each off an otherwise valid single-node request, with the expected (code, message) pairs:
  {"NumberOfNodes": 2} on ClusterType single-node -> ("InvalidParameterValue", "Number of nodes must be 1 or not be supplied for cluster type single-node.")
  {"ClusterType": "multi-node", "NumberOfNodes": 1} -> ("InvalidParameterValue", "Number of nodes for cluster type multi-node must be greater than or equal to 2.")
  {"ClusterParameterGroupName": "<nonexistent>"} -> ("ClusterParameterGroupNotFound", "ClusterParameterGroup not found: <name>")
  {"PreferredMaintenanceWindow": "wed:04:00-wed:04:10"} -> ("InvalidParameterValue", "Maintenance window must be at least 30minutes.")
  {"MaintenanceTrackName": "nosuch"} -> ("InvalidClusterTrack", "Invalid cluster track: nosuch.")
The same check also expects DeleteClusterParameterGroup on the live cluster's group to be ("InvalidClusterParameterGroupState", "One or more Clusters are still members of this parameter group <g>, so the group cannot be deleted").

Cause 2 — redshift.create-cluster-available-fields:
  AssertionError: a creating cluster must not expose an endpoint yet, got {'Address': 'sim2real-behav-rs-create-<id>.<id>.us-west-2.redshift.amazonaws.com', 'Port': 5439, 'VpcEndpoints': [{'VpcEndpointId': 'vpce-<id>', 'VpcId': 'vpc-<id>', 'NetworkInterfaces': [{'NetworkInterfaceId': 'eni-<id>', 'SubnetId': 'subnet-<id>', 'PrivateIpAddress': '10.255.198.194', 'AvailabilityZone': 'us-west-2b'}]}]}
(host names, ids and IPs are environment-specific examples; the requirement is the ABSENCE of Endpoint while ClusterStatus == 'creating')

Cause 3 — redshift.deploy-cluster-cfn-connectivity:
  botocore.errorfactory.ClusterNotFoundFault: An error occurred (ClusterNotFound) when calling the DescribeClusters operation: Cluster sim2real-cfn-rs-<id> not found.
  (raised after the stack containing the AWS::Redshift::Cluster had already reported CREATE_COMPLETE)

Cause 4 — redshift.custom-domain-and-partner-ops-semantics:
  AssertionError: custom-domain/partner contract drift:
  expected={'DescribeCustomDomainAssociations': 'CustomDomainAssociationNotFoundFault', 'CreateCustomDomainAssociation': 'ClusterNotFound', 'ModifyCustomDomainAssociation': 'ClusterNotFound', 'DeleteCustomDomainAssociation': 'ClusterNotFound', 'DescribePartners': 'ClusterNotFound', 'AddPartner': 'ClusterNotFound', 'DeletePartner': 'ClusterNotFound', 'UpdatePartnerStatus': 'ClusterNotFound'}
  observed={all eight: 'NotImplemented'}

Cause 5 — redshift.idc-and-integration-ops-semantics:
  AssertionError: idc/integration contract drift:
  expected={'DescribeRedshiftIdcApplications': 'RedshiftIdcApplicationNotExists', 'DeleteRedshiftIdcApplication': 'RedshiftIdcApplicationNotExists', 'ModifyRedshiftIdcApplication': 'RedshiftIdcApplicationNotExists', 'DescribeIntegrations': 'IntegrationNotFoundFault', 'DeleteIntegration': 'IntegrationNotFoundFault', 'ModifyIntegration': 'IntegrationNotFoundFault', 'DescribeInboundIntegrations': 'IntegrationNotFoundFault'}
  observed={all seven: 'NotImplemented'}

Cause 6 — redshift.create-hsm-client-certificate-duplicate-identifier-rejected:
  AssertionError: unexpected error creating HSM client certificate: 'NotImplemented'
Cause 6 — redshift.hsm-client-certificate-public-key-blob-preserved:
  AssertionError: first attempt: 'NotImplemented'

Cause 7 — redshift.create-snapshot-copy-grant-invalid-kms-key-rejected:
  AssertionError: expected InvalidParameterValue for a KmsKeyId naming no real key and then SnapshotCopyGrantNotFoundFault for the name it was refused under, got ('NotImplemented', 'NotImplemented')

## Steps to Reproduce
1. Boot the emulator; point redshift, cloudformation and kms clients at it.
2. Cause 1: create a cluster parameter group, then call CreateCluster five times off one valid single-node request, each time breaking exactly one field as listed in the evidence. Each call should be refused with the stated code; observe that each SUCCEEDS. Then create the valid cluster and call DeleteClusterParameterGroup on its group — expect InvalidClusterParameterGroupState 400.
3. Cause 2: CreateCluster, then immediately DescribeClusters. While ClusterStatus == 'creating', assert no Endpoint member is present.
4. Cause 3: create a CloudFormation stack containing an AWS::Redshift::Cluster, wait for CREATE_COMPLETE, then call redshift DescribeClusters with that ClusterIdentifier. Expect the cluster; observe ClusterNotFound 404.
5. Cause 4: call the eight custom-domain/partner operations with an unknown cluster or association name.
6. Cause 5: call the seven IDC-application/integration operations with an unknown application or integration ARN.
7. Cause 6: call CreateHsmClientCertificate with a fresh identifier (expect HsmClientCertificate with the PublicKey preserved), then call it again with the same identifier (expect HsmClientCertificateAlreadyExistsFault 400).
8. Cause 7: call CreateSnapshotCopyGrant with a KmsKeyId naming no key (expect InvalidParameterValue 400), then DescribeSnapshotCopyGrants for that grant name (expect SnapshotCopyGrantNotFoundFault).
9. Replay the trace_ids checks to confirm.
