Issue-Source: trace-sweep-divergence
Issue-Trace: detective-datasource-packages-workflow
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: detective-datasource-packages-workflow detective-member-invitation-workflow detective-investigation-error-paths
Issue-Shape-Id: com.amazonaws.detective#AmazonDetective
Issue-Key: detective|ListDatasourcePackages|2xx->2xx|$.DatasourcePackages.ASFF_SECURITYHUB_FINDING
Issue-Key: detective|ListDatasourcePackages|2xx->2xx|$.DatasourcePackages.EKS_AUDIT
Issue-Key: detective|BatchGetGraphMemberDatasources|2xx->2xx|$.MemberDatasources[*].DatasourcePackageIngestHistory.ASFF_SECURITYHUB_FINDING
Issue-Key: detective|CreateMembers|2xx->2xx|$.Members[*].DatasourcePackageIngestStates
Issue-Key: detective|CreateMembers|2xx->2xx|$.Members[*].InvitationType
Issue-Key: detective|GetMembers|2xx->2xx|$.MemberDetails[*]
Issue-Key: detective|ListMembers|2xx->2xx|$.MemberDetails[*]
Issue-Key: detective|GetInvestigation|2xx->2xx|$.State
Issue-Key: detective|GetInvestigation|2xx->2xx|$.Status
Issue-Commit: 0711e0a570adac04732f4f058a2849d70bc38321

# Detective reads omit datasource-package state, the administrator's own membership and investigation state

> _Note: this issue was identified against the code base at commit `0711e0a570adac04732f4f058a2849d70bc38321`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDatasourcePackages

## Description
Eight net-new root causes, all of them the same family: a Detective read answers 200 with a well-formed envelope whose per-datasource-package maps, administrator self-membership record and investigation lifecycle fields are simply absent or stale. The emulator tracks only the DETECTIVE_CORE package and never re-reads the state a prior write in the same fixture established. Every cause below MUST be closed before submitting; they are eight distinct gaps, not one.

