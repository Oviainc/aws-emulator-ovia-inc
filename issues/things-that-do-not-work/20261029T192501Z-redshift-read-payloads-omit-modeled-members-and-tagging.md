Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-aqua-configuration-modify-transition
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-aqua-configuration-modify-transition redshift-create-cluster-lifecycle redshift-empty-inventory-reads redshift-authentication-profile-lifecycle redshift-snapshot-copy-grant-and-authentication-profile redshift-integration-and-idc-application-preconditions redshift-resource-tagging-on-parameter-group
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.AvailabilityZoneRelocation
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.AzDisasterRecoveryStatus
Issue-Key: redshift|DescribeClusters|2xx->2xx|$.DescribeClustersResponse.DescribeClustersResult.Clusters.Cluster.IsMaintenanceWindowActivated
Issue-Key: redshift|CreateCluster|2xx->2xx|$.CreateClusterResponse.CreateClusterResult.Cluster.IsMaintenanceWindowActivated
Issue-Key: redshift|DescribeInboundIntegrations|2xx->2xx|$.DescribeInboundIntegrationsResponse.DescribeInboundIntegrationsResult.Integrations
Issue-Key: redshift|DescribeAuthenticationProfiles|2xx->2xx|$.DescribeAuthenticationProfilesResponse.DescribeAuthenticationProfilesResult.AuthenticationProfiles.member.AuthenticationProfileContent
Issue-Key: redshift|CreateSnapshotCopyGrant|2xx->2xx|$.CreateSnapshotCopyGrantResponse.CreateSnapshotCopyGrantResult.SnapshotCopyGrant.KmsKeyId
Issue-Key: redshift|ModifyIntegration|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeleteIntegration|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeTags|2xx->2xx|$.DescribeTagsResponse.DescribeTagsResult.TaggedResources.TaggedResource[*].Tag.Key
Issue-Key: redshift|DescribeTags|2xx->2xx|$.DescribeTagsResponse.DescribeTagsResult.TaggedResources.TaggedResource[*].Tag.Value
Issue-Key: redshift|CreateTags|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|CreateTags|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeTags|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift read payloads omit modeled members and tagging responses diverge in order, code and wording

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusters

## Description
10 root causes: success payloads drop modeled members or reorder content, and the tagging surface refuses with the wrong error class and wording.

Cause 1 of 10: DescribeClusters omits three Cluster members. AWS serializes AvailabilityZoneRelocationStatus/AvailabilityZoneRelocation, IsMaintenanceWindowActivated and AzDisasterRecoveryStatus on every cluster it describes, reflecting the cluster's configured state (for a cluster created with availability-zone relocation enabled and inside its maintenance window: 'enabled', 'true', 'enabled'). The emulator does not emit these elements at all. Why: a client reading relocation capability, maintenance-window activity or AZ disaster-recovery state from DescribeClusters gets no value and cannot distinguish "disabled" from "not reported". Fix: the DescribeClusters Cluster serializer and the cluster state that backs these three members. Traces: redshift-aqua-configuration-modify-transition#task#3.

Cause 2 of 10: CreateCluster's returned Cluster omits IsMaintenanceWindowActivated, which AWS includes in the create response as well as in describes. Fix: as cause 1, on the CreateCluster response path. Traces: redshift-create-cluster-lifecycle#task#1.

Cause 3 of 10: DescribeInboundIntegrations omits the Integrations collection when there are no integrations. AWS serializes the modeled list member as an EMPTY element (present, no children); the emulator omits it entirely. Why: a client that iterates Integrations without a presence check breaks on the empty-inventory response, and awsQuery/XML consumers see a different document than AWS's. Fix: the DescribeInboundIntegrations response serializer must emit the collection member even when empty. Traces: redshift-empty-inventory-reads#task#12.

