Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative
Issue-Key: cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: 145c20a94e0bd5c99471752c6f247be888f6ff73

# cloudformation__lambda__sqs: a custom resource that answers Status=FAILED leaves the stack in ROLLBACK_FAILED instead of ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `145c20a94e0bd5c99471752c6f247be888f6ff73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the rollback a FAILED custom-resource response triggers never completes.
AWS: recorded behavior only for the terminal status (CloudFormation's Smithy model exposes `StackStatus` with `ROLLBACK_IN_PROGRESS`, `ROLLBACK_COMPLETE` and `ROLLBACK_FAILED` members but states no rule about which one a failed custom resource yields). The recording shows that when a custom resource PUTs a response document with `Status: FAILED` and a `Reason`, CloudFormation marks that logical resource `CREATE_FAILED` carrying the responder's Reason, rolls the stack back, and the stack settles in `ROLLBACK_COMPLETE`; every resource created before it is really deleted from its owning service (an SQS queue created earlier is gone from SQS, not merely reported `DELETE_COMPLETE`). The same template with `Status: SUCCESS` deploys to `CREATE_COMPLETE` and echoes its value.
Emulator: the stack settles in `ROLLBACK_FAILED`, so the rollback the FAILED response correctly started never finishes. (The check also calls out the neighbouring failure mode `CREATE_COMPLETE`, which would mean the response Status was ignored; the observed terminal status here is ROLLBACK_FAILED.)
Why: `ROLLBACK_FAILED` is a stack state AWS only reaches when rollback itself breaks, and it is not updatable by a plain UpdateStack — a client that deploys a template with a custom resource, gets a legitimate FAILED signal, and expects to retry after fixing the responder is instead left with a wedged stack that needs ContinueUpdateRollback/DeleteStack. Any CI pipeline treating ROLLBACK_COMPLETE as "clean failure, safe to redeploy" misreads the emulator's state.
Fix: the CloudFormation stack rollback engine — the rollback executor that deletes the already-created resources of a failed create and computes the stack's terminal status (the cloudformation service's stack-state component), together with the custom-resource response handling that feeds it the CREATE_FAILED reason.
Traces: cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative.

## Evidence
AssertionError: a custom resource that answered FAILED left the stack in ROLLBACK_FAILED, expected ROLLBACK_COMPLETE (CREATE_COMPLETE here means the response Status was ignored). Raised from check_cloudformation__lambda__sqs_cfn_custom_resource_failed_response_rollback_negative.py line 60, after _deploy_cfn_expect_terminal(..., ResponseStatus='FAILED', FailureReason=<reason>, timeout_s=900).

## Steps to Reproduce
1. Create an SQS recorder queue.
2. CreateStack with a template containing an SQS queue plus a Lambda-backed custom resource whose responder PUTs {"Status":"FAILED","Reason":"<reason>"} to the pre-signed response URL (CAPABILITY_IAM).
3. Poll DescribeStacks until terminal: the stack reports ROLLBACK_FAILED; AWS reports ROLLBACK_COMPLETE with a CREATE_FAILED event carrying the responder's Reason and the earlier queue actually deleted from SQS.
4. Deploy the same template with ResponseStatus=SUCCESS as the positive control.
