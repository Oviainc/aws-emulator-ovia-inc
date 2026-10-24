Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow
Issue-Key: cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# cloudformation__cloudwatch__lambda__logs__sqs__ssm: the change-set queue-worker update workflow never completes — the SQS-triggered Lambda worker is never executed (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the deployed change-set workflow stalls because its SQS-triggered Lambda worker never runs ===
AWS: a CloudFormation change set over a stack containing an SQS queue, a Lambda worker with an event source mapping on that queue, an SSM parameter and a CloudWatch alarm predicts per-resource Action / Replacement / Target.RequiresRecreation; executing it then produces exactly that — the replaced SSM parameter gets a new physical id, the modified function keeps its physical id and its event source mapping, the queue's CreatedTimestamp survives, and a message sent after the update is PROCESSED with the new configuration, publishing a custom-metric datapoint that drives the declared alarm out of INSUFFICIENT_DATA.
Emulator: the workflow never reaches a verdict and runs to the suite's 60 s cap. Its load-bearing leg is "a message sent after the update is processed by the worker", and the emulator has no in-process Lambda runtime (open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md records lambda Invoke answering NotImplemented for a real deployment package), so no message is ever consumed, no metric is ever published, and the alarm never leaves INSUFFICIENT_DATA. None of the ten legs can be graded.
Why: this is the only check that grades CloudFormation's change-set prediction against reality for a mixed create-only/updatable property update. A client is left unable to trust the one answer CloudFormation gives to "will this replace my resource?", and the stall gives no indication of which leg failed.
Fix: the Lambda invocation runtime and the SQS event-source-mapping poller that feeds it; then the change-set prediction/execution path in the CloudFormation handler that the remaining legs grade.
Traces: cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow.

## Evidence
cloudformation__cloudwatch__lambda__logs__sqs__ssm.cfn-change-set-queue-worker-update-workflow: 'Check timed out after 60.0s' with no leg reported complete. The workflow (10 legs) depends on the deployed worker consuming a message and publishing the SimParity/QueueWorker MessagesProcessed datapoint; neither occurred, so the drift-detection, denied-invocation and post-delete legs were never reached either. Expected: the change set's per-resource Action/Replacement fields, then the observed post-execution reality (new SSM physical id, byte-identical function physical id, surviving queue CreatedTimestamp, message processed with the new configuration, alarm leaving INSUFFICIENT_DATA).

## Steps to Reproduce
cfn CreateStack from a queue-worker template (AWS::SQS::Queue, AWS::Lambda::Function + AWS::Lambda::EventSourceMapping on the queue, AWS::SSM::Parameter, AWS::CloudWatch::Alarm); cfn CreateChangeSet with an update that touches a create-only property on the SSM parameter and an updatable property on the function; DescribeChangeSet, ExecuteChangeSet; sqs SendMessage and poll for the worker's effect (the results it writes and the custom-metric datapoint) — nothing is ever processed and the check hangs to the cap.
