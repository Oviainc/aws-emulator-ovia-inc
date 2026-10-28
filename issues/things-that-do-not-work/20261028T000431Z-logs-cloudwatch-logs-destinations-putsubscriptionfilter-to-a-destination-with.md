Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.destination-policy-enforces-cross-account-subscription logs.e2e-log-group-subscription-destination-delivery-flow
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.destination-policy-enforces-cross-account-subscription|_error#24|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: logs.e2e-log-group-subscription-destination-delivery-flow|_poll_kinesis_for_envelope#34|Timeout|time.sleep(2.0)
Issue-Commit: 89b9ca4b685f6ae9a714db90fb8f95ad9a86cf49

# CloudWatch Logs destinations: PutSubscriptionFilter to a destination with NO access policy is now accepted with 200 instead of refused, and delivery through a logical destination produces no record at all (2 causes)

> _Note: this issue was identified against the code base at commit `89b9ca4b685f6ae9a714db90fb8f95ad9a86cf49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Two net-new root causes in the CloudWatch Logs (`logs`) destination / subscription-delivery surface. Neither failing check is marked ALREADY FILED or OWNED in the replay report. Cause count: 2 — each is numbered below with its own failing check ids, and BOTH must be closed before this issue is submitted.

SERVICE ATTRIBUTION: both checks lead with the `logs.` token and the diverging operations (`logs:PutSubscriptionFilter`, `logs:PutDestination`) are CloudWatch Logs operations, so this issue is filed against `logs`. Kinesis and IAM only supply the destination stream and the delivery role.

RELATED OPEN / FIXED ISSUES (none of them carries either of these replay keys):
- OPEN 20261025T014519Z-logs-cloudwatch-logs-vended-delivery-and-subscription-plumbing-is.md records, as its cause 4, that this same call answered 400 InvalidParameterException where AWS answers AccessDeniedException. That is NOT what the emulator does now: the call no longer refuses at all, it answers HTTP 200 and creates the filter. Cause 1 below is that different, strictly worse behaviour — the authorization boundary has disappeared rather than been misclassified.
- FIXED 20261025T100129Z-logs-cloudwatch-logs-subscription-destination-plumbing-a-logical-destination.md, cause 1, was that PutSubscriptionFilter refused a logical-destination ARN without roleArn. That refusal is gone (the check now gets past it), so this issue does NOT reopen it; cause 2 below is the next, independent gap the same check reaches.
- OPEN 20261026T072730Z-logs-cloudwatch-logs-subscription-filter-delivery-produces-nothing.md and OPEN 20261026T102156Z-kinesis-logs-a-cloudwatch-logs-subscription-filter-delivers.md describe the same symptom class for a DIRECT Kinesis destinationArn. Cause 2 is the logical-destination (PutDestination + PutDestinationPolicy) indirection path, and neither issue carries this check id.

=== Cause 1 of 2: PutSubscriptionFilter against a CloudWatch Logs destination that has no access policy is accepted (HTTP 200) instead of refused ===
Failing check: logs.destination-policy-enforces-cross-account-subscription
AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) PutSubscriptionFilter requires logGroupName, filterName, filterPattern and destinationArn and returns Unit (an empty 200); its modeled client errors are InvalidOperationException, InvalidParameterException, LimitExceededException, OperationAbortedException and ResourceNotFoundException (all 400). AccessDeniedException is NOT in that modeled set, so the refusal is recorded behavior only. A CloudWatch Logs destination created by PutDestination is unusable until an access policy is attached by PutDestinationPolicy: with no policy attached, even the destination-OWNING account's PutSubscriptionFilter against that destination ARN is refused with error code AccessDeniedException. The message is a template whose fixed text is "User with accountId: ", " is not authorized to perform: logs:PutSubscriptionFilter on resource: " — the variable parts are the caller's account id (from the credentials/STS identity) and the destination ARN taken from the request's destinationArn. Once ANY policy is attached the owning account's identical call succeeds, and a policy naming only a foreign account does not revoke the owner.
Emulator: the call answers HTTP 200 with an empty body (content-type application/x-amz-json-1.1, content-length 2) and creates the subscription filter. No authorization check runs on the destination at all.
Why: the destination access policy is the sole authorization mechanism for a CloudWatch Logs destination, and it is now unenforced. A client (or IaC template) that forgets PutDestinationPolicy is told its cross-account subscription is wired when against AWS it is a hard refusal, so the client's AccessDeniedException handling — the signal that says "attach a destination policy" — never runs, and an unauthorized subscription is silently created. The emulator also no longer distinguishes authorized from unauthorized destinations in any observable way.
Fix: the destination-authorization branch of the PutSubscriptionFilter handler in services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_subscription_filter.rs — when destinationArn names a `destination:` resource, resolve it, evaluate its stored accessPolicy against the caller, and refuse with AccessDeniedException (and the message above) when no policy is attached or no statement authorizes logs:PutSubscriptionFilter for the caller. The denied call must leave no filter behind (DescribeSubscriptionFilters on the group must not list it).
Traces: logs.destination-policy-enforces-cross-account-subscription.

=== Cause 2 of 2: a subscription filter pointed at a logical destination delivers nothing — no record ever reaches the destination's Kinesis stream ===
Failing check: logs.e2e-log-group-subscription-destination-delivery-flow
AWS: recorded behavior only (PutSubscriptionFilter returns Unit, so the model says nothing about the delivered payload; PutDestination returns a Destination carrying the destination arn). For the documented pipeline PutDestination(targetArn=<kinesis stream>, roleArn=<role trusted by logs.amazonaws.com>) -> PutDestinationPolicy(accessPolicy granting this account logs:PutSubscriptionFilter) -> PutSubscriptionFilter(destinationArn=<the destination ARN>), a subsequent PutLogEvents on the subscribed group causes CloudWatch Logs to write a gzip-compressed record to the destination's target Kinesis stream whose JSON envelope has messageType "DATA_MESSAGE", owner equal to the caller's account id, logGroup and logStream equal to the source group and stream names from the request, subscriptionFilters containing exactly the matching filter's name, and logEvents carrying the written message with its own millisecond timestamp. All of those values come from the request and the emulator's state; none is a per-run constant.
Emulator: PutDestination, PutDestinationPolicy and PutSubscriptionFilter all answer 200 (the roleArn-less logical-destination refusal that previously stopped this check here is gone), PutLogEvents is accepted, and then the destination's Kinesis stream stays empty: the check's 120 s TRIM_HORIZON GetShardIterator/GetRecords poll for an envelope containing the written marker never sees a record and the check is killed at the harness cap (`Error: Check timed out after 60.0s`). The Kinesis read path answers in this checkout (GetShardIterator/GetRecords are implemented and no check reports NotImplemented for them), so the reads succeed and simply return nothing.
Why: the whole point of a logical destination is cross-account log subscription, and against the emulator it is a wire-level fiction — accepted with a 200 and then permanently silent. Any consumer of the destination stream blocks forever instead of receiving data or an error, and every downstream contract on the envelope (owner / logGroup / logStream / subscriptionFilters identity, and the messageType) is ungraded because no record is ever produced.
Fix: the subscription-filter delivery path in the cloudwatch-logs crate must resolve a `destination:` destinationArn to its stored targetArn and deliver through it: on the ingest path in services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_log_events.rs, evaluate each stored event against every live filter on the group and, for a filter whose destination is a logical destination, put one gzip DATA_MESSAGE envelope per matching filter into the Kinesis shard state named by that destination's targetArn, stamping the source group/stream, the caller's account as owner, the matching filter's name and the stored event's own timestamp. The destination lookup belongs with the destination state written by services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_destination.rs.
Traces: logs.e2e-log-group-subscription-destination-delivery-flow.

## Evidence
Cause 1 — logs.destination-policy-enforces-cross-account-subscription (verdict: mismatch):
  File "check_logs_destination_policy_enforces_cross_account_subscription.py", line 58, in run -> line 190, in _error
  AssertionError: expected a rejection, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', 'date': '...'}, 'RetryAttempts': 0}}
  Setup established by the same run: kinesis CreateStream (polled ACTIVE) -> IAM role trusted by logs.amazonaws.com with kinesis:PutRecord/PutRecords -> logs PutDestination(destinationName, targetArn=<stream arn>, roleArn=<role>) succeeded, and DescribeDestinations(DestinationNamePrefix=<name>) confirmed the fresh destination carries NO accessPolicy. The call under test is logs.put_subscription_filter(logGroupName=<own group>, filterName='sim2real-owner-filter', filterPattern='', destinationArn=<the destination arn>) with no destination policy attached.
  Expected: error code AccessDeniedException with message "User with accountId: <caller account id> is not authorized to perform: logs:PutSubscriptionFilter on resource: <destination arn>" (fixed text as quoted; account id and destination ARN are request/state values), and DescribeSubscriptionFilters on the group must not list 'sim2real-owner-filter'.
  Actual: HTTP 200, empty body, and the filter is created. (The previously filed behaviour for this same call — 400 InvalidParameterException, open issue 20261025T014519Z cause 4 — no longer occurs; the refusal is now absent entirely, which is why that issue's replay key no longer matches.)

Cause 2 — logs.e2e-log-group-subscription-destination-delivery-flow (verdict: mismatch):
  Error: Check timed out after 60.0s
  Sequence exercised: logs CreateLogGroup + CreateLogStream; kinesis CreateStream(ShardCount=1) polled ACTIVE; iam CreateRole (trust logs.amazonaws.com) + PutRolePolicy (kinesis:PutRecord/PutRecords on the stream arn); logs PutDestination(destinationName, targetArn=<stream arn>, roleArn=<role>) -> returns destination.arn; logs PutDestinationPolicy(accessPolicy allowing {"AWS": <account id>} logs:PutSubscriptionFilter on that arn); logs PutSubscriptionFilter(logGroupName, filterName, filterPattern='', destinationArn=<destination arn>) with roleArn omitted -> accepted; logs PutLogEvents(one event whose message is a unique marker).
  The check then polls kinesis GetShardIterator(TRIM_HORIZON) + GetRecords for up to 120 s looking for a gzip record whose decompressed JSON has a logEvent with message == marker.
  Expected: one such record, with messageType 'DATA_MESSAGE', owner == the STS caller account id, logGroup == the created log group name, logStream == the created log stream name, and subscriptionFilters containing the created filter name.
  Actual: no record of any kind ever appears on the shard; the poll runs until the 60 s harness cap kills the check. The reads themselves succeed (kinesis GetShardIterator/GetRecords are implemented in this checkout and no check in this run reports NotImplemented for them) — the stream is simply empty.

## Steps to Reproduce
Cause 1:
  1. kinesis CreateStream(StreamName=S, ShardCount=1); poll DescribeStream until ACTIVE.
  2. iam CreateRole(R) with a trust policy for logs.amazonaws.com; iam PutRolePolicy(R, allow kinesis:PutRecord/PutRecords on the stream ARN).
  3. logs PutDestination(destinationName=D, targetArn=<stream arn>, roleArn=<R arn>) -> note destination.arn.
  4. logs DescribeDestinations(DestinationNamePrefix=D) -> confirm the destination has NO accessPolicy.
  5. logs CreateLogGroup(G).
  6. logs PutSubscriptionFilter(logGroupName=G, filterName=F, filterPattern='', destinationArn=<destination arn>) with NO PutDestinationPolicy ever called.
     -> observe HTTP 200 with an empty body and the filter created.
     -> expected error code AccessDeniedException, message "User with accountId: <account id> is not authorized to perform: logs:PutSubscriptionFilter on resource: <destination arn>", and logs DescribeSubscriptionFilters(logGroupName=G) must not contain F.
  7. Positive control: logs PutDestinationPolicy(destinationName=D, accessPolicy allowing {"AWS": <account id>} logs:PutSubscriptionFilter on the destination arn), then repeat step 6 -> must succeed, and the policy must round-trip verbatim through DescribeDestinations. Then replace the policy with one naming only foreign account 111122223333 and re-create F -> must still succeed (a foreign-only policy does not revoke the owner).

Cause 2:
  1..3 as above, then logs PutDestinationPolicy(destinationName=D, accessPolicy allowing {"AWS": <account id>} logs:PutSubscriptionFilter on the destination arn).
  4. logs CreateLogGroup(G) + CreateLogStream(G, LS).
  5. logs PutSubscriptionFilter(logGroupName=G, filterName=F, filterPattern='', destinationArn=<destination arn>), roleArn omitted -> accepted (200).
  6. logs PutLogEvents(G, LS, [{timestamp: now_ms, message: <unique marker>}]).
  7. kinesis GetShardIterator(StreamName=S, ShardId=<shard 0>, ShardIteratorType=TRIM_HORIZON) + GetRecords, draining the shard for up to 120 s.
     -> observe no records at all; the check hangs to the harness cap.
     -> expected one gzip record whose JSON envelope has messageType 'DATA_MESSAGE', owner = the caller's account id, logGroup = G, logStream = LS, subscriptionFilters = [F], and one logEvent with the marker and its written millisecond timestamp.
