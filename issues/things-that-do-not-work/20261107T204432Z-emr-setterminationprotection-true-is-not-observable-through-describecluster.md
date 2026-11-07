Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.termination-protection-blocks-terminate-job-flows
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.termination-protection-blocks-terminate-job-flows|_held#12|Timeout|time.sleep(interval_s)
Issue-Commit: 959ef406fada22402fcbc8404625fdf48db790f8

# emr: SetTerminationProtection(true) is not observable through DescribeCluster and does not guard TerminateJobFlows — the protected-cluster check never progresses and times out

> _Note: this issue was identified against the code base at commit `959ef406fada22402fcbc8404625fdf48db790f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetTerminationProtection

## Description
ONE net-new root cause (Cause 1 of 1).

Cause 1 of 1: on a running (WAITING) single-primary cluster, SetTerminationProtection(JobFlowIds=[id], TerminationProtected=true) does not become observable state, so the whole termination-protection path — the round-trip read, the refused TerminateJobFlows, and the control terminate after protection is cleared — never progresses. Failing check: emr.termination-protection-blocks-terminate-job-flows.

AWS: SetTerminationProtection is a mutation on live cluster state. Smithy (services/service-com-amazonaws-emr-elasticmapreduce): SetTerminationProtection takes required JobFlowIds (XmlStringList) and required TerminationProtected (Boolean), returns an empty body, and models only InternalServerError; DescribeCluster returns the Cluster structure whose TerminationProtected member is documented as "Indicates whether Amazon EMR will lock the cluster to prevent the Amazon EC2 instances from being terminated by an API call". After a successful SetTerminationProtection(TerminationProtected=true), DescribeCluster(ClusterId=<id>).Cluster.TerminationProtected is true (the value is the one just set, taken from the request, not a per-run constant), and a bare TerminateJobFlows naming that cluster is then refused — status 400, error code ValidationException, message "Could not shut down one or more job flows since they are termination protected." (entirely fixed text; nothing is interpolated from the request) — with the cluster left in its current state, no USER_REQUEST StateChangeReason recorded and TerminationProtected still true. Clearing protection with SetTerminationProtection(TerminationProtected=false) makes the identical TerminateJobFlows succeed and the cluster reach TERMINATED with StateChangeReason.Code = USER_REQUEST and StateChangeReason.Message = "Terminated by user request" (both fixed text). The ValidationException code and both sentences are recorded behavior only: the Smithy model lists only InternalServerError for both SetTerminationProtection and TerminateJobFlows.

Emulator: the check gets no further than the protection round-trip. It launches a 1x m5.xlarge emr-6.15.0 cluster, polls it to WAITING, calls SetTerminationProtection(TerminationProtected=true), and then polls DescribeCluster(...).Cluster.TerminationProtected for the value true (120 s budget, 5 s interval); the check is killed by the suite's 180 s budget with "Error: Check timed out after 180.0s", i.e. it is still inside that bounded wait and never reaches the TerminateJobFlows refusal, the effect-absent hold or the control terminate. Hypothesis: DescribeCluster does not report the flag SetTerminationProtection stored (it serves TerminationProtected=false, or omits the member), so the poll can never be satisfied and burns the whole remaining budget. Two open issues describe the same flag being inert from the other direction but do NOT carry this check's replay key and do not cover it: 20261102T083234Z-elasticmapreduce-emr-cluster-and-instance-group-reads-omit-modeled.md (DescribeCluster loses the TerminationProtected value supplied at RunJobFlow time) and 20261105T083215Z-ec2-emr-iam-emr-terminatejobflows-ignores-termination-protection-a-protected-multi.md (TerminateJobFlows shuts down an implicitly-protected multi-primary cluster instead of refusing). Neither covers SetTerminationProtection's own post-creation mutation on a single-primary cluster, which is what stalls here.

Why: termination protection is the only guard that keeps a long-running cluster alive. Against the emulator a client that turns protection on cannot confirm it (the read it polls never flips), so every guard built on that read either loops until its own timeout or proceeds as if the cluster were unprotected; a stray or scripted TerminateJobFlows then destroys the cluster instead of returning ValidationException, and the documented repair branch (catch the refusal, clear protection, retry) is never exercised.

Fix: the SetTerminationProtection handler and the cluster state it writes in the emr service — the flag must be persisted per cluster and served back by DescribeCluster's Cluster.TerminationProtected — plus the TerminateJobFlows handler, which must read that stored flag and refuse the whole call with the 400/ValidationException and the sentence above, leaving every named cluster's state and StateChangeReason untouched, while an unprotected terminate records Code USER_REQUEST and Message "Terminated by user request".

Traces: emr.termination-protection-blocks-terminate-job-flows (the only failing check in this suite not already owned by an open issue or blocked by an unimplemented stub).

## Evidence
Suite report, service heading ### emr:

  #### [mismatch] emr.termination-protection-blocks-terminate-job-flows
  Error: Check timed out after 180.0s

The check's sequence: RunJobFlow (1x m5.xlarge, emr-6.15.0, KeepJobFlowAliveWhenNoSteps=true) -> poll to WAITING -> SetTerminationProtection(JobFlowIds=[<cluster id>], TerminationProtected=True) -> poll describe_cluster(ClusterId=<id>)['Cluster']['TerminationProtected'] until it is True (timeout_s=120, interval_s=5) -> bare terminate_job_flows, which must raise ValidationException/InvalidRequestException with message exactly "Could not shut down one or more job flows since they are termination protected." -> hold the cluster at WAITING for 180 s -> clear protection -> identical terminate succeeds to TERMINATED with StateChangeReason {Code: USER_REQUEST, Message: "Terminated by user request"}.

Expected (AWS): the protection poll is satisfied on the first read after SetTerminationProtection returns, and the check proceeds through the refusal and the control terminate.
Actual (emulator): the check is still in the bounded protection poll when the 180 s check budget expires — no assertion about the refusal is ever reached, so the protected-cluster path is unreachable end to end. (The 180.0 s figure is the suite's per-check budget, not a requirement; the divergence is that the protection round-trip never completes.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Create the EMR service role and EC2 instance profile, and a VPC/subnet (the check's private network).
3. RunJobFlow a one-instance-group cluster (MASTER, 1x m5.xlarge, ON_DEMAND, ReleaseLabel emr-6.15.0, KeepJobFlowAliveWhenNoSteps=true, Instances.TerminationProtected=false) and poll DescribeCluster until the cluster is WAITING.
4. Call emr SetTerminationProtection(JobFlowIds=[<cluster id>], TerminationProtected=true).
5. Call emr DescribeCluster(ClusterId=<cluster id>) and read Cluster.TerminationProtected. Expected: true. Observed: the value never becomes true (the check's 120 s poll on it does not complete, and the check is killed at the 180 s budget).
6. Call emr TerminateJobFlows(JobFlowIds=[<cluster id>]) with protection on. Expected: HTTP 400, ValidationException, message "Could not shut down one or more job flows since they are termination protected.", the cluster still WAITING and TerminationProtected still true.
7. Call SetTerminationProtection(TerminationProtected=false) then the identical TerminateJobFlows. Expected: the cluster reaches TERMINATED with StateChangeReason.Code USER_REQUEST and StateChangeReason.Message "Terminated by user request".
