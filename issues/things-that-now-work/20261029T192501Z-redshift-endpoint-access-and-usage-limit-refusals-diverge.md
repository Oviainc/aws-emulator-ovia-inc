Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-endpoint-access-ops-not-found-semantics
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-endpoint-access-ops-not-found-semantics redshift-usage-limit-endpoint-and-scheduled-action-preconditions
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeEndpointAccess|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeleteEndpointAccess|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|ModifyEndpointAccess|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|AuthorizeEndpointAccess|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|RevokeEndpointAccess|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeEndpointAuthorization|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|AuthorizeEndpointAccess|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|AuthorizeEndpointAccess|4xx->4xx|status
Issue-Key: redshift|RevokeEndpointAccess|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|RevokeEndpointAccess|4xx->4xx|status
Issue-Key: redshift|ModifyUsageLimit|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeleteUsageLimit|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift endpoint-access and usage-limit refusals diverge in message, code and validation precedence

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpointAccess

## Description
10 root causes on the Redshift-managed VPC endpoint and usage-limit surfaces. Eight are message-wording divergences; two (causes 7 and 8) are wrong HTTP status AND wrong error code because the emulator runs its cluster-existence lookup before AWS's input validation.

Cause 1 of 10: DescribeEndpointAccess for an unknown EndpointName. Smithy: EndpointNotFoundFault @error("client"), httpError 404. AWS: 404, Message "Endpoint <EndpointName> can't be found" — fixed text "Endpoint " + " can't be found" (no trailing period on this one), variable part is the request's EndpointName. Emulator: "Endpoint <EndpointName> not found." Why: message-matching clients fail to classify the refusal. Fix: the DescribeEndpointAccess handler's endpoint-lookup refusal path. Traces: redshift-endpoint-access-ops-not-found-semantics#task#1.

Cause 2 of 10: DeleteEndpointAccess for an unknown EndpointName. AWS: 404 EndpointNotFound, Message "The endpoint name <EndpointName> can't be found." — a DIFFERENT fixed template from cause 1 ("The endpoint name " + " can't be found."). Emulator: "Endpoint <EndpointName> not found." Fix: DeleteEndpointAccess handler refusal path. Traces: redshift-endpoint-access-ops-not-found-semantics#task#2.

Cause 3 of 10: ModifyEndpointAccess for an unknown EndpointName. AWS: 404 EndpointNotFound, "The endpoint name <EndpointName> can't be found." Emulator: "Endpoint <EndpointName> not found." Fix: ModifyEndpointAccess handler refusal path. Traces: redshift-endpoint-access-ops-not-found-semantics#task#3.

Cause 4 of 10: AuthorizeEndpointAccess with an unknown ClusterIdentifier. Smithy: ClusterNotFoundFault httpError 404, awsQueryError ClusterNotFound. AWS: 404, Message "The specified cluster identifier can't be found." — a FIXED message with NO variable part: AWS does not echo the identifier here. Emulator: "Cluster <ClusterIdentifier> not found.", echoing the request value into a message that AWS leaves constant. Fix: AuthorizeEndpointAccess handler cluster-lookup refusal path. Traces: redshift-endpoint-access-ops-not-found-semantics#task#5.

Cause 5 of 10: RevokeEndpointAccess with an unknown ClusterIdentifier. AWS: 404 ClusterNotFound, fixed "The specified cluster identifier can't be found." Emulator: "Cluster <ClusterIdentifier> not found." Fix: RevokeEndpointAccess handler cluster-lookup refusal path. Traces: redshift-endpoint-access-ops-not-found-semantics#task#6.

Cause 6 of 10: DescribeEndpointAuthorization with an unknown ClusterIdentifier. AWS: 404 ClusterNotFound, fixed "The specified cluster identifier can't be found." Emulator: "Cluster <ClusterIdentifier> not found." Fix: DescribeEndpointAuthorization handler cluster-lookup refusal path. Traces: redshift-endpoint-access-ops-not-found-semantics#task#7.

