Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsdep-traj-create-deployment-configuration-refusals
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsdep-traj-create-deployment-configuration-refusals ecs-ecscm-traj-private-dns-srv-reference-path ecs-ecscm-traj-registry-triplet-mistakes-bridge ecs-ntraf-service-load-balancer-block-mistakes ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop ecs-ecsnrt-traj-ec2-service-scaled-up-without-capacity-events ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair ecs-escc-traj-list-services-filter-and-paging-mistakes
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeServices|2xx->2xx|$.failures[*]
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*]
Issue-Key: ecs|DescribeServices|2xx->2xx|$
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].status
Issue-Key: ecs|ListServices|2xx->2xx|$.serviceArns[*]
Issue-Key: ecs|ListServices|2xx->2xx|$
Issue-Key: ecs|ListServices|2xx->2xx|$.serviceArns
Issue-Key: ecs|ListTasks|2xx->2xx|$.taskArns[*]
Issue-Key: ecs|DescribeServices|0->None|$
Issue-Key: ecs|DescribeServices|0->None|status
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].availabilityZoneRebalancing
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].currentServiceDeployment
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].events[*]
Issue-Key: ecs|DescribeServices|2xx->2xx|$.services[*].runStatus
Issue-Key: ecs|DescribeClusters|2xx->2xx|$
Issue-Key: ecs|DescribeClusters|2xx->2xx|$.clusters[*].attachmentsStatus
Issue-Key: ecs|ListServices|2xx->2xx|$.nextToken
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ECS listings and describes never reflect deletion or stabilization, and expose internal paging tokens

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Ten net-new causes, one fault: the read side of ECS does not track the lifecycle its own mutations started. Deleted services and stopped tasks keep appearing in listings and describes, states that AWS reaches never arrive (so every recorded poll exhausts its budget), and the paging token is internal state rather than an opaque handle. No open issue carries these replay keys.

Cause 1 of 10: DescribeServices for a deleted service. AWS: 200 with services = [] and one Failures entry {arn: <the service ARN from the request>, reason: 'MISSING'} — the 'MISSING' reason token is fixed, the arn is the request value. Smithy: DescribeServicesResponse.failures -> Failures. Emulator: 200 with the service still in services[0], status 'ACTIVE', and no failures entry. Why: a client polling for deletion never sees it, and a client re-creating the service sees a conflict that should not exist. Fix: DescribeServices's handler and the service state's delete path. Traces: ecs-ecsdep-traj-create-deployment-configuration-refusals#task#9.

Cause 2 of 10: DescribeServices polled for a deleted service to reach INACTIVE. AWS: the service remains describable with status 'INACTIVE' after DeleteService. Emulator: the service is absent entirely — services = [] with a failures entry reason 'MISSING' — so the poll for status in ['INACTIVE'] never converges and the step burns its whole 60s cap over 13 attempts. Why: this is the opposite error from cause 1 and shows delete is modelled as 'remove' in one path and 'nothing' in the other; either way no client can observe AWS's INACTIVE phase. Fix: the service state's delete transition (mark INACTIVE and retain, then reap) plus DescribeServices's lookup. Traces: ecs-ecscm-traj-private-dns-srv-reference-path#teardown#15.

Cause 3 of 10: ListServices after the services it names were deleted/failed to create. AWS: 200 with those ARNs absent from serviceArns. Emulator: 200 with three extra serviceArns entries for services that should not be listed. Why: a client enumerating a cluster acts on services that do not exist, and the count-based assertions in its cleanup never settle. Fix: ListServices's handler (filter on the state's lifecycle status). Traces: ecs-ecscm-traj-registry-triplet-mistakes-bridge#task#10 (cluster of 2 steps).

Cause 4 of 10: ListServices polled at teardown for an empty cluster. AWS: serviceArns becomes empty. Emulator: three ARNs remain for the full 60s cap (13 attempts). Why: teardown cannot be confirmed; this is also why DeleteCluster then refuses (filed separately). Fix: ListServices's handler and the delete path in service state. Traces: ecs-ecscm-traj-registry-triplet-mistakes-bridge#teardown#18.

Cause 5 of 10: ListServices after a service create was rejected. AWS: the rejected service does not appear. Emulator: its ARN is listed. Why: a 400-refused CreateService still leaves state behind, so a retry sees a duplicate. Fix: CreateService's handler — do not commit state on the refusal path. Traces: ecs-ntraf-service-load-balancer-block-mistakes#task#20.

