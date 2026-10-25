Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.add-source-identifier-to-subscription
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.add-source-identifier-to-subscription|run#17|AssertionError|assert code == ERR_SOURCE_NOT_FOUND, f"unexpected error code: {code!r}"
Issue-Commit: 79958e9ac1a4e2804b5bffcb8136e3ce17c50800

# RDS AddSourceIdentifierToSubscription is an unimplemented scaffold stub, so an event subscription's SourceIdsList can never be managed and the SourceNotFound refusal never fires

> _Note: this issue was identified against the code base at commit `79958e9ac1a4e2804b5bffcb8136e3ce17c50800`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddSourceIdentifierToSubscription

## Description
Cause 1 of 1 — RDS `AddSourceIdentifierToSubscription` answers `NotImplemented` for every call, so neither its success path nor its modeled not-found refusal can be observed.

NOTE FOR THE FIXER: this run also auto-filed a stub issue naming the SAME operation and the SAME handler file (it was raised from the blocked check `rds.event-subscription-source-management`, which errored on the same 501). The two are ONE cause — implementing the handler described below closes both; close them together rather than treating them as separate work.

AWS: Smithy `com.amazonaws.rds#AmazonRDSv19` models `AddSourceIdentifierToSubscription` with required input members `SubscriptionName` (String) and `SourceIdentifier` (String), output `AddSourceIdentifierToSubscriptionResult{EventSubscription}`, and exactly two modeled client errors — `SourceNotFoundFault` (errorKind client, httpError 404, awsQueryError code `SourceNotFound`, httpResponseCode 404) and `SubscriptionNotFoundFault` (404, awsQueryError code `SubscriptionNotFound`). Per the model's own documentation of `SourceIdentifier`, the identifier must match the subscription's SourceType (for SourceType `db-parameter-group` a `DBParameterGroupName`), so a SourceIdentifier naming no existing resource of that type is refused with `SourceNotFound` (404). On success the returned `EventSubscription` carries the added identifier in `SourceIdsList`, and a subsequent `DescribeEventSubscriptions` for the same SubscriptionName reports it there too.

Emulator: every call is refused with error code `NotImplemented` and the body "operation AddSourceIdentifierToSubscription is scaffolded but not implemented in this emulator". `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/add_source_identifier_to_subscription.rs` still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("AddSourceIdentifierToSubscription"))`. `NotImplemented` is not a modeled error for this operation and appears in no RDS API reference.

Why: a client cannot attach a source to an RDS event-notification subscription at all, so a subscription created with a SourceType can never be narrowed to specific sources and keeps matching every source of that type. Error handling is wrong in both directions: code that branches on `SourceNotFound` (404) to report a bad source identifier never sees it, and code that treats an unmodeled error as retryable/server-side will retry a call that can never succeed.

Fix: implement the `AddSourceIdentifierToSubscription` handler in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/add_source_identifier_to_subscription.rs` — resolve `SubscriptionName` against the event-subscription records in the RDS state module (absent -> `SubscriptionNotFoundFault`), resolve `SourceIdentifier` against the resource family named by that subscription's SourceType (absent -> `SourceNotFoundFault`), append the identifier to the subscription's `SourceIdsList` in state, and return the updated `EventSubscription` so `DescribeEventSubscriptions` reflects it. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line once implemented.

Traces: rds.add-source-identifier-to-subscription.

## Evidence
Failing check `rds.add-source-identifier-to-subscription` (verdict: mismatch):

  File "check_rds_add_source_identifier_to_subscription.py", line 80, in run
    assert code == ERR_SOURCE_NOT_FOUND, f"unexpected error code: {code!r}"
  AssertionError: unexpected error code: 'NotImplemented'

Expected (AWS): calling AddSourceIdentifierToSubscription with a SourceIdentifier naming no existing DB parameter group is refused with HTTP 404 and awsQuery error code `SourceNotFound`; calling it with an existing DB parameter group name returns HTTP 200 with EventSubscription.SourceIdsList containing that name, and DescribeEventSubscriptions then reports SourceIdsList == [that name].
Actual (emulator): both calls are refused with error code `NotImplemented` ("operation AddSourceIdentifierToSubscription is scaffolded but not implemented in this emulator"), so the success leg and the SourceIdsList round trip are never reached.

Source confirmation: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/add_source_identifier_to_subscription.rs lines 3-6 (SCAFFOLDED marker) and line 162 (`Err(crate::scaffolded::Scaffolded::not_implemented("AddSourceIdentifierToSubscription"))`). Modeled errors declared at lines 94-97 are `SourceNotFoundFault` and `SubscriptionNotFoundFault`; `NotImplemented` is neither.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay `rds.add-source-identifier-to-subscription`.
2. Or by hand against the emulator's RDS endpoint:
   a. SNS CreateTopic to get a topic ARN.
   b. RDS `Action=CreateEventSubscription` with SubscriptionName=s, SnsTopicArn=<topic>, SourceType=db-parameter-group, Enabled=true; poll DescribeEventSubscriptions until Status=active.
   c. RDS `Action=CreateDBParameterGroup` with a mysql family to create a real source, name it g.
   d. `Action=AddSourceIdentifierToSubscription&SubscriptionName=s&SourceIdentifier=<name-that-does-not-exist>` -> observed error code `NotImplemented`; expected HTTP 404 with code `SourceNotFound`.
   e. `Action=AddSourceIdentifierToSubscription&SubscriptionName=s&SourceIdentifier=g` -> observed error code `NotImplemented`; expected HTTP 200 with EventSubscription whose SourceIdsList == [g], and DescribeEventSubscriptions(SubscriptionName=s) reporting the same.
