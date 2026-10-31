Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__ec2.obs-ec2-3-rejected-ec2-calls-audit-client-error-codes-with-no-response-elements
Issue-Key: cloudtrail__ec2|DescribeInstances|InvalidInstanceID.Malformed
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# EC2 error responses carry no request id, so ResponseMetadata has no RequestId

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
There is 1 cause in this issue.

Cause 1 of 1 — EC2 client-error responses omit the request id entirely.
AWS: recorded behavior only: every EC2 response, success or error, carries a request id — for the ec2Query protocol as the <RequestID> element of the error envelope (and the x-amzn-RequestId header) — which botocore surfaces as response['ResponseMetadata']['RequestId']. The check reads that value off a refused DescribeInstances so it can join the SDK error to the CloudTrail audit record for the same call.
Emulator: the refused DescribeInstances response contains no RequestId at all, and indexing err.response['ResponseMetadata']['RequestId'] raises KeyError: 'RequestId'. The check never reaches its CloudTrail assertions; the status and error code of DescribeInstances itself were not the failing element.
Why: a client cannot correlate a failed EC2 call with anything — support escalation, log correlation, CloudTrail joins and idempotency bookkeeping all key on the request id. Any code that reads ResponseMetadata['RequestId'] crashes rather than degrading.
Fix: the ec2Query error serializer must emit the per-request id into the error envelope (and the response header) alongside Code and Message.
Note: the open issue 20261022T102725Z-/20261023T102725Z-ec2-ec2query-error-envelope-omits-the-requestid-element.md records the same absence from a different grounding; the suite did not mark this check as owned by it, so this is filed with this check's replay key and the two should be closed together.
Traces: cloudtrail__ec2.obs-ec2-3-rejected-ec2-calls-audit-client-error-codes-with-no-response-elements.

## Evidence
File "check_cloudtrail__ec2_obs_ec2_3_rejected_ec2_calls_audit_client_error_codes_with_no_response_elements.py", line 252, in _expect_client_error -> err.response["ResponseMetadata"]["RequestId"] -> KeyError: 'RequestId', raised while handling the refusal of ec2.describe_instances(InstanceIds=['<nonexistent instance id>']). Expected: ResponseMetadata carries a RequestId string for the refused call.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Replay cloudtrail__ec2.obs-ec2-3-rejected-ec2-calls-audit-client-error-codes-with-no-response-elements.
3. Manually: call ec2 DescribeInstances with an InstanceIds entry that does not exist, catch the ClientError and read exc.response['ResponseMetadata']['RequestId'] -> expect a request id string, observe KeyError.
