Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.cluster-reaches-waiting-with-running-instances emr.emr-run-step-spark emr.step-writes-real-output-to-s3 emr.termination-protection-blocks-terminate-job-flows emr.bootstrap-action-failure-terminates-with-errors emr.action-on-failure-continue-then-terminate-cluster emr.put-autoscaling-policy-malformed-rule-rejected emr.remove-autoscaling-policy-without-existing-policy-rejected emr.concurrent-put-auto-scaling-policy-last-writer-wins emr.set-unhealthy-node-replacement-updates-cluster-configuration ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags ec2__emr.core-fleet-memory-derivation-strands-executors-and-driver ec2__emr__iam.cluster-in-nonexistent-subnet-rejected ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails emr__glue.on-cluster-add-partition-lands-in-the-glue-catalog emr__glue__s3.emr-cluster-logs-uploaded-to-s3-log-uri-positive emr__glue__s3.emr-cluster-step-writes-output-to-s3-positive emr.get-session-invalid-session-id-error-semantics emr.op-cluster-id-mutations-reject-unknown-cluster emr.op-describe-job-flows-is-deprecated emr.op-create-studio-rejects-unresolvable-network emr.op-studio-unknown-id-operations-fail emr.op-session-mapping-unknown-studio-fails emr.op-notebook-execution-ops-reject-unknown-execution emr.set-unhealthy-node-replacement-error-not-swallowed
Issue-Key: emr|CreateInstanceProfile|NotImplemented
Issue-Key: emr|GetInstanceProfile|NotImplemented
Issue-Key: ec2__emr|CreateInstanceProfile|NotImplemented
Issue-Key: ec2__emr__iam|CreateInstanceProfile|NotImplemented
Issue-Key: emr__events__sqs|CreateInstanceProfile|NotImplemented
Issue-Key: emr__glue|CreateInstanceProfile|NotImplemented
Issue-Key: emr__glue__s3|CreateInstanceProfile|NotImplemented
Issue-Key: emr.get-session-invalid-session-id-error-semantics|run#3|AssertionError|assert control_code == "InvalidRequestException", (
Issue-Key: emr.op-cluster-id-mutations-reject-unknown-cluster|_assert_codes#3|AssertionError|assert not wrong, f"expected {expected} for every op, got mismatches: {wrong}"
Issue-Key: emr.op-describe-job-flows-is-deprecated|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: emr.op-create-studio-rejects-unresolvable-network|run#4|AssertionError|assert code in ("ValidationException", "InvalidRequestException"), (
Issue-Key: emr.op-studio-unknown-id-operations-fail|run#4|AssertionError|assert code == "InvalidRequestException", f"{op}: expected InvalidRequestException, got {code}"
Issue-Key: emr.op-session-mapping-unknown-studio-fails|run#6|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: emr.op-notebook-execution-ops-reject-unknown-execution|_assert_codes#3|AssertionError|assert not wrong, f"expected {expected} for every op, got mismatches: {wrong}"
Issue-Key: emr.set-unhealthy-node-replacement-error-not-swallowed|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# EMR control plane: the IAM instance-profile prerequisite and 7 EMR operation families answer 501 NotImplemented, so ~130 EMR checks never reach their assertion (7 causes)

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInstanceProfile

## Description
Seven distinct root causes, every one of them an operation that answers HTTP 501 NotImplemented ("operation X is scaffolded but not implemented in this emulator") where AWS answers a modeled 200 or a modeled 400. Cause 1 is a cross-service (IAM) prerequisite that aborts the overwhelming majority of the EMR behavioral suite before any EMR behavior is exercised; causes 2-7 are EMR's own operations. None of these are marked ALREADY FILED or OWNED by the replay report. Note for triage: the open issue 20261022T151732Z-elasticbeanstalk-elastic-beanstalk-three-modeled-operations-are-absent-from carries IAM CreateInstanceProfile as its own Cause 4 for the Elastic Beanstalk suite; the handler is the same file, but the EMR checks below are not on that issue's replay list, so they are recorded here.

