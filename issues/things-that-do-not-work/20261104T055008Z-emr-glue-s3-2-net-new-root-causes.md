Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-config-positive emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-permission-missing-negative emr__glue__s3.cfn-deploy-emr-cluster-missing-instance-profile-negative emr__glue__s3.emr-cluster-bootstrap-action-script-fetched-from-s3-positive emr__glue__s3.emr-cluster-configured-with-glue-catalog-metastore-positive emr__glue__s3.emr-cluster-glue-catalog-disabled-configuration-negative emr__glue__s3.emr-cluster-glue-catalog-missing-permission-negative emr__glue__s3.emr-cluster-instance-role-trust-mismatch-negative emr__glue__s3.emr-cluster-step-reads-input-from-s3-positive emr__glue__s3.emr-cluster-step-writes-output-to-s3-positive emr__glue__s3.emr-hive-query-reads-glue-catalog-table-positive emr__glue__s3.emr-spark-job-writes-result-via-glue-catalog-table-location-s3-positive emr__glue__s3.emr-step-missing-s3-input-read-permission-negative emr__glue__s3.eventbridge-emr-cluster-state-change-rule-positive emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive emr__glue__s3.cli-emr-create-cluster-submit-step-s3-workflow emr__glue__s3.emr-cluster-logs-uploaded-to-s3-log-uri-positive
Issue-Key: emr__glue__s3|CreateVpcEndpoint|NotImplemented
Issue-Key: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: emr__glue__s3.cli-emr-create-cluster-submit-step-s3-workflow|_poll_until#15|Timeout|return last
Issue-Key: emr__glue__s3.emr-cluster-logs-uploaded-to-s3-log-uri-positive|_poll_cluster_state#10|Timeout|time.sleep(interval_s)
Issue-Commit: 04fa8901c0470ed069cb581dfd2246db1b5bced9

# emr__glue__s3: 2 net-new root causes — CreateVpcEndpoint with route tables refused, and EMR clusters/steps never run or upload logs

> _Note: this issue was identified against the code base at commit `04fa8901c0470ed069cb581dfd2246db1b5bced9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
Two distinct net-new root causes under the emr__glue__s3 heading. Both must be closed.

Cause 1 of 2: EC2 CreateVpcEndpoint refuses any request that carries RouteTableIds, which aborts every EMR cluster check that first builds a gateway endpoint for S3.
AWS: Smithy (ec2, amazonec2) models CreateVpcEndpoint with RouteTableIds as an ordinary member of CreateVpcEndpointRequest and declares no modelled errors; a Gateway endpoint created with RouteTableIds returns 200 with a CreateVpcEndpointResult and installs the service prefix-list route in each named route table.
Emulator: answers error code 'NotImplemented' with the message 'operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator'. The checks abort before any EMR call is made. An open issue reports the same stub under other headings (20261104T044826Z-ec2-lambda-s3-createvpcendpoint-with-route-table-associations-is-refused-notimplemented.md and 20261104T044826Z-ec2-lambda-s3-sqs-createvpcendpoint-refuses-route-table-associations-and-masks-the.md); the replay report does not mark these checks as owned by either.
Why: no private S3 path can be built, so the entire EMR-on-S3/Glue-catalog surface is unreachable, and the refusal is server-shaped ('Receiver'/NotImplemented) so client retries never converge.
Fix: the CreateVpcEndpoint handler in the ec2 service crate — the RouteTableIds branch and the route-table/prefix-list state it must write.
Traces: emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-config-positive, emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-permission-missing-negative, emr__glue__s3.cfn-deploy-emr-cluster-missing-instance-profile-negative, emr__glue__s3.emr-cluster-bootstrap-action-script-fetched-from-s3-positive, emr__glue__s3.emr-cluster-configured-with-glue-catalog-metastore-positive, emr__glue__s3.emr-cluster-glue-catalog-disabled-configuration-negative, emr__glue__s3.emr-cluster-glue-catalog-missing-permission-negative, emr__glue__s3.emr-cluster-instance-role-trust-mismatch-negative, emr__glue__s3.emr-cluster-step-reads-input-from-s3-positive, emr__glue__s3.emr-cluster-step-writes-output-to-s3-positive, emr__glue__s3.emr-hive-query-reads-glue-catalog-table-positive, emr__glue__s3.emr-spark-job-writes-result-via-glue-catalog-table-location-s3-positive, emr__glue__s3.emr-step-missing-s3-input-read-permission-negative, emr__glue__s3.eventbridge-emr-cluster-state-change-rule-positive

Cause 2 of 2: an EMR cluster accepts a step but never executes it and never uploads cluster logs to its LogUri.
AWS: recorded behaviour — RunJobFlow with a step drives the cluster through STARTING/BOOTSTRAPPING/RUNNING, the step reaches COMPLETED, the step's work appears in S3, and the cluster writes its logs under <LogUri>/<cluster-id>/ (which survive TerminateJobFlows).
Emulator: the polled condition is never reached; each check exhausts its budget with 'Check timed out after 60.0s' — no step output and no log objects under LogUri. An open issue reports the non-executing-steps behaviour for sibling checks (20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md); these three are not marked as owned by it.
Why: a client that submits a step and waits for COMPLETED, or that collects cluster logs for diagnosis, waits out its whole timeout and sees an empty bucket.
Fix: the EMR step execution and cluster-log upload paths in the emr service crate (RunJobFlow/AddJobFlowSteps step state machine and the LogUri writer).
Traces: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive, emr__glue__s3.cli-emr-create-cluster-submit-step-s3-workflow, emr__glue__s3.emr-cluster-logs-uploaded-to-s3-log-uri-positive

## Evidence
Cause 1 — replay detail, verbatim: 'botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator'. Expected: HTTP 200 CreateVpcEndpointResult with the prefix-list route installed in the named route table. Actual: NotImplemented.
Cause 2 — replay detail: 'Error: Check timed out after 60.0s' on all three checks. Expected: the step reaching COMPLETED and log objects under <LogUri>/<cluster-id>/ within the polling budget. Actual: nothing written, condition never reached.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite. 2. Replay the emr__glue__s3 checks in trace_ids. 3. Cause 1: call ec2 CreateVpcEndpoint with VpcEndpointType=Gateway, ServiceName=com.amazonaws.<region>.s3 and RouteTableIds=[<rtb-id>]. 4. Cause 2: RunJobFlow with a LogUri and one step, poll DescribeStep/ListSteps for COMPLETED and list <LogUri>/<cluster-id>/ in S3, then TerminateJobFlows and list again.
