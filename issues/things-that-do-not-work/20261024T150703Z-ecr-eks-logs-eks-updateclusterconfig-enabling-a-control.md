Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.cli-eks-update-logging-config-workflow
Issue-Key: ecr__eks__logs.cli-eks-update-logging-config-workflow|_poll_log_group_present#14|Timeout|time.sleep(interval_s)
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# ecr__eks__logs: eks UpdateClusterConfig enabling a control-plane log type never creates /aws/eks/<cluster>/cluster or any log stream (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateClusterConfig

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: enabling an EKS control-plane log type has no effect on CloudWatch Logs ===
AWS: eks UpdateClusterConfig with logging={'clusterLogging': [{'types': ['api'], 'enabled': True}]} returns an Update whose id reaches status Successful, and the enabled log type is then delivered to the log group /aws/eks/<cluster-name>/cluster — the group is created by the service if it does not exist, and at least one log stream appears under it.
Emulator: the update is accepted and reports success, but the log group /aws/eks/<cluster>/cluster never appears to DescribeLogGroups and no stream is ever created, so the check runs to the suite's 60 s cap polling for them (its polls return on the first successful reading, so a correct emulator would end it immediately).
Why: a client enabling control-plane audit/api logging gets a successful update and no logs at all — the one externally visible effect of the setting is missing, and nothing in the API response indicates it. Open issue 20261023T113357Z-eks-control-plane-logging-has-no-effect-no.md carries the negative arm of this behaviour (a DISABLED log type producing no group) and does not carry this positive, CLI-driven arm.
Fix: the eks UpdateClusterConfig / CreateCluster handler's clusterLogging effect — it must provision /aws/eks/<cluster>/cluster and deliver the enabled log types into it.
Traces: ecr__eks__logs.cli-eks-update-logging-config-workflow.

## Evidence
ecr__eks__logs.cli-eks-update-logging-config-workflow: 'Check timed out after 60.0s'. The check runs `aws eks update-cluster-config --name <cluster> --logging '{"clusterLogging":[{"types":["api"],"enabled":true}]}'`, waits for the update id to be Successful, then polls for the log group (timeout_s=360) and its streams (timeout_s=300). Expected: the log group /aws/eks/<cluster>/cluster present and at least one log stream visible through the boto3 logs client; observed: neither ever appeared.

## Steps to Reproduce
Create a VPC/subnets and an EKS cluster role; eks CreateCluster with no clusterLogging; wait ACTIVE; eks UpdateClusterConfig(name=<cluster>, logging={'clusterLogging':[{'types':['api'],'enabled':True}]}); wait for the update to be Successful; logs DescribeLogGroups(logGroupNamePrefix='/aws/eks/<cluster>/cluster') — empty, and DescribeLogStreams on that name finds nothing.
