Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-account-level-reads-and-create-validation
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-account-level-reads-and-create-validation autoscaling-empty-account-describes autoscaling-pagination-invalid-next-token-errors autoscaling-delete-launch-configuration-never-created autoscaling-deletion-protection-blocks-delete-until-cleared autoscaling-group-dependent-operations-on-nonexistent-group
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|DescribeAutoScalingGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DescribeAutoScalingGroups|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: autoscaling|CreateAutoScalingGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|CreateAutoScalingGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: autoscaling|DeleteLaunchConfiguration|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DeleteLaunchConfiguration|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: autoscaling|DeleteAutoScalingGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DeleteAutoScalingGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: autoscaling|UpdateAutoScalingGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|UpdateAutoScalingGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: autoscaling|SetDesiredCapacity|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|SetDesiredCapacity|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: 6f223cb07d8f19a9e48b2b4f7aac2665ac919e98

# autoscaling awsQuery error envelopes omit the RequestId element and the InvalidNextToken message drops the rejected token

> _Note: this issue was identified against the code base at commit `6f223cb07d8f19a9e48b2b4f7aac2665ac919e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Two causes, both on the autoscaling error path. Close both before submitting.

Cause 1 of 2: the awsQuery error envelope has no RequestId element. AWS: for the awsQuery protocol (Smithy service AutoScaling_2011_01_01, protocols: [awsQuery], xmlNamespace http://autoscaling.amazonaws.com/doc/2011-01-01/), a failed request returns <ErrorResponse> containing <Error> (Type/Code/Message) AND a sibling <RequestId>. The element's PRESENCE is the requirement; its value is a fresh opaque id and is never asserted. Emulator: the ErrorResponse carries Error but no RequestId at all — the replay reports "$.ErrorResponse.RequestId: missing (... got None)" on every error-returning operation it reached: DescribeAutoScalingGroups (invalid NextToken), CreateAutoScalingGroup (missing launch source), DeleteLaunchConfiguration (unknown name), DeleteAutoScalingGroup (deletion protection), UpdateAutoScalingGroup and SetDesiredCapacity (unknown group). Status codes and Error/Code values are correct in all of them — only the envelope member is absent. Why: SDKs and support tooling read the request id off the error envelope to correlate and to populate the exception's request-id field; with the element absent, every autoscaling failure is unattributable and any client that parses the envelope strictly sees a malformed error document. This is the same class of defect already filed for other awsQuery/REST-XML services (see open issue 20261022T092913Z-rds-awsquery-error-envelopes-omit-requestid-and-misclassify), which does not cover autoscaling. Fix: the autoscaling crate's awsQuery error serializer (shared error-envelope writer, services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01) must emit <RequestId> alongside <Error>, the same id it already returns in ResponseMetadata on success. Traces: autoscaling-account-level-reads-and-create-validation#task#6 and #task#7, autoscaling-delete-launch-configuration-never-created#task#2, autoscaling-deletion-protection-blocks-delete-until-cleared#task#5, autoscaling-group-dependent-operations-on-nonexistent-group#task#1 and #task#2.

Cause 2 of 2: the InvalidNextToken message does not name the token it rejected. AWS: DescribeAutoScalingGroups with a token the service never issued returns HTTP 400 with Error Code=InvalidNextToken and a message BUILT FROM THE REQUEST — it quotes the submitted token back (recorded example: "The token 'garbage-token-not-issued-by-service' was invalid"). That request-derived rule is the requirement; the surrounding wording is an example. Smithy: InvalidNextToken is a client error, awsQueryError code InvalidNextToken, httpError 400. Emulator: correct 400 and correct Code, but a constant message ('The NextToken value is not valid.') that never echoes the offending token. Why: a client paginating several lists concurrently uses the echoed token to identify WHICH cursor went stale; a constant message makes the failure undiagnosable from the response alone. Fix: the DescribeAutoScalingGroups pagination validation — format the InvalidNextToken message from the received NextToken value. Traces: autoscaling-account-level-reads-and-create-validation#task#6 (also autoscaling-empty-account-describes#task#6, autoscaling-pagination-invalid-next-token-errors#task#1).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline. Six clusters report the missing envelope member; HTTP status and Error/Code match AWS in all of them.

Cause 1 (RequestId missing), one binding per cluster:
- DescribeAutoScalingGroups, autoscaling-account-level-reads-and-create-validation#task#6: "$.ErrorResponse.RequestId: missing (expected '577596b9-296d-43df-ac57-6487814d7a77', got None)" (value is per-request and not a requirement). 3 fixtures affected.
- CreateAutoScalingGroup, autoscaling-account-level-reads-and-create-validation#task#7: "$.ErrorResponse.RequestId: missing (expected '85e68a03-...', got None)".
- DeleteLaunchConfiguration, autoscaling-delete-launch-configuration-never-created#task#2: "$.ErrorResponse.RequestId: missing (expected '71646c98-...', got None)".
- DeleteAutoScalingGroup, autoscaling-deletion-protection-blocks-delete-until-cleared#task#5: "$.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>', got None)". 3 fixtures affected.
- UpdateAutoScalingGroup, autoscaling-group-dependent-operations-on-nonexistent-group#task#1: "$.ErrorResponse.RequestId: missing (expected '9f3c80ec-...', got None)".
- SetDesiredCapacity, autoscaling-group-dependent-operations-on-nonexistent-group#task#2: "$.ErrorResponse.RequestId: missing (expected '52792cb9-...', got None)".

Cause 2 (InvalidNextToken message), same representative step: "$.ErrorResponse.Error.Message: value_mismatch (expected \"The token 'garbage-token-not-issued-by-service' was invalid\", got 'The NextToken value is not valid.')" — expected_status 400, actual_status 400, Code InvalidNextToken on both sides; the expected text embeds the token submitted in the request.

Model evidence (smithy_query, services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): service AutoScaling_2011_01_01, protocols [awsQuery]; shape InvalidNextToken = client error, httpError 400, awsQueryError {code: InvalidNextToken, httpResponseCode: 400}.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Replay autoscaling-account-level-reads-and-create-validation step task#6 (DescribeAutoScalingGroups with NextToken=garbage-token-not-issued-by-service): the body is 400 ErrorResponse/Error with Code=InvalidNextToken but has NO <RequestId> sibling, and the Message does not contain the submitted token.
3. Replay any other failing autoscaling call — e.g. autoscaling-account-level-reads-and-create-validation#task#7 (CreateAutoScalingGroup with no launch source), autoscaling-delete-launch-configuration-never-created#task#2, autoscaling-deletion-protection-blocks-delete-until-cleared#task#5, autoscaling-group-dependent-operations-on-nonexistent-group#task#1 and #task#2 — and confirm <RequestId> is absent from each ErrorResponse.
4. After fixing, re-replay all six bindings listed above and assert a non-empty RequestId element is present in each error body and that the InvalidNextToken message quotes the rejected token.