Cause 1 of 8: ListDatasourcePackages reports only DETECTIVE_CORE.
AWS: Smithy: ListDatasourcePackages.DatasourcePackages -> DatasourcePackageIngestDetails, a map keyed by enum DatasourcePackage whose values are DETECTIVE_CORE, EKS_AUDIT and ASFF_SECURITYHUB_FINDING; each value is a DatasourcePackageIngestDetail {DatasourcePackageIngestState (STARTED|STOPPED|DISABLED), LastIngestStateChange -> LastIngestStateChangeDates, a map keyed by that same ingest-state enum whose values are TimestampForCollection{Timestamp}}. The recording shows the graph answering an entry for EVERY package the graph has had enabled on it, so after UpdateDatasourcePackages enables the non-core packages the map carries all three keys, each STARTED with a LastIngestStateChange entry under the STARTED key holding the time that package's collection began.
Emulator: the map carries DETECTIVE_CORE alone. Replay reports $.DatasourcePackages.ASFF_SECURITYHUB_FINDING and $.DatasourcePackages.EKS_AUDIT as missing, where the recording has each of them STARTED with a LastIngestStateChange.STARTED.Timestamp. Status matches (200->200); only the map's contents diverge.
Why: a client enumerating which packages are ingesting on a graph (the console's data-source page, or any automation that enables EKS_AUDIT and then polls for it to start) sees the package it just enabled as not present at all, so it concludes the enable silently failed and either retries forever or reports the graph as core-only.
Fix: the ListDatasourcePackages handler's projection of graph datasource state, and the graph state it reads: the per-package ingest record must be created for every package UpdateDatasourcePackages enabled, not only for DETECTIVE_CORE.
Traces: detective-datasource-packages-workflow#task#2.

Cause 2 of 8: a package's STOPPED transition is not recorded, so ListDatasourcePackages drops the package entirely instead of reporting it stopped.
AWS: per the same shapes as Cause 1, disabling a package does NOT remove its key: the entry stays, its DatasourcePackageIngestState becomes STOPPED, and LastIngestStateChange accumulates BOTH keys - the STARTED timestamp from when it was first enabled and a STOPPED timestamp for the transition just made. The recording shows exactly that history preserved across the enable-then-disable sequence.
Emulator: after the package is disabled, $.DatasourcePackages.ASFF_SECURITYHUB_FINDING is missing rather than present-and-STOPPED with its two-entry LastIngestStateChange. 200->200.
Why: a client cannot distinguish "this package was never enabled" from "this package was enabled and is now stopped", and loses the ingest-history timestamps it uses to attribute data gaps to a window.
Fix: the graph-level datasource state (the record UpdateDatasourcePackages mutates) must transition a package's ingest state and append to its last-ingest-state-change history instead of removing or never materialising the entry; ListDatasourcePackages then projects it.
Traces: detective-datasource-packages-workflow#task#4.

Cause 3 of 8: BatchGetGraphMemberDatasources omits non-core packages from DatasourcePackageIngestHistory.
AWS: Smithy: BatchGetGraphMemberDatasources.MemberDatasources -> MembershipDatasources{AccountId, GraphArn, DatasourcePackageIngestHistory}, where DatasourcePackageIngestHistory is a map keyed by DatasourcePackage whose value is LastIngestStateChangeDates (ingest-state enum -> TimestampForCollection). A member's history therefore carries one key per package that has ever been enabled for that membership, each holding the state-change timestamps for that package.
Emulator: $.MemberDatasources[0].DatasourcePackageIngestHistory.ASFF_SECURITYHUB_FINDING is missing, where the recording carries that key with a STARTED timestamp (and no STOPPED entry yet).
Why: per-member data-source attribution is the whole point of this batch read; a member that is ingesting a package appears to a client as not ingesting it, so coverage reporting under-counts and remediation automation re-enables a package that is already on.
Fix: the BatchGetGraphMemberDatasources handler's per-member ingest-history projection, over the same member datasource state Cause 1 and 2 fail to populate.
Traces: detective-datasource-packages-workflow#task#5.

Cause 4 of 8: CreateMembers omits DatasourcePackageIngestStates from the returned member and populates InvitationType too early.
AWS: Smithy: CreateMembers.Members -> MemberDetail, which carries DatasourcePackageIngestStates (map DatasourcePackage -> DatasourcePackageIngestState) and InvitationType (enum INVITATION|ORGANIZATION). The recording shows the created member already carrying a complete DatasourcePackageIngestStates map - one entry per package the graph knows, with the graph's current per-package state (core STARTED, the packages not enabled for the new member STOPPED) - while InvitationType is absent on the CreateMembers response itself and only appears on later reads of the same member (recorded behavior only: the model documents the member's type but not when it is first populated).
Emulator: $.Members[0].DatasourcePackageIngestStates is missing where the recording has a three-key map, and $.Members[0].InvitationType is served as 'INVITATION' where the recorded create response has no such member at all. 200->200.
Why: a client that creates members and reads back their package coverage from the create response gets nothing to key on, and a client that treats the presence of InvitationType as "the invitation has been issued" is told so before the recording says it is.
Fix: the CreateMembers handler's MemberDetail serializer and the member record it builds - seed the per-package ingest states from the graph's package state, and leave the invitation type unset on the create response.
Traces: detective-member-invitation-workflow#task#2.

Cause 5 of 8: GetMembers' MemberDetail omits DatasourcePackageIngestStates.
AWS: as Cause 4's MemberDetail: a member read back by GetMembers carries DatasourcePackageIngestStates with one entry per known package and that member's current state for it. (The absent DisabledReason, VolumeUsage* and PercentOfGraphUtilization* members are not at issue - the recording leaves those unset too, and the emulator correctly omits them.)
Emulator: the whole $.MemberDetails[0] object compares unequal for exactly one reason: the served member has AccountId, EmailAddress, GraphArn, MasterId, AdministratorId, Status, InvitedTime, UpdatedTime and InvitationType, and no DatasourcePackageIngestStates at all. 200->200.
Why: this read is how a client learns whether a member it invited is contributing each package's data; without the map it cannot tell a stopped package from an unknown one and must fall back to the batch datasource read, which (Cause 3) is equally incomplete.
Fix: the shared MemberDetail serializer / member-state projection used by GetMembers.
Traces: detective-member-invitation-workflow#task#3.

