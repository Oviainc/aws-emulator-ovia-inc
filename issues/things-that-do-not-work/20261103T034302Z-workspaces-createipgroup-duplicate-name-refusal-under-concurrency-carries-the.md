Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces.concurrent-ip-group-creation-race
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces.concurrent-ip-group-creation-race|run#13|AssertionError|assert all(m == expected_message for m in rejected_messages), (
Issue-Commit: 80501f644d2883f30c53d819479800a81248de03

# CreateIpGroup duplicate-name refusal under concurrency carries the emulator's own message instead of AWS's "Encountered an exception while creating IP Group - group already exists for given group name - <GroupName>"

> _Note: this issue was identified against the code base at commit `80501f644d2883f30c53d819479800a81248de03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIpGroup

## Description
One net-new root cause in the WorkSpaces emulator, surfaced on the concurrent-create path.

Cause 1 of 1: when several CreateIpGroup calls race on one GroupName, the emulator correctly lets exactly one win and refuses the rest with HTTP 400 / ResourceAlreadyExistsException, but the MESSAGE every rejected attempt carries is not the message AWS words for that error class.
  AWS: Smithy (services/service-com-amazonaws-workspaces-workspacesservice/smithy-model/com-amazonaws-workspaces-workspacesservice.json) models CreateIpGroup with input GroupName (IpGroupName, @required), output GroupId (IpGroupId, @pattern ^wsipg-[0-9a-z]{8,63}$) and error ResourceAlreadyExistsException (client, HTTP 400). The model gives that error shape only the generic doc sentence "The specified resource already exists."; the per-operation message is recorded behavior only. Recorded behavior: AWS refuses a create whose GroupName is already held with HTTP 400, code ResourceAlreadyExistsException, and message "Encountered an exception while creating IP Group - group already exists for given group name - <GroupName>". Everything up to and including the final " - " is FIXED text; the single variable part is <GroupName>, taken verbatim from the request. The message names only the requested NAME — it never names the id of the group that already holds it, and it carries no trailing period. This holds identically for every loser of a concurrent race, since each is the same duplicate-name refusal.
  Emulator: every rejected attempt carries message "The IP access control group <existing GroupId> is already named <GroupName>." — different fixed wording, a trailing period AWS does not emit, and it interpolates the pre-existing group's generated id, a per-run value AWS never puts in this message.
  Why: a client that recognizes the duplicate-name condition by message text, or surfaces it to an operator, gets wholly different text than against AWS, so message-based branching and user-facing diagnostics that work on AWS do not work here. Worse on this path: because the emulator's text embeds a generated wsipg- id, a consumer keying on the message sees a value that changes every run and that AWS does not emit at all — so a retry/contention handler keyed on the message is non-deterministic here.
  Fix: the CreateIpGroup handler's name-conflict branch — the message it builds for the ResourceAlreadyExistsException returned from the state layer's NameTaken refusal. The conflict detection itself (single lock, one winner) is correct and must not change; only the constructed message must. Note the related open issue 20261029T192123Z-workspaces-error-message-text-diverges-from-aws-on.md describes this same handler branch from the single-threaded duplicate-name check; if that issue's Cause 2 is fixed, re-replaying the check named below must also pass, since the concurrency path shares the branch. It is filed separately because the suite did not mark this check as owned by it.
  Traces: workspaces.concurrent-ip-group-creation-race. Handler and line: services/service-com-amazonaws-workspaces-workspacesservice/src/ops/create_ip_group.rs:201-211 (crate::state::IpGroupCreateError::NameTaken), message formatted at :204-207.

## Evidence
Suite: workspaces 5/10 (50%), 5 failed. Two of the five are ALREADY FILED stub blockages (workspaces CreateTags, ec2 CreateInternetGateway) and two are OWNED by 20261029T192123Z-workspaces-error-message-text-diverges-from-aws-on.md; this issue covers only the remaining one.

[mismatch] workspaces.concurrent-ip-group-creation-race
  8 concurrent CreateIpGroup calls on one GroupName -> 1 accepted, 7 refused. Codes were correct (all 7 ResourceAlreadyExistsException, so that assertion passed); the message assertion failed.
  Expected (AWS): Message = "Encountered an exception while creating IP Group - group already exists for given group name - <GroupName>" for each of the 7 rejected attempts (fixed prefix + the GroupName from the request).
  Actual (emulator): all 7 carried 'The IP access control group wsipg-00000001 is already named sim2real-wsipg-race-f4934d7ac91e.' — 'wsipg-00000001' is the winning group's generated id and 'sim2real-wsipg-race-...' is that run's generated name; both are per-run values filling the emulator's own template. The template is what diverges, not those values.
  Assertion: check_workspaces_concurrent_ip_group_creation_race.py:59 — AssertionError: every rejected attempt must carry 'Encountered an exception while creating IP Group - group already exists for given group name - <name>'.
  Source of the emulator string: services/service-com-amazonaws-workspaces-workspacesservice/src/ops/create_ip_group.rs:204-207.

Not yet exercised by this check (it aborts at the message assertion) but still part of it on re-replay: exactly one group must persist under the shared name in DescribeIpGroups, and its groupId must be one of the accepted GroupIds.

## Steps to Reproduce
1. Pick a GroupName not yet in use, e.g. "<unique-name>".
2. Issue 8 concurrent workspaces CreateIpGroup(GroupName="<unique-name>", GroupDesc="race", Tags=[...]) calls from 8 threads.
3. Observe that 1 call returns 200 with a GroupId and 7 return HTTP 400 with code ResourceAlreadyExistsException (correct) but Message 'The IP access control group <winning GroupId> is already named <unique-name>.'; AWS returns "Encountered an exception while creating IP Group - group already exists for given group name - <unique-name>" with no group id and no trailing period.
4. (On re-replay) DescribeIpGroups must show exactly one group under <unique-name>, with the accepted GroupId.

Replay: run_behavioral_suite and re-run workspaces.concurrent-ip-group-creation-race.
