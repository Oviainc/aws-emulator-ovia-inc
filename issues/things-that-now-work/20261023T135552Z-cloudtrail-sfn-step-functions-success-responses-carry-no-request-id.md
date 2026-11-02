Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__stepfunctions.obs-sfn-2-data-selector-logs-start-sync-execution-to-the-trail-prefix-only-after-it-is-added
Issue-Key: cloudtrail__stepfunctions.obs-sfn-2-data-selector-logs-start-sync-execution-to-the-trail-prefix-only-after-it-is-added|_marker#23|KeyError|return sfn.describe_state_machine(stateMachineArn=subject_arn)["ResponseMetadata"][
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Step Functions success responses carry no request id, so ResponseMetadata has no RequestId

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStateMachine

## Description
There is 1 cause in this issue.

Cause 1 of 1 — sfn successful responses omit the request id.
AWS: recorded behavior only: every Step Functions response carries a request id in the x-amzn-RequestId response header, which botocore surfaces as response['ResponseMetadata']['RequestId']. The check reads it off a successful DescribeStateMachine to mark a point in time it can later correlate against CloudTrail's record of the same call.
Emulator: the successful DescribeStateMachine response has no RequestId, and indexing response['ResponseMetadata']['RequestId'] raises KeyError: 'RequestId'. The call itself returned 200 with the state machine; only the request id is missing, and the check aborts there.
Why: a client cannot correlate a Step Functions call with its audit record, its logs or a support case. Any code that reads ResponseMetadata['RequestId'] — a very common idiom in correlation and retry bookkeeping — raises KeyError rather than degrading.
Fix: the Step Functions response serializer / the shared HTTP response layer must emit a per-request x-amzn-RequestId header on successful responses as well as error ones.
Traces: cloudtrail__sfn.obs-sfn-2-data-selector-logs-start-sync-execution-to-the-trail-prefix-only-after-it-is-added.

The check id carries the composite service token 'cloudtrail__sfn', which the per-service bootstrap gate cannot place by exact equality; cloudtrail and sfn are both stood up and neither appears in the pending set, so this is filed rather than deferred.

## Evidence
File "check_cloudtrail__sfn_obs_sfn_2_data_selector_logs_start_sync_execution_to_the_trail_prefix_only_after_it_is_added.py", line 119, in _marker -> return sfn.describe_state_machine(stateMachineArn=subject_arn)["ResponseMetadata"]["RequestId"] -> KeyError: 'RequestId'. Expected: ResponseMetadata carries a RequestId string on the successful DescribeStateMachine.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Replay cloudtrail__sfn.obs-sfn-2-data-selector-logs-start-sync-execution-to-the-trail-prefix-only-after-it-is-added.
3. Manually: create a state machine with sfn, then call sfn DescribeStateMachine and read response['ResponseMetadata']['RequestId'] -> expect a request id string, observe KeyError.