Cause 1 of 7: the IAM instance-profile prerequisite (CreateInstanceProfile, GetInstanceProfile) answers 501.
AWS: IAM CreateInstanceProfile returns 200 with the created InstanceProfile; GetInstanceProfile returns 200 with it (NoSuchEntity 404 only when absent). EMR's own API never mints the EC2 instance profile or the service role for the caller, so a cluster-launching client must create them itself and pass the profile name as RunJobFlow.JobFlowRole (Smithy: RunJobFlow.JobFlowRole, XmlString len 0..10280). Recorded behavior in these checks: the IAM create/attach sequence succeeds and RunJobFlow then accepts the profile.
Emulator: both answer HTTP 501, surfacing as botocore ClientError code 'NotImplemented'. Checks that wrap it report "cannot provision the EMR service role / EC2 instance profile".
Why: no client can provision the instance profile EMR requires, so every cluster-launching workflow is dead. This single cause aborts ~130 of the 252 failing checks in this suite (every lifecycle, step, fleet, scaling, session, studio and cross-service EMR check), masking all the behavior they were written to measure.
Fix: the scaffold stubs services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_instance_profile.rs and .../src/ops/get_instance_profile.rs (plus whatever instance-profile state add_role_to_instance_profile.rs shares).
Traces: emr.cluster-reaches-waiting-with-running-instances, emr.emr-run-step-spark, emr.step-writes-real-output-to-s3, emr.termination-protection-blocks-terminate-job-flows, emr.bootstrap-action-failure-terminates-with-errors, emr.action-on-failure-continue-then-terminate-cluster, emr.put-autoscaling-policy-malformed-rule-rejected, emr.remove-autoscaling-policy-without-existing-policy-rejected, emr.concurrent-put-auto-scaling-policy-last-writer-wins (GetInstanceProfile), emr.set-unhealthy-node-replacement-updates-cluster-configuration (GetInstanceProfile), ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags, ec2__emr.core-fleet-memory-derivation-strands-executors-and-driver, ec2__emr__iam.cluster-in-nonexistent-subnet-rejected, ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group, emr__events__sqs.emr-step-status-change-delivers-failed-and-withholds-it-from-the-completed-rule, emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails, emr__glue.on-cluster-add-partition-lands-in-the-glue-catalog, emr__glue__s3.emr-cluster-logs-uploaded-to-s3-log-uri-positive, emr__glue__s3.emr-cluster-step-writes-output-to-s3-positive.

Cause 2 of 7: GetSession answers 501 instead of resolving the session id.
AWS: Smithy com.amazonaws.emr#GetSession takes ClusterId and SessionId (both required, len 0..256) and models exactly two errors — InvalidRequestException (HTTP 400, "there is something wrong with user input") and InternalServerException (500). A well-formed but nonexistent SessionId reaches lookup and is refused 400 InvalidRequestException; a malformed one is refused up front.
Emulator: 501 NotImplemented for every GetSession call, so neither the negative nor the positive-control path is observable.
Why: a client cannot distinguish a malformed session id from an unknown one from a server fault; all three collapse into an unmodeled 501 that no SDK error model accounts for.
Fix: the scaffold stub services/service-com-amazonaws-emr-elasticmapreduce/src/ops/get_session.rs (its '//! SCAFFOLDED, NOT IMPLEMENTED' marker and Scaffolded::not_implemented("GetSession") body are present in the checkout).
Traces: emr.get-session-invalid-session-id-error-semantics.

Cause 3 of 7: the cluster-id mutation family (ModifyCluster, CancelSteps, AddInstanceFleet, GetClusterSessionCredentials) answers 501 instead of refusing an unknown cluster.
AWS: Smithy models each of these with ClusterId required and InvalidRequestException (HTTP 400) as the client error — ModifyCluster {ClusterId required, StepConcurrencyLevel}, CancelSteps {ClusterId, StepIds required}, AddInstanceFleet {ClusterId, InstanceFleet required}, GetClusterSessionCredentials {ClusterId required}. A syntactically valid but nonexistent j- id is refused 400 InvalidRequestException by all four.
Emulator: all four answer 501 NotImplemented.
Why: an unknown-cluster mutation is indistinguishable from a server defect, and no caller can resize a fleet, cancel steps, change step concurrency or obtain session credentials at all.
Fix: the scaffold stubs services/service-com-amazonaws-emr-elasticmapreduce/src/ops/{modify_cluster,cancel_steps,add_instance_fleet,get_cluster_session_credentials}.rs.
Traces: emr.op-cluster-id-mutations-reject-unknown-cluster.

Cause 4 of 7: DescribeJobFlows answers 501 instead of the retired-operation refusal.
AWS: Smithy com.amazonaws.emr#DescribeJobFlows is marked deprecated; real EMR refuses it with a 400 ValidationException regardless of arguments rather than serving job-flow data.
Emulator: 501 NotImplemented.
Why: a client gets an unmodeled 501 (and an SDK-level NotImplemented) where AWS gives a 400 client error, so retirement-handling code paths cannot be exercised; equally, an emulator that later serves data here would be overly permissive.
Fix: the handler services/service-com-amazonaws-emr-elasticmapreduce/src/ops/describe_job_flows.rs — it should return the 400 ValidationException refusal, not 501.
Traces: emr.op-describe-job-flows-is-deprecated.

Cause 5 of 7: the EMR Studio family (CreateStudio, DescribeStudio, CreateStudioSessionMapping) answers 501.
AWS: Smithy CreateStudio (Name/AuthMode/VpcId/SubnetIds/ServiceRole/WorkspaceSecurityGroupId/EngineSecurityGroupId/DefaultS3Location required) models InvalidRequestException (400); DescribeStudio {StudioId required} and CreateStudioSessionMapping {StudioId, IdentityType, SessionPolicyArn required} both model InvalidRequestException (400). An unresolvable VPC/subnet/role on CreateStudio, and an unknown StudioId on DescribeStudio or CreateStudioSessionMapping, are all 400 InvalidRequestException.
Emulator: all three answer 501 NotImplemented.
Why: EMR Studio is entirely unusable and every unknown-studio refusal is reported as a server-side 'not implemented' rather than a client error.
Fix: the scaffold stubs services/service-com-amazonaws-emr-elasticmapreduce/src/ops/{create_studio,describe_studio,create_studio_session_mapping}.rs.
Traces: emr.op-create-studio-rejects-unresolvable-network, emr.op-studio-unknown-id-operations-fail, emr.op-session-mapping-unknown-studio-fails.