Cause 6 of 10: ListTasks after the task stopped. AWS: the stopped task's ARN is absent from the default (RUNNING) listing. Emulator: the ARN is still returned. Why: a client enumerating live tasks operates on a stopped one. Fix: ListTasks's handler (default desiredStatus filtering against task state). Traces: ecs-ecsnrt-traj-fargate-unpullable-tag-stopped-record#task#12 (cluster of 3 steps).

Cause 7 of 10: DescribeServices polled while a circuit-breaker deployment should roll back. AWS: the service reaches the recorded deployment state within the fixture's time budget. Emulator: the state never arrives and the whole 600s fixture budget is spent on this step, with no response recorded. Why: any client waiting on deployment completion or rollback hangs indefinitely. Fix: the service-deployment state machine the service reads (rollout/stability progression), not DescribeServices's serializer. Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#15.

Cause 8 of 10: DescribeServices polled after scaling an EC2 service with no capacity. AWS: the service reaches the recorded body (currentServiceDeployment set, events carrying the placement-failure message, runStatus set). Emulator: the poll is capped at 60s without ever matching. Why: the client cannot tell 'still converging' from 'will never converge'. Fix: the service's deployment/placement progression in state. Traces: ecs-ecsnrt-traj-ec2-service-scaled-up-without-capacity-events#task#6.

Cause 9 of 10: DescribeClusters polled for attachmentsStatus 'UPDATE_COMPLETE' after a capacity provider is attached. AWS: the cluster reports attachmentsStatus reaching UPDATE_COMPLETE. Emulator: the cluster body carries capacityProviders but no attachmentsStatus member at all, so the poll never converges (13 attempts, 60s cap). Why: the documented way to wait for a capacity-provider attachment to finish is unusable. Fix: the cluster state's attachment tracking plus DescribeClusters's serializer. Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#setup#9.

Cause 10 of 10: ListServices nextToken is internal state. AWS: nextToken is an opaque token whose content is not a resource identifier (Smithy: ListServices is @paginated with inputToken/outputToken nextToken). Emulator: nextToken is the literal service ARN of the last item on the page. Why: the token leaks internal state and its format is observable; clients and proxies that log or route on tokens see resource ARNs, and any format change silently breaks resumption. Fix: ListServices's handler — emit the crate's opaque page-token codec instead of a raw ARN. Traces: ecs-escc-traj-list-services-filter-and-paging-mistakes#task#17.

## Evidence
Replay of trace set simparity-wire-aws-v1; all ten clusters have empty owned_by and empty stub_path, status 200 on both sides unless noted.
Cause 1 — $.failures[0] expected {arn: <service-arn>, reason: 'MISSING'}, got absent; $.services[0] present with status 'ACTIVE'.
Cause 2 — '$: poll capped at 60s'; actual body services = [] with failures[0].reason 'MISSING'; '$.services[0].status: poll: not in [INACTIVE] after 13 attempt(s)'.
Cause 3 — $.serviceArns[0..2] 'extra (expected None)'.
Cause 4 — '$.serviceArns: poll: 3 item(s), not 0 after 13 attempt(s)'.
Cause 5 — $.serviceArns[0] 'extra (expected None)'.
Cause 6 — $.taskArns[0] 'extra (expected None)'.
Cause 7 — 'status: expected , got None' plus '$: fixture budget 600s spent'; no response produced for the step.
Cause 8 — '$: poll capped at 60s'; divergent keys $.services[0].currentServiceDeployment, $.services[0].events[0], $.services[0].runStatus, $.services[0].availabilityZoneRebalancing.
Cause 9 — '$.clusters[0].attachmentsStatus: poll: not in [UPDATE_COMPLETE] after 13 attempt(s)'; actual cluster body has no attachmentsStatus member.
Cause 10 — $.nextToken expected an opaque token, got 'arn:aws:ecs:<region>:<account>:service/<cluster>/<service>'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Representative: fixture ecs-ecsdep-traj-create-deployment-configuration-refusals, step task#9 — DeleteService then DescribeServices; the deleted service is still returned ACTIVE instead of a failures entry with reason MISSING.
3. Replay each fixture#step in trace_ids; the poll-based causes are the steps whose detail reads 'poll capped at 60s' or 'fixture budget 600s spent'.
