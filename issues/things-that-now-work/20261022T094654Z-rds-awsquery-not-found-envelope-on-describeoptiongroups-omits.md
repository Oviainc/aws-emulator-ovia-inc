Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-query-envelope-shared-with-rds
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-query-envelope-shared-with-rds
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: rds|DescribeOptionGroups|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: 9cc0add35ee0fa09b41f1226e545932e7bbe5c43

# RDS awsQuery not-found envelope on DescribeOptionGroups omits the RequestId element

> _Note: this issue was identified against the code base at commit `9cc0add35ee0fa09b41f1226e545932e7bbe5c43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeOptionGroups

## Description
ONE cause.

Cause 1 of 1: the awsQuery error envelope returned by DescribeOptionGroups for an unknown option group carries no RequestId.
AWS: Smithy (rds, protocol awsQuery, version 2014-10-31) serializes an operation error as ErrorResponse{Error{Type,Code,Message}, RequestId}; DescribeOptionGroups's not-found fault is a client error with httpError 404. The recording asserts the RequestId element is present (shaped; its value is environment-specific and is NOT asserted). The status (404) and the error code in the recorded envelope are the requirements here; the message text is an example.
Emulator: the status 404 and the Error block match, but $.ErrorResponse.RequestId is missing entirely (got None).
Why: the fault cannot be correlated. SDK error objects surface RequestId for retry telemetry and support escalation, and XML consumers that treat ErrorResponse/RequestId as required fail to deserialize the fault, turning a modeled 404 into a parse error.
Fix: the shared awsQuery error serializer for the rds service — emit RequestId on every ErrorResponse, not only on success envelopes. The existing OPEN issue 20261022T092913Z-rds-awsquery-error-envelopes-omit-requestid-and-misclassify describes the same family of defect but does not carry this fixture step's replay key, so this step is not covered by it.
Traces: docdb-query-envelope-shared-with-rds#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, state_reset_failures 0, bootstrap_signal false).

Fixture docdb-query-envelope-shared-with-rds, step task#3, service rds, operation DescribeOptionGroups: expected status 404, actual 404 — $.ErrorResponse.RequestId: missing (expected a shaped value, got None). Divergence key: service=rds|DescribeOptionGroups|status=404->404|keys=$.ErrorResponse.RequestId. No stub_path and no owning issue were reported for this cluster.

Model reference: services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json (awsQuery, errors carry awsQueryError code + httpResponseCode).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture docdb-query-envelope-shared-with-rds step task#3: send an awsQuery DescribeOptionGroups (Version=2014-10-31) to the rds endpoint naming an OptionGroupName that does not exist.
4. Observe the 404 ErrorResponse XML: the Error element is present but there is no sibling RequestId element.
