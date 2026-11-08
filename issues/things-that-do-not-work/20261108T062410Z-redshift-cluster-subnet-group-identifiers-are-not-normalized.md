Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-rshealth-describe-identifier-mistakes
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-rshealth-describe-identifier-mistakes
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeClusterSubnetGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 5efea226428cd2abb66c0a11f0d4b2dd794f4348

# Redshift cluster subnet-group identifiers are not normalized to lowercase, so the ClusterSubnetGroupNotFoundFault message echoes the request's mixed case

> _Note: this issue was identified against the code base at commit `5efea226428cd2abb66c0a11f0d4b2dd794f4348`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterSubnetGroups

## Description
One cause.

Cause 1 of 1: DescribeClusterSubnetGroups refuses an unknown ClusterSubnetGroupName with the right status and the right error code, but the message names the identifier exactly as the request spelled it instead of the lowercased identifier Amazon Redshift works with.

AWS: Amazon Redshift treats a cluster-subnet-group name as a lowercase identifier — Smithy: CreateClusterSubnetGroup.ClusterSubnetGroupName documents "The name for the subnet group. Amazon Redshift stores the value as a lowercase string." Because the stored/resolved identifier is the lowercase form, the not-found refusal for DescribeClusterSubnetGroups (Smithy: DescribeClusterSubnetGroups errors ClusterSubnetGroupNotFoundFault, awsQueryError code ClusterSubnetGroupNotFoundFault, httpError 400) names that lowercase form. The recorded response is HTTP 400, Error.Type Sender, Error.Code ClusterSubnetGroupNotFoundFault, with message text of the fixed form "Cluster Subnet group '<name>' not found." — the fixed text is "Cluster Subnet group '" ... "' not found." and the single variable part is <name>, the requested ClusterSubnetGroupName AFTER lowercasing (in the recording the request's mixed-case name appears in the message fully lower-cased).

Emulator: same status 400 and same error code, but the message interpolates the raw request value verbatim, preserving its upper-case characters, so the message's variable part differs from AWS's: expected "Cluster Subnet group 'sim2real_probe_rshealth_b76fbc' not found.", actual "Cluster Subnet group 'Sim2real_Probe_RSHealth_b76fbc' not found." Hypothesis: the handler never applies Redshift's lowercase normalization to the identifier — neither when echoing it into the refusal nor, by implication, when storing/looking one up — so a subnet group created as "MyGroup" and described as "mygroup" (or the reverse) would also not resolve.

Why: a client that creates a subnet group with any upper-case character gets back, from AWS, a resource whose canonical name is lowercase, and every later read/compare/CloudFormation drift check uses that lowercase name. Against the emulator the identifier is case-preserving, so a case-insensitive lookup that works on AWS fails here, and error-message assertions and log scrapers that match AWS's canonical (lowercased) identifier do not match. The refusal message itself is a requirement, so the mixed-case rendering is a wire defect even where lookup happens to succeed.

Fix: the redshift cluster-subnet-group handling — normalize ClusterSubnetGroupName to lowercase at the service boundary (create/modify/delete/describe) so both the state key and the ClusterSubnetGroupNotFoundFault message formatting in the DescribeClusterSubnetGroups handler use the lowercase identifier.

Not covered by any open issue: the adjacent open issues 20261103T040731Z-redshift-describe-path-request-validation-answers-the-wrong (describe-path status/taxonomy) and 20261104T045922Z-redshift-control-plane-cfn-provisioned-subnet-group-invisible (CFN-provisioned subnet group invisibility, no subnet validation) concern different defects and neither mentions identifier case normalization; the replay reports this cluster as unowned.

Traces: redshift-rshealth-describe-identifier-mistakes#task#10.

## Evidence
Fixture redshift-rshealth-describe-identifier-mistakes, step task#10, operation DescribeClusterSubnetGroups (service redshift), verdict mismatch, expected_status 400 / actual_status 400.

Divergence detail (verbatim from the replay):
  $.ErrorResponse.Error.Message: value_mismatch (expected "Cluster Subnet group 'sim2real_probe_rshealth_b76fbc' not found.", got "Cluster Subnet group 'Sim2real_Probe_RSHealth_b76fbc' not found.")

Expected (AWS): HTTP 400, ErrorResponse.Error.Code ClusterSubnetGroupNotFoundFault, Message "Cluster Subnet group '<lowercased ClusterSubnetGroupName>' not found."
Actual (emulator): HTTP 400, same code, Message "Cluster Subnet group '<ClusterSubnetGroupName exactly as sent>' not found."

Model citation: Smithy CreateClusterSubnetGroupMessage.ClusterSubnetGroupName — "Amazon Redshift stores the value as a lowercase string"; DescribeClusterSubnetGroups errors: ClusterSubnetGroupNotFoundFault (client, httpError 400, awsQueryError code ClusterSubnetGroupNotFoundFault).

The only non-fixed values in the expected message are the identifier itself; the status, the error code and the surrounding fixed text all match, so the divergence is isolated to the identifier's case.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture redshift-rshealth-describe-identifier-mistakes.
2. At step task#10 the fixture calls Redshift DescribeClusterSubnetGroups with a ClusterSubnetGroupName containing upper-case characters that names no existing subnet group.
3. Observe the 400 ErrorResponse: Error.Code is ClusterSubnetGroupNotFoundFault (correct), but Error.Message renders the requested name with its original upper-case characters instead of the lowercase identifier AWS reports.
4. Equivalent by hand: CreateClusterSubnetGroup with a mixed-case name, then DescribeClusterSubnetGroups with the all-lowercase spelling of the same name — AWS resolves it (the stored name is lowercase); the emulator's refusal/lookup is case-sensitive.
