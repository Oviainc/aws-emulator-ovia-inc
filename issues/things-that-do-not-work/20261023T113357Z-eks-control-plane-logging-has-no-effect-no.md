Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.eks-enable-cluster-logging-creates-log-group ecr__eks__logs.eks-cluster-log-group-naming-follows-convention-per-cluster ecr__eks__logs.eks-audit-log-streams-delivered-to-cloudwatch ecr__eks__logs.eks-disabled-log-type-produces-no-log-group ecr__eks__logs.cli-eks-update-logging-config-workflow
Issue-Key: ecr__eks__logs.eks-enable-cluster-logging-creates-log-group|_poll_log_group_present#8|Timeout|time.sleep(interval_s)
Issue-Key: ecr__eks__logs.eks-cluster-log-group-naming-follows-convention-per-cluster|_poll_log_group_present#10|Timeout|time.sleep(interval_s)
Issue-Key: ecr__eks__logs.eks-audit-log-streams-delivered-to-cloudwatch|_poll_log_group_present#8|Timeout|time.sleep(interval_s)
Issue-Key: ecr__eks__logs.eks-disabled-log-type-produces-no-log-group|_poll_log_group_present#8|Timeout|time.sleep(interval_s)
Issue-Key: ecr__eks__logs.cli-eks-update-logging-config-workflow|run#11|CalledProcessError|proc = subprocess.run(
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# EKS control-plane logging has no effect: no /aws/eks/<cluster>/cluster log group is ever created and UpdateClusterConfig --logging fails (2 causes)

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateClusterConfig

## Description
Two net-new root causes in the EKS emulator. BOTH must be closed before submitting. The open issue 20261022T154613Z-ecr-eks-logs-ecr-uploadlayerpart-is-a-501-stub-and-a.md records the same missing log group for a CloudFormation-provisioned cluster; the causes below are the direct-API path and carry different check ids.

Cause 1 of 2: enabling a cluster log type never creates its CloudWatch Logs group.
AWS: when a cluster's logging configuration enables one or more types (api, audit, authenticator, controllerManager, scheduler), EKS creates the log group /aws/eks/<cluster-name>/cluster and delivers control-plane log streams into it; a type that is left disabled produces no stream, and each cluster gets its own group following that naming convention.
Emulator: recorded behavior only - after CreateCluster with enabled log types (or after enabling them), the checks poll DescribeLogGroups/DescribeLogStreams for /aws/eks/<cluster>/cluster and reach the 60s deadline with nothing created.
Why: the documented observability path for an EKS cluster yields no log group, so a client cannot read control-plane logs or assert that disabling a type suppresses them.
Fix: the cluster logging side effect in the eks service crate (create the derived log group on CreateCluster/UpdateClusterConfig and vend control-plane streams into it).
Traces: ecr__eks__logs.eks-enable-cluster-logging-creates-log-group, ecr__eks__logs.eks-cluster-log-group-naming-follows-convention-per-cluster, ecr__eks__logs.eks-audit-log-streams-delivered-to-cloudwatch, ecr__eks__logs.eks-disabled-log-type-produces-no-log-group.

Cause 2 of 2: UpdateClusterConfig with a logging payload is refused.
AWS: UpdateClusterConfig accepts {"clusterLogging": [{"types": ["api"], "enabled": true}]} and returns an Update object (type LoggingUpdate) that settles Successful.
Emulator: the call fails - the AWS CLI exits 254 (a service error response) and no update is returned, so the logging configuration can never be changed after creation.
Why: a client that turns control-plane logging on after the fact cannot do so, and the CLI surfaces an error where AWS returns an Update.
Fix: the UpdateClusterConfig handler in the eks service crate - accept the logging member, persist it and return the LoggingUpdate.
Traces: ecr__eks__logs.cli-eks-update-logging-config-workflow.

## Evidence
Cause 1: Error: Check timed out after 60.0s while polling for /aws/eks/<cluster>/cluster and its streams (eks-enable-cluster-logging-creates-log-group, eks-cluster-log-group-naming-follows-convention-per-cluster, eks-audit-log-streams-delivered-to-cloudwatch, eks-disabled-log-type-produces-no-log-group). Expected: the derived log group exists for enabled types and does not for disabled ones.
Cause 2: subprocess.CalledProcessError: Command '['aws', 'eks', 'update-cluster-config', '--name', 'sim2real-eks-d9efb6d49b09', '--logging', '{"clusterLogging": [{"types": ["api"], "enabled": true}]}', '--region', 'us-west-2', '--output', 'json']' returned non-zero exit status 254. Expected: exit 0 with an Update of type LoggingUpdate.

## Steps to Reproduce
Cause 1: aws eks create-cluster ... --logging '{"clusterLogging":[{"types":["api","audit"],"enabled":true}]}'; then poll aws logs describe-log-groups --log-group-name-prefix /aws/eks/<cluster>/cluster -> never appears.
Cause 2: aws eks update-cluster-config --name <cluster> --logging '{"clusterLogging": [{"types": ["api"], "enabled": true}]}' -> exit 254.