Cause 6 of 8: ListMembers omits DatasourcePackageIngestStates on the invited member AND omits the administrator's own membership record entirely.
AWS: Smithy: ListMembers.MemberDetails -> MemberDetailList of the same MemberDetail. The recording lists TWO members for a graph with one invited account: the invited member (Status VERIFICATION_FAILED, InvitationType INVITATION, full DatasourcePackageIngestStates map) and the administrator account's own membership (AccountId equal to the graph owner, no EmailAddress, Status ENABLED, no InvitationType, and every package STARTED). Recorded behavior only: the model does not state that the administrator is listed as a member of its own graph, but the recording shows it is.
Emulator: $.MemberDetails[0] diverges exactly as in Cause 5 (no DatasourcePackageIngestStates), and $.MemberDetails[1] is missing outright - the administrator's membership record is never created, so the listing is short by one. 200->200.
Why: a client reconciling graph membership (billing attribution, "who contributes to this graph") sees the administrator account as a non-member of the graph it administers, and every member it does see has no package coverage.
Fix: the graph's member state - a graph must hold an ENABLED self-membership for its administrator from creation - plus the same MemberDetail projection as Cause 5.
Traces: detective-member-invitation-workflow#task#4.

Cause 7 of 8: after the invited member is removed, ListMembers answers an empty listing instead of the administrator's own membership.
AWS: by Cause 6's recorded rule, removing the invited member leaves the administrator's self-membership behind, so the listing still carries one MemberDetail: the administrator account, no EmailAddress, Status ENABLED, no InvitationType, all packages STARTED.
Emulator: $.MemberDetails[0] is missing - the listing is empty. 200->200.
Why: a client reading the listing after a removal concludes the graph has no members and no contributing accounts at all, which is a different graph state than AWS reports; teardown automation that waits for "only the administrator remains" never converges.
Fix: same administrator self-membership in graph state as Cause 6 - the member-removal path must not be able to empty a graph of its administrator's record.
Traces: detective-member-invitation-workflow#task#6.