Cause 4 of 10: DescribeAuthenticationProfiles does not echo AuthenticationProfileContent verbatim. Smithy: AuthenticationProfileContent is a plain String. AWS stores and returns the exact document the caller supplied, preserving its member order. The emulator parses the JSON and re-serializes it with members in a different (sorted) order. Why: a client that compares the returned content to what it submitted, or that hashes/signs the document, sees a mismatch on an unchanged profile. Fix: store and return AuthenticationProfileContent as an opaque string in the authentication-profile state rather than round-tripping it through a JSON model. Traces: redshift-authentication-profile-lifecycle#task#2.

Cause 5 of 10: CreateSnapshotCopyGrant returns the requested KMS alias instead of the resolved key identifier. AWS resolves the KmsKeyId argument (including the default Redshift alias) to the KMS key's identifier and returns that in SnapshotCopyGrant.KmsKeyId. The emulator echoes the alias string it was given. Why: a client that uses the returned KmsKeyId to look the key up in KMS, or to compare grants, resolves an alias where AWS gave a key id. Fix: CreateSnapshotCopyGrant must resolve KmsKeyId through KMS before storing/returning the grant. Traces: redshift-snapshot-copy-grant-and-authentication-profile#task#1.

Cause 6 of 10: ModifyIntegration for an unknown integration ARN returns the wrong message. Smithy: IntegrationNotFoundFault @error("client"), httpError 404. AWS: 404 with the FIXED message "The requested integration doesn't exist." — no variable part; AWS does not echo the ARN. Emulator: "Integration not found: <IntegrationArn>", echoing the request value. Fix: ModifyIntegration's integration-lookup refusal message. Traces: redshift-integration-and-idc-application-preconditions#task#2.

Cause 7 of 10: DeleteIntegration for an unknown integration ARN likewise returns "Integration not found: <IntegrationArn>" where AWS returns the fixed "The requested integration doesn't exist." Fix: DeleteIntegration's refusal message. Traces: redshift-integration-and-idc-application-preconditions#task#3.

Cause 8 of 10: DescribeTags returns a resource's tags in the wrong order. AWS returns TaggedResources in the order the tags were created on the resource; the emulator returns them sorted by tag key, so position-by-position the Key and Value of every entry disagree with AWS's. Why: a client that indexes TaggedResources positionally (or asserts on the first tag) reads the wrong tag. Fix: preserve tag insertion order in the tag state and in the DescribeTags serializer. Traces: redshift-resource-tagging-on-parameter-group#task#3.

Cause 9 of 10: CreateTags rejects a reserved 'aws:'-prefixed tag key with the wrong error class and wording. Smithy: CreateTags declares both InvalidTagFault (httpError 400, awsQueryError InvalidTagFault) and ResourceNotFoundFault; AWS refuses a system-tag mutation with Code 'InvalidParameterValue' and the fixed message "Caller is an end user and not allowed to mutate system tags" — no variable part, the offending key is not echoed. The emulator refuses with Code 'InvalidTagFault' and the emulator-authored "Invalid tag key '<key>': the prefix 'aws:' is reserved for Amazon Web Services." Why: a client switching on the error code does not see InvalidParameterValue and mis-classifies a permanent authorization-shaped refusal as a tag-format problem. Fix: CreateTags' reserved-prefix validation must raise InvalidParameterValue with AWS's fixed message. Traces: redshift-resource-tagging-on-parameter-group#task#6.

Cause 10 of 10: DescribeTags for a resource ARN that does not exist returns the wrong message. AWS: 404 ResourceNotFoundFault, "<resource-type> (<resource-name>) was not found." — fixed form is the ARN's resource-type token, then the resource name in parentheses, then " was not found." Emulator: "The resource could not be found: <full ARN>", a generic sentence echoing the whole ARN. Fix: the DescribeTags resource-resolution refusal message. Traces: redshift-resource-tagging-on-parameter-group#task#7.

## Evidence
Replay of simparity-wire-aws-v1; all ten clusters verdict=mismatch.

