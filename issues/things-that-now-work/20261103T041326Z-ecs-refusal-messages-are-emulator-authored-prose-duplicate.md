Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.concurrent-create-service-duplicate-name ecs.ecsnt-same-service-name-accepted-in-a-second-cluster ecs.escc-create-service-retry-with-the-same-client-token-returns-the-original ecs.escc-negative-desired-count-rejected ecs.escc-network-configuration-rejected-for-a-bridge-task-definition ecs.escc-replica-service-without-desired-count-rejected ecs.escc-describe-services-on-an-unknown-cluster-is-cluster-not-found
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.concurrent-create-service-duplicate-name|run#18|AssertionError|assert message in duplicate_messages, (
Issue-Key: ecs.ecsnt-same-service-name-accepted-in-a-second-cluster|run#9|AssertionError|assert got == want, f"the same name in the same cluster: expected {want!r}, got {got!r}"
Issue-Key: ecs.escc-create-service-retry-with-the-same-client-token-returns-the-original|run#17|AssertionError|assert got == want, f"the same request under a new token: expected {want!r}, got {got!r}"
Issue-Key: ecs.escc-negative-desired-count-rejected|run#10|AssertionError|assert msg == want, f"update desiredCount -1 message: expected {want!r}, got {msg!r}"
Issue-Key: ecs.escc-network-configuration-rejected-for-a-bridge-task-definition|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ecs.escc-replica-service-without-desired-count-rejected|run#6|AssertionError|assert (code, message) == ("InvalidParameterException", "DesiredCount is missing"), (
Issue-Key: ecs|DescribeServices|ClusterNotFoundException
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# ECS refusal messages are emulator-authored prose: duplicate service name, negative desiredCount, networkConfiguration-on-bridge, REPLICA desiredCount and unknown-cluster DescribeServices all diverge from AWS's wording

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
The emulator refuses these requests — the status and the error code are right — but it substitutes its own prose for AWS's message, and in one case the wrong message class entirely. An error message is part of the contract: clients and CLI wrappers match on it, and the emulator's texts leak internal phrasing and request values AWS does not include. Five causes, numbered below — all five must be closed before this issue is done.

Cause 1 of 5: the duplicate-service-name refusal is reworded.
AWS: Smithy: CreateService.serviceName (required String) and .clientToken (String); error InvalidParameterException (httpError 400). Creating a service whose name is already ACTIVE in the cluster — whether raced concurrently, retried under a NEW clientToken, or re-issued after the name was taken — is refused with HTTP 400 InvalidParameterException and the message `Creation of service was not idempotent.` That text is entirely fixed: it names neither the service nor the cluster. The same name IS accepted in a second cluster, so the refusal is scoped to the cluster.
Emulator: answers InvalidParameterException with `service "<serviceName>" already exists in cluster "<clusterName>"` — emulator-authored prose that interpolates the service and cluster names AWS never includes.
Why: a client or test that matches AWS's fixed idempotency text does not recognise the refusal and treats it as an unknown error; conversely anything matching the emulator's text breaks against AWS. The wording also misdescribes the condition: AWS frames it as a failed idempotent creation, not as a name collision.
Fix: the CreateService handler's duplicate-name refusal message, services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs.
Traces: ecs.concurrent-create-service-duplicate-name, ecs.ecsnt-same-service-name-accepted-in-a-second-cluster, ecs.escc-create-service-retry-with-the-same-client-token-returns-the-original.

Cause 2 of 5: the negative-desiredCount refusal on UpdateService is reworded.
AWS: Smithy: UpdateService.desiredCount -> BoxedInteger with no @range, so the lower bound is service-enforced; error InvalidParameterException 400. desiredCount=-1 is refused with HTTP 400 InvalidParameterException and the fixed message `Task count can not be lesser than 0` — AWS words it in terms of task count and interpolates nothing.
Emulator: `DesiredCount can not be lesser than 0` — the same sentence with the member name swapped in.
Why: the message is the only machine-readable discriminator between the several InvalidParameterException causes UpdateService can return, so matching clients mis-route the error.
Fix: the UpdateService handler's desiredCount validation message, services/.../src/ops/update_service.rs.
Traces: ecs.escc-negative-desired-count-rejected.

Cause 3 of 5: the networkConfiguration-on-a-bridge-task-definition refusal is reworded.
AWS: Smithy: CreateService.networkConfiguration -> NetworkConfiguration and the task definition's NetworkMode enum; error InvalidParameterException 400. Supplying a networkConfiguration for a service whose task definition uses bridge networking is refused with HTTP 400 InvalidParameterException and the fixed message `Network Configuration is not valid for the given networkMode of this task definition.` — the sentence is fixed and does NOT quote the networkMode value.
Emulator: `Network Configuration must not be provided when networkMode 'bridge' is specified.` — different sentence, and it interpolates the networkMode value.
Why: same contract break — the text is what distinguishes this refusal from the other CreateService InvalidParameterExceptions.
Fix: the CreateService handler's network-mode validation message, same handler as Cause 1.
Traces: ecs.escc-network-configuration-rejected-for-a-bridge-task-definition.

