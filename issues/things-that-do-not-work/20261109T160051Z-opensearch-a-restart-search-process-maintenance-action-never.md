Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.domain-maintenance-status-window-expiry
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.domain-maintenance-status-window-expiry|run#10|AssertionError|assert final is not None, (
Issue-Commit: 44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07

# opensearch: a RESTART_SEARCH_PROCESS maintenance action never reaches a terminal GetDomainMaintenanceStatus Status, so ListDomainMaintenances never shows SUCCESS

> _Note: this issue was identified against the code base at commit `44786d0c9f40e8c1d9dd84c42bcd4a9c51c55d07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomainMaintenanceStatus

## Description
This issue carries 1 root cause, in the opensearch service. It must be closed before submitting.

Cause 1 of 1: a node maintenance action the emulator accepted never advances to a terminal status on the reads a polling client makes — GetDomainMaintenanceStatus keeps answering a non-terminal Status for the whole execution window, so the action's completion (and therefore the SUCCESS row ListDomainMaintenances must show) is unobservable.

AWS: Smithy: StartDomainMaintenance POST /2021-01-01/opensearch/domain/{DomainName}/domainMaintenance (DomainName @httpLabel required @length 3..28 @pattern ^[a-z][a-z0-9\\-]+$, Action MaintenanceType required, NodeId @length 10..40) -> MaintenanceId (RequestId @length 1..100); GetDomainMaintenanceStatus GET /2021-01-01/opensearch/domain/{DomainName}/domainMaintenance?maintenanceId=<id> -> Status (MaintenanceStatus enum), StatusMessage, NodeId, Action, CreatedAt, UpdatedAt; ListDomainMaintenances GET /2021-01-01/opensearch/domain/{DomainName}/domainMaintenances -> DomainMaintenances. Recorded behavior: on a 3-data-node domain, once DescribeDomainNodes lists a node, StartDomainMaintenance(Action='RESTART_SEARCH_PROCESS', NodeId=<a listed data node>) is accepted and the action then advances to a terminal status within its execution window on ordinary 5 s polls; the terminal value a successful action reports is the unmodelled string 'SUCCESS', and the same MaintenanceId is listed by ListDomainMaintenances with Status 'SUCCESS'. The requirement exercised here is that the accepted action DOES become terminal with no further client-driven step.

Emulator: the sequence CreateDomain(InstanceCount=3) -> DescribeDomainNodes -> StartDomainMaintenance(RESTART_SEARCH_PROCESS, NodeId=<first listed node>) -> GetDomainMaintenanceStatus polled every 5 s never yields a Status in {SUCCESS, COMPLETED, FAILED, TIMED_OUT}: the poll loop exhausts its window and the check fails with 'maintenance action never reached a terminal Status within its execution window, with no additional client-driven step issued'. The downstream assertions (Status == 'SUCCESS', and the MaintenanceId listed by ListDomainMaintenances with that status) are therefore never reached.

Why: a client that starts node maintenance has no way to learn that it finished. Automation around RESTART_SEARCH_PROCESS/REBOOT_NODE/RESTART_DASHBOARD starts the action, gets a MaintenanceId and polls until terminal — against the emulator that loop never exits, so any step gated on completion blocks forever, and the failure paths (FAILED/TIMED_OUT) are equally unreachable.

Fix: the maintenance state machine behind StartDomainMaintenance / GetDomainMaintenanceStatus / ListDomainMaintenances and the domain state it reads — an accepted action must progress through its non-terminal statuses to a terminal one over the reads a client actually makes, and the terminal value a successful action reports must be the SUCCESS that AWS reports (also carried by ListDomainMaintenances for the same MaintenanceId), rather than resting on a non-terminal value for the life of the window. Open issue 20261030T115510Z-opensearch-the-startdomainmaintenance-getdomainmaintenancestatus-lifecycle-never-reaches-a.md describes the same behavior but carries only the replay key of opensearch.domain-maintenance-lifecycle; it does not carry this check, so this failure is filed here and should be closed together with it.

Traces: opensearch.domain-maintenance-status-window-expiry.

## Evidence
Failing check: [mismatch] opensearch.domain-maintenance-status-window-expiry.
AssertionError: maintenance action never reached a terminal Status within its execution window, with no additional client-driven step issued.
Expected (AWS, recorded): get_domain_maintenance_status(DomainName=<d>, MaintenanceId=<id>) polled at a client-realistic interval returns a terminal Status within the action's execution window, final Status == 'SUCCESS' (a value outside the modelled MaintenanceStatus enum PENDING|IN_PROGRESS|COMPLETED|FAILED|TIMED_OUT), with Action and NodeId echoing the request; list_domain_maintenances(DomainName=<d>) then contains an entry whose MaintenanceId equals the started action's id with Status 'SUCCESS'.
Actual (emulator): every poll inside the window returns a non-terminal Status (never one of SUCCESS/COMPLETED/FAILED/TIMED_OUT), so no terminal status and no SUCCESS row is ever observable. The domain name, node id, maintenance id and the concrete poll timings are per-run values and are not part of the requirement; the requirement is that the accepted action becomes terminal and reports SUCCESS.

## Steps to Reproduce
1. opensearch.create_domain(DomainName=<lowercase name matching ^[a-z][a-z0-9\\-]+$, 3..28 chars>, ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':3}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, AccessPolicies=<es:* for the account root on the domain>).
2. Poll opensearch.describe_domain(DomainName=<d>) until DomainStatus.Processing is False.
3. Poll opensearch.describe_domain_nodes(DomainName=<d>) until DomainNodesStatusList has an entry with a NodeId; node_id = that NodeId.
4. opensearch.start_domain_maintenance(DomainName=<d>, Action='RESTART_SEARCH_PROCESS', NodeId=node_id) -> MaintenanceId (retry on DependencyFailureException/ThrottlingException and on ResourceNotFoundException 'The requested node ID is invalid').
5. Poll opensearch.get_domain_maintenance_status(DomainName=<d>, MaintenanceId=<id>) every 5 s with no other call in between: the emulator never returns a Status in {SUCCESS, COMPLETED, FAILED, TIMED_OUT}. On AWS it becomes 'SUCCESS' and opensearch.list_domain_maintenances(DomainName=<d>) then lists that MaintenanceId with Status 'SUCCESS'.
6. Re-replay opensearch.domain-maintenance-status-window-expiry.
