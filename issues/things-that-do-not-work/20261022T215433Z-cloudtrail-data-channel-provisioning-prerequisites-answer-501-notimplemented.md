Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail-data.create-channel-nonexistent-event-data-store-rejected cloudtrail-data.e2e-audit-event-pipeline-blocked-for-new-account
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data.create-channel-nonexistent-event-data-store-rejected|run#5|AssertionError|assert malformed_code == "ValidationException", (
Issue-Key: cloudtrail-data.e2e-audit-event-pipeline-blocked-for-new-account|run#5|AssertionError|assert eds_code == "InvalidParameterException" or eds_arn is not None, (
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# cloudtrail-data channel provisioning prerequisites answer 501 NotImplemented: CreateEventDataStore and CreateChannel never reach their modelled rejections (2 causes)

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Two cloudtrail-data behavioral checks walk the documented custom-events-into-CloudTrail-Lake pipeline, whose first two steps are served by the CloudTrail control plane (crate services/service-com-amazonaws-cloudtrail-cloudtrail-20131101). Both steps answer 501 NotImplemented, so the pipeline's step-specific rejections are never produced. The checks' ids lead with `cloudtrail-data`, which is why this issue carries that service token; the handlers to fix live in the cloudtrail crate's ops directory. No open issue covers either operation (the only cloudtrail/cloudtrail-data issues on file are the two resolved bootstrap issues).

=== Cause 1 of 2: CreateEventDataStore answers NotImplemented ===
AWS: CreateEventDataStore either creates the event data store and returns EventDataStoreArn/Name/Status, or — on an account for which CloudTrail Lake is closed to new customers, which is the recorded behavior here — refuses with error code InvalidParameterException and HTTP 400 (Smithy: com.amazonaws.cloudtrail#CreateEventDataStore errors include InvalidParameterException, errorKind client, httpError 400; input Name @length 3..128 @pattern ^[a-zA-Z0-9._\-]+$, RetentionPeriod @range 7..3653). The recorded refusal message ("CloudTrail Lake is no longer accepting new customers...") is an example, not a requirement; the code and status are.
Emulator: 'NotImplemented' — 'operation amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateEventDataStore is not implemented in this emulator'.
Why: a client cannot tell whether it has a Lake-capable account; an unmodelled NotImplemented is not in the operation's error set, so error handling for either outcome (created, or InvalidParameterException) never runs.
Fix: the CreateEventDataStore handler in the cloudtrail service crate (its ops module) plus the event-data-store registry in that crate's state.
Traces: cloudtrail-data.e2e-audit-event-pipeline-blocked-for-new-account.

=== Cause 2 of 2: CreateChannel answers NotImplemented, so neither its malformed-Location nor its missing-destination rejection exists ===
AWS: CreateChannel validates Destinations[].Location and distinguishes two refusal classes. A Location that violates the member's character-set/ARN form is refused with ValidationException/400; a well-formed but never-created event-data-store ARN is refused with error code EventDataStoreNotFoundException and HTTP 404 (Smithy: com.amazonaws.cloudtrail#CreateChannel errors include EventDataStoreNotFoundException, errorKind client, httpError 404, and EventDataStoreARNInvalidException/InvalidParameterException at 400; input Name @length 3..128 @pattern ^[a-zA-Z0-9._\-]+$, Destinations @length 1..200, required). In both cases no channel is created and ListChannels does not list the name. The recorded not-found message 'The event data store <arn> was not found.' is request-derived (it interpolates the Location given), so that template is the rule.
Emulator: 'NotImplemented' — 'operation amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateChannel is not implemented in this emulator' for every CreateChannel call.
Why: no channel can ever be provisioned in the emulator, which is also why every cloudtrail-data PutAuditEvents path is only reachable against an absent channel; and a client exercising the documented failure classes sees one undifferentiated 501 instead of 400-vs-404.
Fix: the CreateChannel handler in the cloudtrail service crate (its ops module): destination validation, the event-data-store existence lookup, and channel registration (which is also what would let the cloudtrail-data crate's channel registry in services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/src/state.rs ever hold a channel).
Traces: cloudtrail-data.create-channel-nonexistent-event-data-store-rejected, cloudtrail-data.e2e-audit-event-pipeline-blocked-for-new-account.

## Evidence
From the behavioral replay against the emulator:

Cause 1 — check cloudtrail-data.e2e-audit-event-pipeline-blocked-for-new-account, step 1: create_event_data_store(Name=..., AdvancedEventSelectors=[{Name:'select-activity-audit-log', FieldSelectors:[{Field:'eventCategory', Equals:['ActivityAuditLog']}]}], MultiRegionEnabled=False, RetentionPeriod=7, TerminationProtectionEnabled=False, TagsList=[...]). Expected either a created EventDataStoreArn or Error.Code 'InvalidParameterException' (HTTP 400). Got Error.Code 'NotImplemented' — AssertionError: "CreateEventDataStore failed for an unexpected reason: 'NotImplemented'".

Cause 2 — check cloudtrail-data.create-channel-nonexistent-event-data-store-rejected: create_channel(Name='sim2real-badch-ctl-...', Source='Custom', Destinations=[{Type:'EVENT_DATA_STORE', Location:'not a valid arn!!'}]). Expected Error.Code 'ValidationException'; got 'NotImplemented' with message 'operation amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateChannel is not implemented in this emulator'. The same check's second arm expects Error.Code 'EventDataStoreNotFoundException' (HTTP 404) for Location='arn:aws:cloudtrail:us-west-2:<account>:eventdatastore/<uuid>' and is never reached.

## Steps to Reproduce
Boot the emulator and point a botocore cloudtrail client at it in us-west-2.
1. Cause 1: cloudtrail.create_event_data_store(Name='sim2real-cted-x', AdvancedEventSelectors=[{'Name':'select-activity-audit-log','FieldSelectors':[{'Field':'eventCategory','Equals':['ActivityAuditLog']}]}], MultiRegionEnabled=False, RetentionPeriod=7, TerminationProtectionEnabled=False) — expect a created store or InvalidParameterException/400; observe NotImplemented/501.
2. Cause 2a: cloudtrail.create_channel(Name='sim2real-badch', Source='Custom', Destinations=[{'Type':'EVENT_DATA_STORE','Location':'not a valid arn!!'}]) — expect ValidationException/400; observe NotImplemented/501.
3. Cause 2b: cloudtrail.create_channel(Name='sim2real-badch2', Source='Custom', Destinations=[{'Type':'EVENT_DATA_STORE','Location':'arn:aws:cloudtrail:us-west-2:<account>:eventdatastore/<uuid>'}]) — expect EventDataStoreNotFoundException/404 with 'The event data store <that arn> was not found.'; observe NotImplemented/501. Then cloudtrail.list_channels() must not list either name.