DescribeClusters (redshift-aqua-configuration-modify-transition#task#3; cluster also covers #task#5 and redshift-create-cluster-lifecycle#task#2), 200 vs 200:
  Cluster.AvailabilityZoneRelocation: missing (expected 'enabled', got None)
  Cluster.IsMaintenanceWindowActivated: missing (expected 'true', got None)
  Cluster.AzDisasterRecoveryStatus: missing (expected 'enabled', got None)
CreateCluster (redshift-create-cluster-lifecycle#task#1), 200 vs 200:
  Cluster.IsMaintenanceWindowActivated: missing (expected 'true', got None)
DescribeInboundIntegrations (redshift-empty-inventory-reads#task#12), 200 vs 200:
  DescribeInboundIntegrationsResult.Integrations: missing (expected '', got None)
DescribeAuthenticationProfiles (redshift-authentication-profile-lifecycle#task#2; cluster also covers #task#4), 200 vs 200:
  AuthenticationProfileContent: expected '{"Client_ID":"sim2real-probe-client","AllowDBUserOverride":"1"}', got '{"AllowDBUserOverride":"1","Client_ID":"sim2real-probe-client"}' — same members, re-ordered.
CreateSnapshotCopyGrant (redshift-snapshot-copy-grant-and-authentication-profile#task#1), 200 vs 200:
  SnapshotCopyGrant.KmsKeyId: expected the resolved key id, got 'alias/aws/redshift'
ModifyIntegration (redshift-integration-and-idc-application-preconditions#task#2) and DeleteIntegration (#task#3), 404 vs 404:
  expected "The requested integration doesn't exist.", got 'Integration not found: arn:aws:redshift:us-west-2:000000000000:integration:11111111-1111-1111-1111-111111111111'
DescribeTags (redshift-resource-tagging-on-parameter-group#task#3), 200 vs 200:
  TaggedResource[0].Tag.Key: expected 'sim2real-probe-env', got 'sim2real-probe-owner'; [0].Tag.Value: expected 'sandbox', got 'simparity'; [1] the mirror image — the two tags are returned in the opposite order.
CreateTags (#task#6), 400 vs 400:
  Error.Code: expected 'InvalidParameterValue', got 'InvalidTagFault'
  Error.Message: expected 'Caller is an end user and not allowed to mutate system tags', got "Invalid tag key 'aws:owner': the prefix 'aws:' is reserved for Amazon Web Services."
DescribeTags (#task#7), 404 vs 404:
  expected 'parametergroup (sim2real-probe-tagpg-missing) was not found.', got 'The resource could not be found: arn:aws:redshift:us-west-2:000000000000:parametergroup:sim2real-probe-tagpg-missing'

Tag keys/values, resource names and ARNs shown are request/state values filling the templates; the presence of the modeled members, the ordering rule, the error code and the fixed wording are the requirements.

## Steps to Reproduce
1. Replay redshift-aqua-configuration-modify-transition step task#3 (DescribeClusters after a relocation-enabled cluster is created) and check for AvailabilityZoneRelocation, IsMaintenanceWindowActivated and AzDisasterRecoveryStatus.
2. Replay redshift-create-cluster-lifecycle step task#1 and check CreateCluster's Cluster for IsMaintenanceWindowActivated.
3. Replay redshift-empty-inventory-reads step task#12 and check that DescribeInboundIntegrations emits an empty Integrations element.
4. Replay redshift-authentication-profile-lifecycle step task#2 and compare AuthenticationProfileContent byte-for-byte with what was submitted.
5. Replay redshift-snapshot-copy-grant-and-authentication-profile step task#1 and compare SnapshotCopyGrant.KmsKeyId with the recording.
6. Replay redshift-integration-and-idc-application-preconditions steps task#2 and task#3 with an unknown integration ARN and compare Error.Message.
7. Replay redshift-resource-tagging-on-parameter-group: step task#3 (DescribeTags ordering), step task#6 (CreateTags with an 'aws:'-prefixed key -> compare Error.Code and Error.Message), step task#7 (DescribeTags for a nonexistent resource ARN -> compare Error.Message).