Cause 8 of 8: GetInvestigation reports the investigation's original lifecycle values, not the ones the workflow set.
AWS: Smithy: GetInvestigation.State -> State enum (ACTIVE|ARCHIVED) and GetInvestigation.Status -> Status enum (RUNNING|FAILED|SUCCESSFUL), 'the status based on the completion status of the investigation'; UpdateInvestigationState takes a required State and is the door that archives an investigation. A read after the investigation has been archived therefore reports State ARCHIVED, and an investigation whose run did not complete successfully reports Status FAILED.
Emulator: $.State is 'ACTIVE' where the recording has 'ARCHIVED', and $.Status is 'SUCCESSFUL' where the recording has 'FAILED' - i.e. the archive write is not reflected in the read, and the completion status is hard-answered SUCCESSFUL rather than taken from the investigation's outcome. 200->200.
Why: a client archiving an investigation and polling GetInvestigation to confirm never sees ARCHIVED, so it retries or treats the archive as rejected; and because every investigation reads back SUCCESSFUL, a failed investigation is reported as a completed one and its findings are consumed as trustworthy.
Fix: the investigation record in state (UpdateInvestigationState must persist State; the investigation's completion status must be stored and read back) and the GetInvestigation handler's projection of it.
Traces: detective-investigation-error-paths#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 56.2%, 12 divergence clusters, all service=detective, none stub-blocked, none owned by an open issue - the 9 prior detective issues are all in the resolved folder).

Cause 1 - detective-datasource-packages-workflow#task#2, ListDatasourcePackages, expected 200 / actual 200:
  $.DatasourcePackages.ASFF_SECURITYHUB_FINDING: missing (expected {'DatasourcePackageIngestState': 'STARTED', 'LastIngestStateChange': {'DISABLED': None, 'STOPPED': None, 'STARTED': {'Timestamp': <iso8601>}}}, got None)
  $.DatasourcePackages.EKS_AUDIT: missing (same expected shape, got None)

Cause 2 - detective-datasource-packages-workflow#task#4, ListDatasourcePackages, 200/200:
  $.DatasourcePackages.ASFF_SECURITYHUB_FINDING: missing (expected {'DatasourcePackageIngestState': 'STOPPED', 'LastIngestStateChange': {'DISABLED': None, 'STOPPED': {'Timestamp': <iso8601>}, 'STARTED': {'Timestamp': <iso8601>}}}, got None)

Cause 3 - detective-datasource-packages-workflow#task#5, BatchGetGraphMemberDatasources, 200/200:
  $.MemberDatasources[0].DatasourcePackageIngestHistory.ASFF_SECURITYHUB_FINDING: missing (expected {'STOPPED': None, 'STARTED': {'Timestamp': <iso8601>}}, got None)

Cause 4 - detective-member-invitation-workflow#task#2, CreateMembers, 200/200:
  $.Members[0].DatasourcePackageIngestStates: missing (expected {'ASFF_SECURITYHUB_FINDING': 'STOPPED', 'DETECTIVE_CORE': 'STARTED', 'EKS_AUDIT': 'STOPPED'}, got None)
  $.Members[0].InvitationType: type_mismatch (expected None, got 'INVITATION')

Cause 5 - detective-member-invitation-workflow#task#3, GetMembers, 200/200:
  $.MemberDetails[0]: expected the member with DatasourcePackageIngestStates {'ASFF_SECURITYHUB_FINDING': 'STOPPED', 'DETECTIVE_CORE': 'STARTED', 'EKS_AUDIT': 'STOPPED'}; got the same member (AccountId, EmailAddress, GraphArn, MasterId, AdministratorId, Status VERIFICATION_FAILED, InvitedTime, UpdatedTime, InvitationType INVITATION) with NO DatasourcePackageIngestStates member.

Cause 6 - detective-member-invitation-workflow#task#4, ListMembers, 200/200:
  $.MemberDetails[0]: as Cause 5 (DatasourcePackageIngestStates absent)
  $.MemberDetails[1]: missing - expected the administrator's own membership {AccountId=<graph owner>, EmailAddress unset, GraphArn, MasterId/AdministratorId=<graph owner>, Status 'ENABLED', InvitationType unset, DatasourcePackageIngestStates {'ASFF_SECURITYHUB_FINDING': 'STARTED', 'DETECTIVE_CORE': 'STARTED', 'EKS_AUDIT': 'STARTED'}}, got None

Cause 7 - detective-member-invitation-workflow#task#6, ListMembers, 200/200:
  $.MemberDetails[0]: missing - expected the administrator's own ENABLED membership (all three packages STARTED), got None (empty listing)

Cause 8 - detective-investigation-error-paths#task#8, GetInvestigation, 200/200:
  $.State: value_mismatch (expected 'ARCHIVED', got 'ACTIVE')
  $.Status: value_mismatch (expected 'FAILED', got 'SUCCESSFUL')

All timestamps, account ids, graph ARNs and email addresses above are per-run values that fill the template; the requirement is the presence and shape of the members and the enum values, not any recorded literal.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it, then replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1-3: replay fixture detective-datasource-packages-workflow. Steps task#2 and task#4 call ListDatasourcePackages on the graph after UpdateDatasourcePackages has enabled (then disabled) EKS_AUDIT / ASFF_SECURITYHUB_FINDING; step task#5 calls BatchGetGraphMemberDatasources for the member. Observe the DatasourcePackages / DatasourcePackageIngestHistory maps carry only DETECTIVE_CORE.
3. Cause 4-7: replay fixture detective-member-invitation-workflow. task#2 CreateMembers, task#3 GetMembers, task#4 ListMembers (two members expected), task#6 ListMembers after the invited member is removed (administrator's own record expected). Observe the missing DatasourcePackageIngestStates on every MemberDetail, the eagerly-set InvitationType on the create response, and the absent administrator self-membership.
4. Cause 8: replay fixture detective-investigation-error-paths through task#8; after the investigation has been archived via UpdateInvestigationState, GetInvestigation still answers State ACTIVE and Status SUCCESSFUL.
5. Each step answers HTTP 200 as recorded, so the divergence is in the response body only.