Cause 6 of 7: DescribeNotebookExecution and StopNotebookExecution answer 501.
AWS: Smithy both take NotebookExecutionId (required, len 0..256) and model InvalidRequestException (HTTP 400) for an unknown execution id.
Emulator: both answer 501 NotImplemented.
Why: notebook-execution state cannot be read or stopped, and the unknown-id refusal is unmodeled.
Fix: the scaffold stubs services/service-com-amazonaws-emr-elasticmapreduce/src/ops/{describe_notebook_execution,stop_notebook_execution}.rs.
Traces: emr.op-notebook-execution-ops-reject-unknown-execution.

Cause 7 of 7: SetUnhealthyNodeReplacement answers 501 instead of validating the JobFlowIds.
AWS: Smithy SetUnhealthyNodeReplacement {JobFlowIds required, UnhealthyNodeReplacement required} returns an empty 200 on success. Recorded behavior: a syntactically well-formed but nonexistent JobFlowId is refused with a 400 ValidationException — EMR resolves the id before touching cluster state, and must neither succeed silently nor fabricate a cluster.
Emulator: 501 NotImplemented.
Why: a client cannot tell a rejected mutation from an unimplemented one, and the whole unhealthy-node-replacement setting is unreachable.
Fix: the scaffold stub services/service-com-amazonaws-emr-elasticmapreduce/src/ops/set_unhealthy_node_replacement.rs.
Traces: emr.set-unhealthy-node-replacement-error-not-swallowed.

## Evidence
Cause 1 — expected HTTP 200 CreateInstanceProfileResult / GetInstanceProfileResult; actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateInstanceProfile operation: operation CreateInstanceProfile is scaffolded but not implemented in this emulator (141 of the 169 enumerated failures reach this or a sibling stub; the same text for GetInstanceProfile).
Cause 2 — expected (InvalidRequestException, 400) for a well-formed-but-unknown SessionId; actual AssertionError: "positive control failed: well-formed-but-nonexistent SessionId must reach lookup and fail with InvalidRequestException, got CODE='NotImplemented'".
Cause 3 — expected InvalidRequestException for every op; actual AssertionError: "expected InvalidRequestException for every op, got mismatches: {'ModifyCluster': 'NotImplemented', 'CancelSteps': 'NotImplemented', 'AddInstanceFleet': 'NotImplemented', 'GetClusterSessionCredentials': 'NotImplemented'}".
Cause 4 — expected ValidationException (400); actual AssertionError: "expected ValidationException, got NotImplemented".
Cause 5 — actual AssertionError: "expected CreateStudio to reject a bad network/role request, got NotImplemented"; "DescribeStudio: expected InvalidRequestException, got NotImplemented"; "expected CreateStudioSessionMapping on an unknown studio to fail, got NotImplemented".
Cause 6 — actual AssertionError: "expected InvalidRequestException for every op, got mismatches: {'DescribeNotebookExecution': 'NotImplemented', 'StopNotebookExecution': 'NotImplemented'}".
Cause 7 — expected ValidationException; actual AssertionError: "expected ValidationException, got 'NotImplemented'".
Corroborating checkout state: services/service-com-amazonaws-emr-elasticmapreduce/src/ops/get_session.rs lines 4 and 155 carry the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and return Err(crate::scaffolded::Scaffolded::not_implemented("GetSession")).

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. iam = client('iam'); iam.create_instance_profile(InstanceProfileName='p') -> expect 200; observe 501 NotImplemented. Same for iam.get_instance_profile.
3. emr = client('emr'); emr.get_session(ClusterId='j-ZZZZZZZZZZZZZ', SessionId='s-nope') -> expect 400 InvalidRequestException; observe 501 NotImplemented.
4. emr.modify_cluster(ClusterId='j-ZZZZZZZZZZZZZ', StepConcurrencyLevel=2), emr.cancel_steps(ClusterId='j-ZZZZZZZZZZZZZ', StepIds=['s-1']), emr.add_instance_fleet(...), emr.get_cluster_session_credentials(ClusterId='j-ZZZZZZZZZZZZZ') -> expect 400 InvalidRequestException each; observe 501.
5. emr.describe_job_flows() -> expect 400 ValidationException; observe 501.
6. emr.create_studio(...), emr.describe_studio(StudioId='es-NOPE'), emr.create_studio_session_mapping(...) -> expect 400 InvalidRequestException; observe 501.
7. emr.describe_notebook_execution(NotebookExecutionId='ex-NOPE'), emr.stop_notebook_execution(NotebookExecutionId='ex-NOPE') -> expect 400 InvalidRequestException; observe 501.
8. emr.set_unhealthy_node_replacement(JobFlowIds=['j-ZZZZZZZZZZZZZ'], UnhealthyNodeReplacement=True) -> expect 400 ValidationException; observe 501.
9. Re-replay the check ids in trace_ids.
