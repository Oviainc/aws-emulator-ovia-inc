Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-partner-and-custom-domain-preconditions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-partner-and-custom-domain-preconditions
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribePartners|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|AddPartner|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|UpdatePartnerStatus|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeletePartner|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CreateCustomDomainAssociation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|ModifyCustomDomainAssociation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeleteCustomDomainAssociation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeCustomDomainAssociations|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift partner and custom-domain refusals carry emulator-authored not-found messages

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePartners

## Description
8 root causes: the partner-integration and custom-domain-association operations all answer the right error class and HTTP status but the wrong message text. Smithy: ClusterNotFoundFault is @error("client"), httpError 404, awsQueryError code ClusterNotFound; CustomDomainAssociationNotFoundFault likewise httpError 404. Status and code match in every cluster; only $.ErrorResponse.Error.Message is wrong.

Cause 1 of 8: DescribePartners with an unknown ClusterIdentifier. AWS: 404 ClusterNotFound, Message "Cluster not found: <ClusterIdentifier>" — fixed text "Cluster not found: " plus the ClusterIdentifier from the request. Emulator: "Cluster <ClusterIdentifier> not found." Why: a client or operator tooling matching AWS's refusal text does not recognize the refusal, so a missing-cluster condition is reported as an unknown failure. Fix: the DescribePartners handler's cluster-lookup refusal path. Traces: redshift-partner-and-custom-domain-preconditions#task#1.

Cause 2 of 8: AddPartner, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: AddPartner handler refusal path. Traces: redshift-partner-and-custom-domain-preconditions#task#2.

Cause 3 of 8: UpdatePartnerStatus, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: UpdatePartnerStatus handler refusal path. Traces: redshift-partner-and-custom-domain-preconditions#task#3.

Cause 4 of 8: DeletePartner, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: DeletePartner handler refusal path. Traces: redshift-partner-and-custom-domain-preconditions#task#4.

Cause 5 of 8: CreateCustomDomainAssociation, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: CreateCustomDomainAssociation handler refusal path. Traces: redshift-partner-and-custom-domain-preconditions#task#5.

Cause 6 of 8: ModifyCustomDomainAssociation, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: ModifyCustomDomainAssociation handler refusal path. Traces: redshift-partner-and-custom-domain-preconditions#task#6.

Cause 7 of 8: DeleteCustomDomainAssociation, unknown cluster. AWS: 404 ClusterNotFound, "Cluster not found: <ClusterIdentifier>". Emulator: "Cluster <ClusterIdentifier> not found." Fix: DeleteCustomDomainAssociation handler refusal path. Traces: redshift-partner-and-custom-domain-preconditions#task#7.

Cause 8 of 8: DescribeCustomDomainAssociations for a custom domain name with no association. AWS: 404 CustomDomainAssociationNotFound with the association-scoped message "There aren't any associations linked to the custom domain name <CustomDomainName>." — fixed text "There aren't any associations linked to the custom domain name " plus the request's CustomDomainName and a trailing period. Emulator: "CustomDomainAssociation not found: <CustomDomainName>", a generic resource-not-found rendering that names an internal type name rather than AWS's sentence. Why: callers that distinguish "no associations yet" from other failures by message cannot. Fix: the DescribeCustomDomainAssociations handler's empty-result refusal path. Traces: redshift-partner-and-custom-domain-preconditions#task#8.

## Evidence
Replay of simparity-wire-aws-v1; all eight clusters verdict=mismatch with expected_status == actual_status == 404 and matching Error.Code, diverging only on Error.Message.

Representative (DescribePartners, #task#1):
  expected 'Cluster not found: sim2real-probe-cluster-missing', got 'Cluster sim2real-probe-cluster-missing not found.'
AddPartner #task#2, UpdatePartnerStatus #task#3, DeletePartner #task#4, CreateCustomDomainAssociation #task#5, ModifyCustomDomainAssociation #task#6, DeleteCustomDomainAssociation #task#7: identical expected/got pair.
DescribeCustomDomainAssociations #task#8:
  expected "There aren't any associations linked to the custom domain name sim2real-probe.example.com.", got 'CustomDomainAssociation not found: sim2real-probe.example.com'

The cluster identifier and domain name shown come from the request; the surrounding wording is the requirement.

## Steps to Reproduce
1. Replay fixture redshift-partner-and-custom-domain-preconditions.
2. Inspect steps task#1 through task#8: each targets either a nonexistent ClusterIdentifier (task#1..task#7) or a custom domain name with no association (task#8).
3. Compare $.ErrorResponse.Error.Message against the recorded response for each step.
