Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.cli-emr-create-cluster-submit-step-s3-workflow
Issue-Key: s3|HeadObject|404
Issue-Commit: a27fb6d1af0279893787d520ac4dc13c8147b724

# emr__glue__s3: a CLI-created auto-terminating cluster's CUSTOM_JAR step writes no S3 output — HeadObject of the step's declared key answers 404 after the cluster reaches TERMINATED

> _Note: this issue was identified against the code base at commit `a27fb6d1af0279893787d520ac4dc13c8147b724`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddJobFlowSteps

## Description
Cause 1 of 1: a cluster created with a CUSTOM_JAR step and --auto-terminate runs the cluster lifecycle to TERMINATED but never executes the step's command, so the object the step was supposed to write never exists in S3.
AWS: `aws emr create-cluster --steps '[{"Type":"CUSTOM_JAR","Jar":"command-runner.jar","Args":["bash","-c","... aws s3 cp ... s3://<bucket>/<key>"]}]' --auto-terminate` submits a step the cluster actually runs: command-runner.jar executes the given command line on the primary node, the step reaches COMPLETED, the cluster then auto-terminates to TERMINATED (not TERMINATED_WITH_ERRORS), and the step's own side effect is real — HeadObject of the key the step wrote answers HTTP 200 with ContentLength > 0. Smithy (com.amazonaws.emr): StepConfig carries HadoopJarStep (Jar/Args) and ActionOnFailure, and StepState models PENDING/RUNNING/COMPLETED/FAILED/CANCELLED/INTERRUPTED; the model says nothing about execution, so the step's side effect is recorded behavior only.
Emulator: the cluster reaches TERMINATED as expected, but the step's command is never run, so the declared output key is absent and S3 answers HTTP 404 to HeadObject — botocore reports `An error occurred (404) when calling the HeadObject operation: Not Found` (HeadObject has no body, so 404 with no error code is S3's correct answer for a missing key; the defect is the missing object, not the S3 response). The bucket name and object key in the recording are per-run values; the requirement is that the key the step declares exists and is non-empty once the step has run.
Why: any CLI- or SDK-driven EMR pipeline that submits work and then consumes the step's artifacts from S3 breaks at the first read: the client sees a cleanly TERMINATED cluster (a success signal) with no output behind it, so a workflow cannot distinguish "the job ran and produced nothing" from "the job never ran", and downstream stages fail on a missing object instead of on a failed step.
Fix: the EMR step executor behind AddJobFlowSteps/RunJobFlow Steps in the elasticmapreduce service crate — the step must actually run its HadoopJarStep (command-runner.jar command line included) and perform its S3 writes before the auto-terminating cluster moves to TERMINATED.
Traces: emr__glue__s3.cli-emr-create-cluster-submit-step-s3-workflow (cluster reaches TERMINATED, then `s3.head_object(Bucket=<bucket>, Key="output/out.txt")` raises `An error occurred (404) when calling the HeadObject operation: Not Found`). Context: open issues 20261108T104909Z-emr-glue-s3-2-net-new-causes-cluster.md (same no-application-executed cause, grounded in GetObject/NoSuchKey and DescribeStep FAILED keys), 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md and 20261105T074335Z-emr-glue-s3-emr-cluster-steps-never-reach.md describe neighbouring symptoms but none carries this check's replay key — note that in THIS check the cluster does reach TERMINATED and the failure surfaces purely as the step's missing S3 side effect.

## Evidence
Expected (AWS): after `aws emr create-cluster ... --auto-terminate --steps` with a CUSTOM_JAR/command-runner.jar step whose command copies a file to s3://<bucket>/output/out.txt, DescribeCluster reports Status.State=TERMINATED and HeadObject(Bucket=<bucket>, Key="output/out.txt") answers HTTP 200 with ContentLength > 0.
Actual (emulator): DescribeCluster reports TERMINATED, but HeadObject of that key fails: `botocore.exceptions.ClientError: An error occurred (404) when calling the HeadObject operation: Not Found` — the step's command was never executed, so no object was ever written (HeadObject carries no error code or message by design; the divergence is the absent object).

## Steps to Reproduce
1. Create an S3 bucket and an EMR service role + EC2 instance profile; pick a subnet.
2. Run `aws emr create-cluster --name <n> --release-label emr-6.15.0 --applications Name=Hadoop --instance-type m5.xlarge --instance-count 1 --service-role <role> --ec2-attributes InstanceProfile=<profile>,SubnetId=<subnet> --log-uri s3://<bucket>/logs/ --auto-terminate --steps '[{"Name":"write-output","Type":"CUSTOM_JAR","ActionOnFailure":"CONTINUE","Jar":"command-runner.jar","Args":["bash","-c","echo hello > /tmp/out.txt && aws s3 cp /tmp/out.txt s3://<bucket>/output/out.txt"]}]'`.
3. Poll DescribeCluster until Status.State is TERMINATED or TERMINATED_WITH_ERRORS. Observed: TERMINATED (as expected).
4. Call HeadObject(Bucket=<bucket>, Key="output/out.txt"). Observed: 404 Not Found. Expected: 200 with ContentLength > 0 containing the bytes the step wrote.
