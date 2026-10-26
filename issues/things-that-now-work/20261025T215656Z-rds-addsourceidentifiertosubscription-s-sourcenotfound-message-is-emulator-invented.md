Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.event-subscription-source-management
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.event-subscription-source-management|run#27|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: e7a2f2386a844500e403d56aecba07370542f5da

# RDS AddSourceIdentifierToSubscription's SourceNotFound message is emulator-invented ("<source-type> <id> not found in <region>") instead of AWS's "Could not find source :<SourceIdentifier>"

> _Note: this issue was identified against the code base at commit `e7a2f2386a844500e403d56aecba07370542f5da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddSourceIdentifierToSubscription

## Description
REOPENS 20261025T074531Z-rds-addsourceidentifiertosubscription-is-an-unimplemented-scaffold-stub-so.md: its fix left the call refused. The handler is now implemented and the refusal CLASS it produces is right — HTTP 404 with awsQuery error code `SourceNotFound` — but the refusal's message is the emulator's own prose rather than AWS's, so the one remaining observable of that fix still diverges.

Cause 1 of 1: the `SourceNotFoundFault` message emitted when `SourceIdentifier` names no resource of the subscription's `SourceType` is composed from the emulator's own template.

AWS: Smithy `com.amazonaws.rds#AmazonRDSv19` models `AddSourceIdentifierToSubscription` with required input members `SubscriptionName` and `SourceIdentifier` and exactly two client errors — `SourceNotFoundFault` (errorKind client, httpError 404, awsQueryError code `SourceNotFound`, httpResponseCode 404) and `SubscriptionNotFoundFault` (404, awsQueryError code `SubscriptionNotFound`). The model documents `SourceNotFoundFault` only as "The requested source could not be found" and documents `SourceIdentifier` as having to be a value of the subscription's source type (a `DBParameterGroupName` for SourceType `db-parameter-group`), so the message TEXT is recorded behavior only. Recorded behavior (replayed against real RDS and re-confirmed by this check, see the check's own comment at check_rds_event_subscription_source_management.py): the refusal is 404 / `SourceNotFound` with the message `Could not find source :<SourceIdentifier>`. Its fixed text is `Could not find source :` (note: a space before the colon, nothing between the colon and the identifier — RDS does not interpolate the source type there and does not append a period); its only variable part is `<SourceIdentifier>`, taken verbatim from the request. No region, no source type, no resource-kind word appears in it.

Emulator: the same call is refused 404 `SourceNotFound` but with the message `db-parameter-group sim2real-behav-does-not-exist not found in us-west-2` — i.e. the template `"{source_type} {source_identifier} not found in {region}"`: it prepends the subscription's SourceType, replaces AWS's fixed text with `not found in`, and appends the signing region, which AWS never includes.

Why: a client that matches on, logs, or surfaces RDS's not-found text for an event-subscription source sees text no AWS error ever carries. Parsers keyed on the documented/recorded `Could not find source :` prefix (and tooling that extracts the offending identifier as the suffix after it) fail to classify the error, and operators comparing emulator output with AWS runbook text get a different diagnosis string for the same condition. Because the error code is correct, the divergence is silent until the message is read.

Fix: the refusal construction in the `AddSourceIdentifierToSubscription` handler — the `SourceNotFoundFault { message: .. }` it builds on the "identifier is not among the stored sources of this source type" branch. It must emit the fixed text `Could not find source :` followed by the request's `SourceIdentifier`, with no source type and no region interpolated; the resolution logic itself (and the 404/`SourceNotFound` classification, and the fact that the rejected call must not mutate `SourceIdsList`) is already correct and must stay. If other RDS handlers construct this same fault, the message belongs in one shared constructor so the copies cannot drift.

Traces: rds.event-subscription-source-management (the subscription create -> add source -> remove source -> modify -> negative add -> delete walk; every leg before the negative add passes, so only this message diverges).

## Evidence
Failing check `rds.event-subscription-source-management` (verdict: mismatch):

  File "check_rds_event_subscription_source_management.py", line 77, in run
    assert message == expected, f"expected message {expected!r}, got {message!r}"
  AssertionError: expected message 'Could not find source :sim2real-behav-does-not-exist', got 'db-parameter-group sim2real-behav-does-not-exist not found in us-west-2'

Expected (AWS): AddSourceIdentifierToSubscription on an active `db-parameter-group` subscription with a SourceIdentifier naming no DB parameter group -> HTTP 404, awsQuery error code `SourceNotFound`, message `Could not find source :<SourceIdentifier>` (fixed text `Could not find source :`; `<SourceIdentifier>` copied from the request).
Actual (emulator): HTTP 404, error code `SourceNotFound` (correct), message `<SourceType> <SourceIdentifier> not found in <region>` — e.g. `db-parameter-group sim2real-behav-does-not-exist not found in us-west-2`.

The preceding legs of the check all pass (create with SourceIds=[pg_a], add pg_b, remove pg_a, ModifyEventSubscription(Enabled=false)), and the error CODE assertion (`code == "SourceNotFound"`, one line earlier) passes, so the divergence is confined to the message text.

Source confirmation: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/add_source_identifier_to_subscription.rs lines 205-218 build `SourceNotFoundFault` with `message: Some(format!("{source_type} {source_identifier} not found in {region}"))`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay `rds.event-subscription-source-management`.
2. Or by hand against the emulator's RDS endpoint (query protocol):
   a. SNS `CreateTopic` to obtain a topic ARN.
   b. RDS `Action=CreateDBParameterGroup&DBParameterGroupName=pg-a&DBParameterGroupFamily=mysql8.0&Description=x`.
   c. RDS `Action=CreateEventSubscription&SubscriptionName=sub-1&SnsTopicArn=<topic>&SourceType=db-parameter-group&SourceIds.member.1=pg-a&Enabled=true`; poll `DescribeEventSubscriptions` until Status=active.
   d. RDS `Action=AddSourceIdentifierToSubscription&SubscriptionName=sub-1&SourceIdentifier=does-not-exist`.
   e. Observe: HTTP 404, Code `SourceNotFound`, Message `db-parameter-group does-not-exist not found in us-west-2`. Expected Message: `Could not find source :does-not-exist`.
   f. `DescribeEventSubscriptions(SubscriptionName=sub-1)` still reports SourceIdsList == [pg-a] (correct — the rejected add must not mutate it).