Cause 7 of 10: AuthorizeEndpointAccess with Account equal to the caller's own account id (and a nonexistent cluster). AWS validates the Account argument FIRST and refuses 400 InvalidParameterValue with the fixed message "Specify an AWS account that is different from the current AWS account." (no variable part). Emulator skips that validation entirely and instead resolves the cluster, answering 404 ClusterNotFound "Cluster <ClusterIdentifier> not found." Why: a client probing the self-authorization guard sees a 404 retryable-looking not-found instead of a 400 permanent input error, and never learns the real reason; status class, error code and message are all wrong. Fix: AuthorizeEndpointAccess input validation must run the Account != own-account check before the cluster lookup. Traces: redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#7.

Cause 8 of 10: RevokeEndpointAccess with Account equal to the caller's own account id. AWS: 400 InvalidParameterValue, fixed "Specify an AWS account that is different from the current AWS account." Emulator: 404 ClusterNotFound "Cluster <ClusterIdentifier> not found." Why/Fix: as cause 7, in RevokeEndpointAccess's validation ordering. Traces: redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#8.

Cause 9 of 10: ModifyUsageLimit for an unknown UsageLimitId. Smithy: UsageLimitNotFoundFault httpError 404. AWS: 404, Message "Usage limit <UsageLimitId> is not found" — fixed text "Usage limit " + " is not found" with no trailing period. Emulator: "Usage limit <UsageLimitId> not found." Fix: ModifyUsageLimit handler refusal path. Traces: redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#2.

Cause 10 of 10: DeleteUsageLimit for an unknown UsageLimitId. AWS: 404, "Usage limit <UsageLimitId> is not found". Emulator: "Usage limit <UsageLimitId> not found." Fix: DeleteUsageLimit handler refusal path. Traces: redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#3.

## Evidence
Replay of simparity-wire-aws-v1, all ten clusters verdict=mismatch.

DescribeEndpointAccess (redshift-endpoint-access-ops-not-found-semantics#task#1), 404 vs 404:
  expected "Endpoint sim2real-probe-ep-nf-01 can't be found", got 'Endpoint sim2real-probe-ep-nf-01 not found.'
DeleteEndpointAccess (#task#2) and ModifyEndpointAccess (#task#3), 404 vs 404:
  expected "The endpoint name sim2real-probe-ep-nf-01 can't be found.", got 'Endpoint sim2real-probe-ep-nf-01 not found.'
AuthorizeEndpointAccess (#task#5), RevokeEndpointAccess (#task#6), DescribeEndpointAuthorization (#task#7), 404 vs 404:
  expected "The specified cluster identifier can't be found.", got 'Cluster sim2real-probe-redshift-cluster-nf-01 not found.'
AuthorizeEndpointAccess (redshift-usage-limit-endpoint-and-scheduled-action-preconditions#task#7) and RevokeEndpointAccess (#task#8):
  status: expected 400, got 404
  Error.Code: expected 'InvalidParameterValue', got 'ClusterNotFound'
  Error.Message: expected 'Specify an AWS account that is different from the current AWS account.', got 'Cluster sim2real-probe-cluster-missing not found.'
ModifyUsageLimit (#task#2) and DeleteUsageLimit (#task#3), 404 vs 404:
  expected 'Usage limit sim2real-probe-ul-missing is not found', got 'Usage limit sim2real-probe-ul-missing not found.'

Endpoint names, cluster identifiers and usage-limit ids shown are request values; the fixed wording and, for causes 7-8, the 400/InvalidParameterValue classification are the requirements.

## Steps to Reproduce
1. Replay fixture redshift-endpoint-access-ops-not-found-semantics and inspect steps task#1, task#2, task#3 (unknown EndpointName) and task#5, task#6, task#7 (unknown ClusterIdentifier); compare Error.Message with the recordings.
2. Replay fixture redshift-usage-limit-endpoint-and-scheduled-action-preconditions and inspect steps task#7 and task#8: AuthorizeEndpointAccess / RevokeEndpointAccess with Account set to the caller's own account id. Observe 404 ClusterNotFound instead of 400 InvalidParameterValue.
3. Inspect steps task#2 and task#3 of the same fixture (unknown UsageLimitId) and compare Error.Message.
