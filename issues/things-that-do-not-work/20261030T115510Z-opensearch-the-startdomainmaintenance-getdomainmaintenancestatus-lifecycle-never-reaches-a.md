Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.domain-maintenance-lifecycle
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.domain-maintenance-lifecycle|_wait_maintenance_terminal#10|Timeout|time.sleep(interval_s)
Issue-Commit: 8606934b49a04db2d5998d802f8da740c15f20ac

# opensearch: the StartDomainMaintenance -> GetDomainMaintenanceStatus lifecycle never reaches a terminal status within the replay window (1 cause)

> _Note: this issue was identified against the code base at commit `8606934b49a04db2d5998d802f8da740c15f20ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomainMaintenanceStatus

## Description
This issue carries 1 root cause, in the opensearch service. It must be closed before submitting.

Cause 1 of 1: a node maintenance action started on a domain cannot be driven to a terminal status — the StartDomainMaintenance -> GetDomainMaintenanceStatus sequence produces no terminal Status at all inside the window the replay gives it, so the whole maintenance lifecycle is unobservable end to end.

AWS: Smithy: StartDomainMaintenance POST /2021-01-01/opensearch/domain/{DomainName}/domainMaintenance (DomainName @httpLabel required, Action MaintenanceType required, NodeId @length 10..40) answers MaintenanceId (RequestId @length 1..100); GetDomainMaintenanceStatus GET /2021-01-01/opensearch/domain/{DomainName}/domainMaintenance?maintenanceId=<id> answers Status (MaintenanceStatus enum: PENDING|IN_PROGRESS|COMPLETED|FAILED|TIMED_OUT), StatusMessage, NodeId, Action, CreatedAt, UpdatedAt; ListDomainMaintenances lists the same DomainMaintenanceDetails; DescribeDomainNodes models DependencyFailureException (424) for a census it cannot fetch. Recorded behavior (the check's measured real-AWS trace, us-west-2): after the domain's node census becomes readable, StartDomainMaintenance(Action=RESTART_SEARCH_PROCESS, NodeId=<a listed data node>) is accepted and the action then advances PENDING -> IN_PROGRESS -> a terminal status, and the terminal status a SUCCESSFUL action reports is the unmodelled value "SUCCESS" (not the COMPLETED the enum documents). The requirement exercised here is that the action DOES advance to a terminal status on the reads a polling client makes.

Emulator: the full sequence CreateDomain(ClusterConfig.InstanceCount=3) -> DescribeDomainNodes -> StartDomainMaintenance(RESTART_SEARCH_PROCESS, NodeId=<first listed data node>) -> GetDomainMaintenanceStatus polling never yields a terminal Status: the check consumes its entire window and is killed with `Error: Check timed out after 60.0s`, having never read a terminal value. Two emulator-side windows each cost a polling client a whole poll interval before the state machine moves: (a) the freshly created domain is held `work_in_flight` for `state::DOMAIN_WORK` (3 s) and DescribeDomainNodes refuses the census with DependencyFailureException for that whole window (describe_domain_nodes.rs handler, work_in_flight gate), and (b) the maintenance record is minted with status PENDING and its start/completion instants are due only after later wall-clock instants (`state::MAINTENANCE_STEP`), materialised by whichever later read happens to come in, so the read a client makes right after the start still returns PENDING. Taken together the action is still non-terminal on every read inside the window and the lifecycle never completes.

Separate, already-filed sub-behavior: where the status DOES settle, the terminal value the emulator writes is COMPLETED (`state::MAINTENANCE_STATUS_COMPLETED`) rather than the SUCCESS AWS reports. That value is Cause 2 of the OPEN issue 20261029T182036Z-opensearch-domain-upgrade-and-maintenance-state-machines-report.md, which carries this check's old replay key (`assert final_status == "SUCCESS"`). That key no longer matches: the check no longer gets as far as a terminal status, so the flow-never-terminates defect described above is NOT covered by that issue and is filed here.

Why: a client that starts node maintenance has no way to learn that it finished. It calls StartDomainMaintenance, gets a MaintenanceId, then polls GetDomainMaintenanceStatus and keeps reading PENDING until it gives up — exactly the loop every AWS-targeting automation around RESTART_SEARCH_PROCESS/REBOOT_NODE/RESTART_DASHBOARD runs. Orchestration that gates a subsequent step (another maintenance action, a config change, a health check) on the action's terminal status blocks forever, and negative paths (FAILED/TIMED_OUT) are equally unreachable, so no maintenance outcome whatsoever can be asserted against the emulator.

Fix: the maintenance state machine's progression in the StartDomainMaintenance / GetDomainMaintenanceStatus handlers and the domain state it reads, plus the in-flight gate DescribeDomainNodes consults — an accepted action must advance through PENDING -> IN_PROGRESS -> a terminal status over the reads a client actually makes (rather than only after wall-clock spans that each consume a client poll interval), and the node census of a just-created domain must become readable in the same bounded way. Not a patch: the pacing constants and the settle step are the components named under Traces below.

Traces: opensearch.domain-maintenance-lifecycle. Code citations: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/start_domain_maintenance.rs:206 (begin_maintenance), services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/describe_domain_nodes.rs:389 (work_in_flight -> DependencyFailureException), services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs:446 (DOMAIN_WORK = 3 s), src/state.rs:5868 (MAINTENANCE_STEP = 500 ms), src/state.rs:4891 (Domain::begin_maintenance, status PENDING with starts_at/completes_at), src/state.rs:5903 (Maintenance::settle, PENDING -> IN_PROGRESS -> COMPLETED).

## Evidence
Failing check: [mismatch] opensearch.domain-maintenance-lifecycle — `Error: Check timed out after 60.0s`.

Expected (AWS, recorded): StartDomainMaintenance(DomainName=<3-data-node domain>, Action='RESTART_SEARCH_PROCESS', NodeId=<NodeId read from DescribeDomainNodes>) -> 200 {MaintenanceId}; GetDomainMaintenanceStatus(DomainName, MaintenanceId) -> Status advancing PENDING -> IN_PROGRESS -> a terminal value (recorded terminal value for a successful action: 'SUCCESS'; the modelled MaintenanceStatus enum lists PENDING|IN_PROGRESS|COMPLETED|FAILED|TIMED_OUT), with Action='RESTART_SEARCH_PROCESS' and NodeId echoing the requested node, and the MaintenanceId present in ListDomainMaintenances before and after completion.

Actual (emulator): no terminal Status is ever read — the check is killed at the 60 s cap mid-poll. Observed emulator behaviour of the individual calls: DescribeDomainNodes on the just-created domain is refused with DependencyFailureException (424, message 'The node information is not available now, please try again later') for the whole DOMAIN_WORK window, and the maintenance record created by StartDomainMaintenance reports Status='PENDING' on the read a client makes next, moving to IN_PROGRESS/COMPLETED only once MAINTENANCE_STEP wall-clock spans have elapsed and a later read materialises them. The terminal value the emulator eventually writes is 'COMPLETED', not 'SUCCESS' (that value difference is Cause 2 of the open issue 20261029T182036Z-opensearch-domain-upgrade-and-maintenance-state-machines-report.md).

Fixed-text vs variable parts of the messages quoted above: 'The node information is not available now, please try again later' is fixed text of DependencyFailureException for this operation; the DomainName, NodeId and MaintenanceId values are taken from the request and from emulator state and are not part of any requirement.

## Steps to Reproduce
1. opensearch.create_domain(DomainName=<lowercase name matching ^[a-z][a-z0-9\-]+$, 3..28 chars>, EngineVersion='OpenSearch_2.13', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':3,'DedicatedMasterEnabled':False,'ZoneAwarenessEnabled':False}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}).
2. Poll opensearch.describe_domain_nodes(DomainName=<d>) until it answers 200 with a non-empty DomainNodesStatusList, retrying on DependencyFailureException/ResourceNotFoundException/ValidationException — note the DependencyFailureException window.
3. node_id = DomainNodesStatusList[0]['NodeId']; opensearch.start_domain_maintenance(DomainName=<d>, Action='RESTART_SEARCH_PROCESS', NodeId=node_id) -> MaintenanceId.
4. opensearch.get_domain_maintenance_status(DomainName=<d>, MaintenanceId=<id>) and assert Action/NodeId echo the request; opensearch.list_domain_maintenances(DomainName=<d>) and assert the MaintenanceId is listed.
5. Poll opensearch.get_domain_maintenance_status(DomainName=<d>, MaintenanceId=<id>) at a client-realistic interval until Status is terminal: the emulator keeps returning PENDING and the sequence never reaches a terminal status before the replay window expires (60 s). Replay opensearch.domain-maintenance-lifecycle to confirm.
