Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted
Issue-Key: ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted|_outlast_lookback#29|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ds log subscription never delivers domain-controller security events to CloudWatch Logs — the subscription is stored but no event ever reaches the log group, and the check is cut off with no response

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLogSubscription

## Description
Cause 1 of 1.
AWS: ds CreateLogSubscription "Creates a subscription to forward real-time Directory Service domain controller security logs to the specified Amazon CloudWatch log group in your Amazon Web Services account" (Smithy com.amazonaws.directoryservice#CreateLogSubscription: input {DirectoryId required, pattern ^d-[0-9a-f]{10}$; LogGroupName required, len 1..512, pattern ^[-._/#A-Za-z0-9]+$}; errors ClientException 400, EntityAlreadyExistsException 400, EntityDoesNotExistException 400, InsufficientPermissionsException 400, ServiceException 500, UnsupportedOperationException 400). Recorded behavior for the forwarding itself (the model describes it in prose only): once a subscription exists on a directory whose domain controllers are up, an audited act on that directory — here a ds-data CreateUser — produces a security event record in the named CloudWatch Logs group, readable through logs FilterLogEvents/GetLogEvents and naming the account that was acted on; after DeleteLogSubscription no further events for that directory arrive in the group, and an act performed while no subscription exists is never forwarded.
Emulator: the check produced no response at all — the suite reports "Check timed out after 60.0s" for ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted, so the awaited delivery was never observed. The subscription record itself is accepted and stored (services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_log_subscription.rs:179-186 sets `directory.log_subscription = Some(LogSubscriptionRecord { log_group_name, created_at })`, with EntityDoesNotExistException for an unknown directory and EntityAlreadyExistsException for a second subscription), and ListLogSubscriptions reports it back, but nothing in the emulator turns a directory act into a log event: there is no producer that writes into the subscribed CloudWatch Logs group, so the positive control the check waits for cannot ever be satisfied.
Hypothesis (not stated by the model or the recording): the stored subscription is inert state only — the ds crate has no emitter wired to the logs service — which is why the check spends its whole budget polling an empty log group.
Why: a client that enables domain-controller security-log forwarding and then consumes those events (SIEM ingestion, an alarm on account-creation events, a metric filter on the group) gets an empty log group against the emulator, and the DeleteLogSubscription "stops forwarding" half of the contract is equally unobservable: both the presence and the absence of events read the same. Any workflow validated against the emulator will appear to work with zero events flowing.
Fix: the ds crate's log-subscription feature needs a delivery path from the directory's audited acts (including ds-data writes against that directory) into the subscribed CloudWatch Logs group — i.e. an emitter alongside services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_log_subscription.rs that creates/uses the log stream in `LogSubscriptionRecord.log_group_name` and stops when DeleteLogSubscription clears the record.
Budget caveat for the fixer: this check also carries its own fixed waits that exceed the suite's 60-second per-check cap (a 300s forwarding-lookback wait before each (re)subscription, plus multi-minute delivery and absence windows), so the cap alone truncates it even against a correct implementation. Re-replay it with a budget larger than those windows to confirm the fix; the timeout above is therefore evidence that no delivery was seen, not a measurement of where the run stalled.
Traces: ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted.

## Evidence
Failing check: ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted [mismatch] — "Error: Check timed out after 60.0s" (no response body recorded; the check's own deadlines are _DELIVERY_DEADLINE_S=900.0, _RESTORE_DEADLINE_S=1800.0, _FORWARDING_LOOKBACK_S=300.0, _MAX_ABSENCE_S=2700.0, all larger than the 60s cap).

Expected (AWS): after ds.create_log_subscription(DirectoryId=<id>, LogGroupName=<group>) and a ds-data CreateUser against <id>, logs.filter_log_events(logGroupName=<group>) eventually returns at least one domain-controller security event whose message names the created account; after ds.delete_log_subscription(DirectoryId=<id>), no further events for that directory appear in <group>; an account created before the (re)subscription is never forwarded.

Actual (emulator): no event is ever delivered — the check never reaches a verdict and is cut off at the harness cap. Code showing why nothing can be delivered: services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_log_subscription.rs:139-187 validates the directory, refuses an unknown one with EntityDoesNotExistException and a duplicate with EntityAlreadyExistsException, then stores `LogSubscriptionRecord { log_group_name, created_at }` on the directory and returns an empty output. No other component reads that record to produce log events into the named group.

Smithy consulted (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): CreateLogSubscription input DirectoryId (required, ^d-[0-9a-f]{10}$) and LogGroupName (required, len 1..512, ^[-._/#A-Za-z0-9]+$); doc "Creates a subscription to forward real-time Directory Service domain controller security logs to the specified Amazon CloudWatch log group in your Amazon Web Services account."; errors ClientException(400), EntityAlreadyExistsException(400), EntityDoesNotExistException(400), InsufficientPermissionsException(400), ServiceException(500), UnsupportedOperationException(400).

## Steps to Reproduce
1. Boot the emulator; create botocore clients for 'ds', 'ec2', 'logs' and 'ds-data' against it.
2. logs.create_log_group(logGroupName='/aws/directoryservice/sim2real-behav-<unique>') and attach the resource policy that lets ds.amazonaws.com write to it.
3. Build a VPC with two subnets in distinct AZs; ds.create_microsoft_ad(Name='sim2real-behav-<unique>.example.com', ShortName='SIM2REAL', Password='Sim2Real#Pass99', Edition='Standard', VpcSettings=...); poll ds.describe_directories until Stage == 'Active'.
4. ds.enable_directory_data_access(DirectoryId=<id>); poll ds.describe_directory_data_access until DataAccessStatus == 'Enabled'.
5. ds.create_log_subscription(DirectoryId=<id>, LogGroupName=<group>) -> 200; confirm ds.list_log_subscriptions(DirectoryId=<id>) reports it.
6. dsdata.create_user(DirectoryId=<id>, SAMAccountName='s2rLogSub1', GivenName='Sim', Surname='Real').
7. Poll logs.filter_log_events(logGroupName=<group>) (and logs.describe_log_streams) for several minutes: AWS yields a domain-controller security event naming 's2rLogSub1'; the emulator's group stays empty and has no streams.
8. ds.delete_log_subscription(DirectoryId=<id>), create another ds-data user, and confirm nothing new arrives (AWS: nothing arrives after the delete; the emulator cannot distinguish this from step 7).
9. Re-replay ds__logs.log-subscription-delivers-domain-controller-security-events-and-stops-when-deleted with a per-check budget above the check's own 300s lookback / 900s delivery windows.
