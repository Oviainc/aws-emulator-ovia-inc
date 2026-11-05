Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr__iam.e2e-create-emr-cluster-multi-master-lifecycle
Issue-Key: ec2__emr__iam.e2e-create-emr-cluster-multi-master-lifecycle|phase#9|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# emr TerminateJobFlows ignores termination protection: a protected multi-master cluster is shut down instead of refused ValidationException

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TerminateJobFlows

## Description
One net-new root cause.

Cause 1 of 1: TerminateJobFlows on a cluster with TerminationProtected=true is accepted instead of refused.

AWS: a cluster whose Instances.TerminationProtected is true (which EMR sets implicitly for a multi-primary cluster) cannot be shut down by a bare TerminateJobFlows. The call is refused with error code ValidationException and the message "Could not shut down one or more job flows since they are termination protected." (fixed text, no request-derived parts), and every named cluster stays in its current state; the documented repair is SetTerminationProtection(TerminationProtected=false) then TerminateJobFlows. Smithy (services/service-com-amazonaws-emr-elasticmapreduce): TerminateJobFlows models InternalServerError only, so the ValidationException code and that sentence are recorded behavior only.

Emulator: the bare terminate of the protected multi-master cluster raises nothing — the protection flag has no effect on the terminate path.

Why: termination protection is the guard that keeps a long-running cluster alive; against the emulator a stray or scripted TerminateJobFlows destroys a protected cluster, and the branch clients rely on (catch ValidationException, clear protection, retry) never runs.

Fix: the TerminateJobFlows handler in the emr service must read each named cluster's TerminationProtected state and refuse the whole call with the code and message above when any of them is protected, leaving every named cluster's state unchanged.

Traces: ec2__emr__iam.e2e-create-emr-cluster-multi-master-lifecycle (phase 4/6 post_invoke). Open issues 20261022T235108Z-ec2-neighbouring-service-fidelity-gaps-reached-through-ec2-led.md and 20261102T094623Z-ec2-emr-emr-adds-no-ingress-rules-to.md cover other emr causes; the suite does not mark this check as owned by either.

## Evidence
Expected (AWS, from the recorded refusal): terminate_job_flows ['j-...'] raises ValidationException: "Could not shut down one or more job flows since they are termination protected."
Actual (emulator): no error at all. AssertionError: [phase 4/6 post_invoke] the bare terminate of a protected multi-master cluster should be refused with ["terminate_job_flows ['j-0000000000002']: ValidationException: Could not shut down one or more job flows since they are termination protected."], got [].

## Steps to Reproduce
1. Deploy the emr-private-network estate (VPC/subnet plus EMR service and instance-profile roles).
2. RunJobFlow a 3-primary (multi-master) cluster and wait until it reaches WAITING.
3. Call emr TerminateJobFlows(JobFlowIds=[<cluster id>]) without first calling SetTerminationProtection.
4. Expected: ValidationException "Could not shut down one or more job flows since they are termination protected." and the cluster still WAITING. Observed: the call succeeds, no error raised.