Cause 4 of 5: the missing-desiredCount refusal for a REPLICA service is reworded.
AWS: Smithy: CreateService.schedulingStrategy -> SchedulingStrategy enum (REPLICA|DAEMON) and .desiredCount -> BoxedInteger, optional in the model; a REPLICA service created without desiredCount is refused with HTTP 400 InvalidParameterException and the fixed message `DesiredCount is missing`.
Emulator: `desiredCount is required when schedulingStrategy is REPLICA` — emulator-authored prose.
Why: the message differs in both wording and shape, so client-side matching and user-facing error surfaces diverge from AWS.
Fix: the CreateService handler's required-field validation message, same handler as Cause 1.
Traces: ecs.escc-replica-service-without-desired-count-rejected.

Cause 5 of 5: DescribeServices against a cluster name AWS never issued reports the wrong ClusterNotFoundException message, conflating never-existed with INACTIVE.
AWS: Smithy: DescribeServices.cluster (String), errors ClusterNotFoundException (httpError 400) and InvalidParameterException 400. A cluster name that was never created is refused with HTTP 400 ClusterNotFoundException and the fixed message `Cluster not found.` A cluster that WAS created and then deleted is a known cluster still readable as INACTIVE: DescribeServices on it answers normally with a per-item MISSING failure, not an exception.
Emulator: raises ClusterNotFoundException with `The referenced cluster was inactive.` for a never-issued name — the wrong message, and one that asserts a state (INACTIVE) the cluster was never in.
Why: the two conditions are deliberately distinguishable in AWS; a client diagnosing a deploy cannot tell "you typed a cluster that does not exist" from "your cluster was deleted", which are different remediations. The check fails as an uncaught ClusterNotFoundException because the message assertion is reached through a path AWS does not take.
Fix: the DescribeServices handler's cluster-resolution error construction (and the shared ECS cluster lookup it uses), services/.../src/ops/describe_services.rs.
Traces: ecs.escc-describe-services-on-an-unknown-cluster-is-cluster-not-found.

## Evidence
Replay output:
- ecs.concurrent-create-service-duplicate-name [mismatch] — AssertionError: expected the duplicate-name refusal message, got 'service "<serviceName>" already exists in cluster "<clusterName>"'. Expected message: 'Creation of service was not idempotent.'
- ecs.ecsnt-same-service-name-accepted-in-a-second-cluster [mismatch] — AssertionError: the same name in the same cluster: expected ('InvalidParameterException', 'Creation of service was not idempotent.'), got ('InvalidParameterException', 'service "<serviceName>" already exists in cluster "<clusterName>"').
- ecs.escc-create-service-retry-with-the-same-client-token-returns-the-original [mismatch] — AssertionError: the same request under a new token: expected ('InvalidParameterException', 'Creation of service was not idempotent.'), got ('InvalidParameterException', 'service "<serviceName>" already exists in cluster "<clusterName>"').
- ecs.escc-negative-desired-count-rejected [mismatch] — AssertionError: update desiredCount -1 message: expected 'Task count can not be lesser than 0', got 'DesiredCount can not be lesser than 0'.
- ecs.escc-network-configuration-rejected-for-a-bridge-task-definition [mismatch] — AssertionError: expected message 'Network Configuration is not valid for the given networkMode of this task definition.', got "Network Configuration must not be provided when networkMode 'bridge' is specified."
- ecs.escc-replica-service-without-desired-count-rejected [mismatch] — AssertionError: got ('InvalidParameterException', 'desiredCount is required when schedulingStrategy is REPLICA'); expected ('InvalidParameterException', 'DesiredCount is missing').
- ecs.escc-describe-services-on-an-unknown-cluster-is-cluster-not-found [error] — botocore.errorfactory.ClusterNotFoundException: An error occurred (ClusterNotFoundException) when calling the DescribeServices operation: The referenced cluster was inactive. Expected ('ClusterNotFoundException', 'Cluster not found.') for a never-issued cluster name; the deleted-cluster leg expects a normal 200 response carrying a MISSING failure instead of any exception.
In every case the HTTP status (400) and the error code are correct; only the message text diverges (Cause 5 additionally takes the wrong message class for the never-existed cluster).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the seven checks listed in trace_ids (all under the `ecs` service heading).
3. Cause 1: CreateCluster, CreateService <name>, then CreateService with the same <name> in that cluster (concurrently, and again with a fresh clientToken) and read Error.Message. 4. Cause 2: UpdateService with desiredCount=-1. 5. Cause 3: CreateService with a networkConfiguration against a task definition whose networkMode is bridge. 6. Cause 4: CreateService with schedulingStrategy=REPLICA and no desiredCount. 7. Cause 5: DescribeServices(cluster=<name never created>, services=["anything"]) and compare against DescribeServices on a cluster that was created and then deleted.
