Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.event-data-store-lifecycle-ops-reject-unknown-arn
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.event-data-store-lifecycle-ops-reject-unknown-arn|_assert_error_map#8|AssertionError|assert actual == expected, f"error map mismatch: expected={expected} actual={actual}"
Issue-Commit: 58bb21045c140f54eac821540018fc7201e16d37

# UpdateEventDataStore's "no mutable field supplied" refusal uses a non-AWS message (and is not reported as AWS words it)

> _Note: this issue was identified against the code base at commit `58bb21045c140f54eac821540018fc7201e16d37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEventDataStore

## Description
There is 1 distinct cause in this issue; it must be closed before submitting.

Cause 1 of 1 — UpdateEventDataStore refuses a call that supplies only EventDataStore (no updatable field) with the right error code but the wrong message text.

AWS: Smithy UpdateEventDataStoreRequest has one required member, EventDataStore (EventDataStoreArn, @length 3..256, pattern ^[a-zA-Z0-9._/\-:]+$), and the optional updatable members Name, AdvancedEventSelectors, MultiRegionEnabled, OrganizationEnabled, RetentionPeriod, TerminationProtectionEnabled, KmsKeyId, BillingMode; the operation binds InvalidParameterException (errorKind client, httpError 400, awsQueryError InvalidParameter) and EventDataStoreNotFoundException (404, awsQueryError EventDataStoreNotFound). Recorded behavior: when ONLY EventDataStore is supplied, AWS refuses with HTTP 400 / Code 'InvalidParameterException' and the fixed message text 'At least one of the following must be provided: AdvancedEventSelectors, Name, RetentionPeriod, MultiRegionEnabled, TerminationProtectionEnabled, OrganizationEnabled, KmsKeyId'. The whole message is FIXED text — there are no request- or state-derived parts in it: the member list is the literal, ordered list AWS names (AdvancedEventSelectors, Name, RetentionPeriod, MultiRegionEnabled, TerminationProtectionEnabled, OrganizationEnabled, KmsKeyId), and no ARN, account, region or value from the request appears. The recording also shows this validation runs BEFORE the store is resolved: the ARN used in the check names no existing event data store, yet the refusal is InvalidParameterException rather than EventDataStoreNotFoundException (the emulator already gets that ordering right).

Emulator: same HTTP status and the same error code 'InvalidParameterException', but the message is its own wording — 'At least one optional parameter must be specified besides EventDataStore: Name, ...' (the replay detail is cut after 'Name,', so only the diverging prefix is visible; the fixed text 'At least one optional parameter must be specified besides EventDataStore: ' is already wrong and the member list that follows starts with Name rather than AdvancedEventSelectors). The sibling probes in the same check (GetEventDataStore, and per the check's expectation DeleteEventDataStore, RestoreEventDataStore, StartEventDataStoreIngestion, StopEventDataStoreIngestion, all expecting EventDataStoreNotFoundException with 'The event data store <arn> was not found.') are not implicated by the visible diff — GetEventDataStore matched exactly.

Why: the error message is part of the contract for this refusal class. A client, test fixture or CLI wrapper that matches on AWS's text (or a human reading the message to learn WHICH fields are updatable) gets different text and a different, incomplete field list — notably AdvancedEventSelectors is not even named first, so the message no longer enumerates the updatable set the way AWS documents it. Tooling that asserts the refusal message, and users diagnosing a no-op update, both diverge.

Fix: UpdateEventDataStore's input validation in the CloudTrail service crate (the handler's 'at least one updatable member' precondition, ops/update_event_data_store.rs and whatever shared validation helper builds its InvalidParameterException), which must emit AWS's fixed message verbatim, with the member list in AWS's order. Note the already-FIXED issue 20261023T135552Z-cloudtrail-refusal-fidelity-wrong-error-codes-for-selectors.md covered other CloudTrail refusal messages (trail, event-selector, Insights and PutEventConfiguration surfaces) but did NOT cover this UpdateEventDataStore precondition message, so this is a net-new divergence rather than a regression of that fix.

Traces: cloudtrail.event-data-store-lifecycle-ops-reject-unknown-arn.

## Evidence
Check cloudtrail.event-data-store-lifecycle-ops-reject-unknown-arn [mismatch]:
AssertionError: error map mismatch.

Expected (real AWS) for UpdateEventDataStore(EventDataStore=<well-formed but unknown eventdatastore ARN>):
  ('InvalidParameterException', 'At least one of the following must be provided: AdvancedEventSelectors, Name, RetentionPeriod, MultiRegionEnabled, TerminationProtectionEnabled, OrganizationEnabled, KmsKeyId')

Actual (emulator):
  ('InvalidParameterException', 'At least one optional parameter must be specified besides EventDataStore: Name, …')   [replay detail truncated after 'Name,']

Message anatomy: the entire expected message is FIXED text, including the member list and its order; nothing in it is derived from the request or from emulator state. The emulator's leading fixed text differs ('At least one optional parameter must be specified besides EventDataStore: ' vs 'At least one of the following must be provided: ') and its member list does not begin with AdvancedEventSelectors.

Control in the same check that PASSED: GetEventDataStore returned ('EventDataStoreNotFoundException', 'The event data store arn:aws:cloudtrail:us-west-2:<account>:eventdatastore/<uuid> was not found.') exactly as expected — so the error code and the validate-before-lookup ordering for UpdateEventDataStore are already correct; only the message text diverges.

Smithy (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/smithy-model/com-amazonaws-cloudtrail-cloudtrail-20131101.json): UpdateEventDataStore input = required EventDataStore + optional Name, AdvancedEventSelectors, MultiRegionEnabled, OrganizationEnabled, RetentionPeriod, TerminationProtectionEnabled, KmsKeyId, BillingMode; errors include InvalidParameterException (client, httpError 400, awsQueryError InvalidParameter) and EventDataStoreNotFoundException (404).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Replay check cloudtrail.event-data-store-lifecycle-ops-reject-unknown-arn.
3. Manually: fabricate a well-formed event-data-store ARN that names nothing, e.g. arn:aws:cloudtrail:<region>:<account>:eventdatastore/<random uuid>, then call cloudtrail UpdateEventDataStore with ONLY EventDataStore=<that arn> and no other member.
4. Inspect the ClientError: Code must be 'InvalidParameterException' (it is) and Message must be exactly 'At least one of the following must be provided: AdvancedEventSelectors, Name, RetentionPeriod, MultiRegionEnabled, TerminationProtectionEnabled, OrganizationEnabled, KmsKeyId'. The emulator instead returns 'At least one optional parameter must be specified besides EventDataStore: Name, …'.
5. For contrast, call GetEventDataStore / DeleteEventDataStore / RestoreEventDataStore / StartEventDataStoreIngestion / StopEventDataStoreIngestion with the same ARN: those must answer ('EventDataStoreNotFoundException', 'The event data store <arn> was not found.') — GetEventDataStore already does.
