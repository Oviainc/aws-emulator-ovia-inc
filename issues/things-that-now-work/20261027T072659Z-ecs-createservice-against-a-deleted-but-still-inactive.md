Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.task-definition-register-deregister-lifecycle
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.task-definition-register-deregister-lifecycle|run#35|AssertionError|assert got == ("ClusterNotFoundException", expected_message), (
Issue-Commit: 3d2bed9d9c0136653e6daec548ca7ef547e87a29

# ECS CreateService against a deleted-but-still-INACTIVE cluster returns the "Cluster not found." message instead of "The referenced cluster was inactive." (1 cause)

> _Note: this issue was identified against the code base at commit `3d2bed9d9c0136653e6daec548ca7ef547e87a29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
THIS BUNDLE CONTAINS 1 CAUSE.

Cause 1 of 1 — CreateService on a cluster that has been deleted but is still discoverable as INACTIVE refuses with the correct error code but with the message AWS reserves for a cluster that is GONE, so a client cannot tell the two states apart.

AWS: Smithy: CreateService errors include ClusterNotFoundException (client, HTTP 400, httpError defaulted) and its documentation is "The specified cluster wasn't found. You can view your available clusters with ListClusters. Amazon ECS clusters are Region specific."; CreateService.cluster is a String naming the cluster short name or ARN. The model does not spell the message text, so the message requirement is recorded behavior only: ECS distinguishes two states behind the one error code. When the cluster is still visible to DescribeClusters with status INACTIVE (DeleteCluster has been accepted, but the cluster record has not yet aged out), CreateService is refused with code ClusterNotFoundException and message "The referenced cluster was inactive." — the whole message is fixed text for that error class, with no variable part. Only once the cluster is no longer returned by DescribeClusters at all (DescribeClusters answers a MISSING failure rather than a cluster) does the same code carry the message "Cluster not found.", also entirely fixed text. The selector is the cluster's own observable state, not the request: same request, two different messages depending on whether the named cluster still describes INACTIVE or has disappeared.

Emulator: the prior beats of the lifecycle are correct — DeleteCluster echoes the created clusterArn, and DescribeClusters then reports the cluster with status INACTIVE (so the emulator IS in the "still visible, INACTIVE" state). But CreateService{cluster=<that INACTIVE cluster>, serviceName=<new>, taskDefinition=<a live ACTIVE revision>, desiredCount=0, launchType=EC2} is refused with code ClusterNotFoundException and message "Cluster not found." — the gone-cluster text — where AWS answers "The referenced cluster was inactive.". HTTP status (400) and error code (ClusterNotFoundException) both match; only the message diverges. Hypothesis: the CreateService cluster lookup treats any non-ACTIVE cluster as absent and reuses a single not-found refusal, instead of branching on an existing-but-INACTIVE record.

Why: a client relying on AWS's behavior loses the only signal that separates "you deleted this cluster, re-create it (or wait for it to finish deprovisioning)" from "you named a cluster that never existed in this account/Region — check the name or the Region". Deployment tooling and operators regularly branch on exactly that distinction: retry/recreate on the inactive case, fail fast and report a typo or wrong-Region endpoint on the not-found case. Because the error code is identical in both states, the message is the discriminator; anything that matches, logs or surfaces it is misled into the wrong remediation, and an automated retry loop that keys off "inactive" never fires.

Fix: the cluster-resolution/refusal path in the CreateService handler (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs, plus any shared cluster-lookup helper it calls in the ECS state layer) must branch on the resolved cluster's stored status: when a cluster record exists with status INACTIVE, raise ClusterNotFoundException with the fixed text "The referenced cluster was inactive."; keep "Cluster not found." only for the case where no cluster record resolves at all. The same branch should be applied consistently wherever ECS resolves a cluster for a mutating call, so DeleteService/UpdateService/RunTask do not inherit the collapsed message.

Relation to open issues: 20261025T112916Z-ecs-refusal-messages-use-emulator-internal-wording-createservice.md is also about CreateService refusal text, but its two causes are the task-definition resolution message ("Unable to describe task definition." vs "TaskDefinition not found.") and CreateCluster's clusterName pattern message; neither touches cluster-state resolution, and its Issue-Trace-Ids/Issue-Keys do not carry this check. 20261024T081305Z-ecs-is-overly-permissive-ten-validations-authorizations-aws.md is about MISSING refusals (200 where AWS returns 400); here the refusal does happen with the right code and status. So this cause is net-new.

Traces: ecs.task-definition-register-deregister-lifecycle.

## Evidence
Check ecs.task-definition-register-deregister-lifecycle, verdict [mismatch].

Failure as replayed:
  File "check_ecs_task_definition_register_deregister_lifecycle.py", line 100, in run
    assert got == ("ClusterNotFoundException", expected_message)
  AssertionError: CreateService on the INACTIVE cluster: got ('ClusterNotFoundException', 'Cluster not found.')

The check computes its expectation from the cluster state it actually observed:
  expected_message = "The referenced cluster was inactive." if status == "INACTIVE" else "Cluster not found."
and the assertion text reports status == INACTIVE, so DescribeClusters had just returned the cluster with status INACTIVE (not a MISSING failure). Expected ('ClusterNotFoundException', 'The referenced cluster was inactive.'); got ('ClusterNotFoundException', 'Cluster not found.').

Message classification: both strings are entirely FIXED text for their respective states — there is no name, id, limit or timestamp interpolated into either, so nothing here is a per-run value. The variable thing is which of the two fixed messages is correct, and that is selected by the cluster's observable status.

Everything before this beat passed, which bounds the defect to the cluster-state branch: CreateCluster read back ACTIVE; RegisterTaskDefinition returned status ACTIVE / revision 1 with family and networkMode 'bridge' preserved on an independent DescribeTaskDefinition; DeregisterTaskDefinition returned INACTIVE and the revision stayed describable; DeleteTaskDefinitions drove it to DELETE_IN_PROGRESS/refusal and removed it from the family's ACTIVE listing; a second live revision registered; DeleteCluster echoed the original clusterArn and the cluster then described INACTIVE. HTTP status class (4xx) and error code were both graded and both matched.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; the failing check is ecs.task-definition-register-deregister-lifecycle.
2. By hand against the emulator's ECS endpoint:
   a. CreateCluster{clusterName=C}.
   b. RegisterTaskDefinition{family=F, requiresCompatibilities=["EC2"], networkMode="bridge", containerDefinitions=[{name:"main", image:"public.ecr.aws/amazonlinux/amazonlinux:2023", memory:128, command:["sleep","60"], essential:true}]} -> note the returned taskDefinitionArn T (status ACTIVE).
   c. DeleteCluster{cluster=C}.
   d. DescribeClusters{clusters=[C]} and confirm the cluster is still returned with status "INACTIVE" (this is the state that selects AWS's inactive wording; if it instead comes back as a MISSING failure, the not-found wording is the correct one and this reproduction does not apply).
   e. CreateService{cluster=C, serviceName=<new unique name>, taskDefinition=T, desiredCount=0, launchType="EC2"}.
3. Observe HTTP 400 with code ClusterNotFoundException and message "Cluster not found.", where AWS returns HTTP 400, code ClusterNotFoundException, message "The referenced cluster was inactive.".
4. Negative control for the other branch: repeat step 2e with a cluster name that was never created in this account/Region; AWS answers ClusterNotFoundException / "Cluster not found." there — the emulator must keep that message only for this case.
