Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted
Issue-Key: ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted|_outlast_lookback#29|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: ad889ea564c74ff66a8d5d7080e6fcd3e62d9684

# The Managed Microsoft AD log-subscription forwarding scenario never settles — no domain-controller security event ever reaches the subscribed log group

> _Note: this issue was identified against the code base at commit `ad889ea564c74ff66a8d5d7080e6fcd3e62d9684`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLogSubscription

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: the end-to-end Directory Service log-forwarding scenario reaches no verdict. The check creates a Managed Microsoft AD, opens its data plane, creates a log group with a ds.amazonaws.com resource policy, subscribes the directory to it, and then asks for a forwarded domain-controller security event naming a ds-data user it just created. It is cut at the 60s replay budget with no verdict, so neither the delivery leg nor the withhold leg is observable.

AWS (Smithy: service-com-amazonaws-directoryservice-directoryservice-20150416):
  - CreateMicrosoftAD returns a DirectoryId matching `^d-[0-9a-f]{10}$`; DescribeDirectories then reports the directory advancing to Stage `Active` (the check accepts any of Active/Inoperable/Impaired/Failed/Deleted as terminal and requires Active). Its modelled failures are ClientException, DirectoryLimitExceededException, InvalidParameterException, UnsupportedOperationException (all 400) and ServiceException (500) — none of which was returned here.
  - EnableDirectoryDataAccess returns an empty success body and DescribeDirectoryDataAccess must then report DataAccessStatus `Enabled` (modelled refusals: AccessDeniedException, ClientException, DirectoryDoesNotExistException, DirectoryInDesiredStateException, DirectoryUnavailableException, UnsupportedOperationException, ServiceException).
  - CreateLogSubscription takes DirectoryId (`^d-[0-9a-f]{10}$`) and LogGroupName (@length 1..512, pattern `^[-._/#A-Za-z0-9]+$`), returns an empty success body, and is refused with EntityAlreadyExistsException / EntityDoesNotExistException / InsufficientPermissionsException where applicable. Recorded behavior for the forwarding itself: once subscribed, domain-controller security logs — including the audited user-account-management category — are forwarded into the named log group, so FilterLogEvents on that group returns an event whose message names the created SAM account; events predating the subscription are not transferred, and events produced while the subscription is deleted are never forwarded, not even retroactively after it is re-created.

Emulator: the scenario produces no observable at all within the budget. The check's own waits are generous (3600s for the directory stage, 300s for data access, 900s for the first forwarded event) and it is the replay budget of 60s that cuts it, which means at least one of those states never settles promptly: the directory never reports Stage Active, and/or DataAccessStatus never reaches Enabled, and/or the subscribed log group never receives a forwarded event for the ds-data user. On a real emulator these are synthetic state transitions with no provisioning to wait for, so none of them should take minutes.

Why: a client cannot verify that Directory Service audit forwarding works at all, nor the security property the scenario exists for — that deleting the log subscription actually WITHHOLDS domain-controller security events and that re-creating it restores delivery. Compliance tooling that reads `/aws/directoryservice/<...>` for account-management audit records sees an empty group and cannot tell "nothing happened" from "forwarding is broken".

Related open ds issues cover different surfaces and do not carry this replay key: 20261107T045342Z-ds-7-net-new-causes-four-refusal-report.md and 20261106T203453Z-ds-ec2-ds-2-net-new-causes-describedirectories-vpcsettings-omits.md (refusal/report strings), 20261107T042250Z-ds-disabledirectorydataaccess-does-not-close-the-data-plane.md (data-plane gate).

Fix: the CreateMicrosoftAD / DescribeDirectories stage state machine, the EnableDirectoryDataAccess → DescribeDirectoryDataAccess status transition, and the CreateLogSubscription / DeleteLogSubscription handler's forwarding path that writes domain-controller security events into the subscribed CloudWatch Logs group (and stops writing while no subscription exists).

Traces: ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted.

## Evidence
Check ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted: `Error: Check timed out after 60.0s`.

The check's sequence and what AWS answers at each step:
1. CreateMicrosoftAD(Name=<fqdn>, ShortName=SIM2REAL, Edition=Standard, VpcSettings={VpcId, two SubnetIds in distinct AZs}) -> DirectoryId `d-xxxxxxxxxx`.
2. Poll DescribeDirectories(DirectoryIds=[id]) until Stage is terminal; assert Stage == "Active".
3. EnableDirectoryDataAccess(DirectoryId) then poll DescribeDirectoryDataAccess until DataAccessStatus == "Enabled".
4. logs:CreateLogGroup("/aws/directoryservice/<name>") + logs:PutResourcePolicy allowing Principal {"Service": "ds.amazonaws.com"} logs:CreateLogStream/logs:PutLogEvents on `arn:aws:logs:<region>:<account>:log-group:<group>:*`.
5. ds-data CreateUser (pre-subscription user), then ds:CreateLogSubscription(DirectoryId, LogGroupName); ListLogSubscriptions must report exactly that one subscription.
6. ds-data CreateUser again and poll logs:FilterLogEvents(logGroupName=<group>, startTime=<run start>) for an event whose message names that SAMAccountName.
7. DeleteLogSubscription, create another user, and assert over a window that no event names it and that the group's storedBytes does not advance; then re-create the subscription and assert delivery resumes.

Expected (AWS): step 6 yields at least one forwarded domain-controller security event naming the user; steps 5 and 7 yield none for the pre-subscription and withheld users.
Actual (emulator): the run is cut at 60s before any verdict — the check's own budgets for these steps are 3600s / 300s / 900s respectively, so the cut means one of the state transitions or the forwarding never happens promptly rather than that the check ran out of its own patience. No modelled ds error was returned at any step.

## Steps to Reproduce
Replay ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted.

Minimal probe of each leg, independent of the full check:
1. CreateMicrosoftAD with a VpcSettings naming two subnets in distinct AZs; poll DescribeDirectories and time how long Stage takes to reach "Active" (AWS: it does reach Active; the emulator should reach it immediately).
2. EnableDirectoryDataAccess, then poll DescribeDirectoryDataAccess for DataAccessStatus == "Enabled".
3. logs:CreateLogGroup a "/aws/directoryservice/..." group and logs:PutResourcePolicy granting ds.amazonaws.com logs:CreateLogStream + logs:PutLogEvents on it.
4. ds:CreateLogSubscription(DirectoryId, LogGroupName); confirm ListLogSubscriptions reports it.
5. ds-data CreateUser(DirectoryId, SAMAccountName=<name>); then poll logs:FilterLogEvents(logGroupName=<group>) for an event whose message contains <name>. Expect at least one; the emulator yields none.
6. ds:DeleteLogSubscription, create another user, and confirm no event names it and storedBytes does not advance; re-create the subscription and confirm delivery resumes for a third user.
