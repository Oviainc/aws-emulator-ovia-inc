Issue-Source: trace-sweep-divergence
Issue-Trace: logs-b2-delivery-source-destination-pipeline
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-b2-delivery-source-destination-pipeline logs-delivery-source-crud-lifecycle logs-delivery-destination-and-cross-account-policy-lifecycle logs-get-delivery-destination-policy-returns-empty-when-unset logs-b2-lookup-tables-and-integrations logs-lookup-table-describe-pagination logs-lookup-table-crud-lifecycle logs-lookup-table-duplicate-create-conflict logs-lookup-table-duplicate-create-error logs-lookup-table-error-paths logs-lookup-table-errors logs-lookup-table-lifecycle logs-lookup-table-tags-and-description-only-update logs-b2-transformer-configuration
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|PutDeliverySource|4xx->2xx|$.__type
Issue-Key: logs|PutDeliverySource|4xx->2xx|$.message
Issue-Key: logs|PutDeliverySource|4xx->2xx|status
Issue-Key: logs|DeleteDeliveryDestinationPolicy|4xx->2xx|$
Issue-Key: logs|DeleteDeliveryDestinationPolicy|4xx->2xx|status
Issue-Key: logs|GetDeliverySource|4xx->2xx|$.__type
Issue-Key: logs|GetDeliverySource|4xx->2xx|$.message
Issue-Key: logs|GetDeliverySource|4xx->2xx|status
Issue-Key: logs|DeleteDeliverySource|4xx->2xx|$
Issue-Key: logs|DeleteDeliverySource|4xx->2xx|status
Issue-Key: logs|PutDeliverySource|4xx->4xx|$.message
Issue-Key: logs|GetDeliveryDestinationPolicy|2xx->2xx|$.policy.deliveryDestinationPolicy
Issue-Key: logs|DescribeLookupTables|4xx->2xx|$.__type
Issue-Key: logs|DescribeLookupTables|4xx->2xx|$.message
Issue-Key: logs|DescribeLookupTables|4xx->2xx|status
Issue-Key: logs|CreateLookupTable|4xx->4xx|$.message
Issue-Key: logs|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: logs|ListTagsForResource|2xx->4xx|status
Issue-Key: logs|GetTransformer|2xx->4xx|$.transformerConfig
Issue-Key: logs|GetTransformer|2xx->4xx|status
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# logs delivery-source, lookup-table and transformer reads succeed on absent resources (10 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeliverySource

## Description
Ten root causes on the delivery-source, lookup-table and transformer surfaces. The shared fault is that operations naming an absent or invalid resource are answered as successes (or with the wrong status/text) instead of the modeled refusal, and one read path re-serializes a stored policy document instead of returning it verbatim. Cause count: 10 — all ten must be closed.

=== Cause 1 of 10: PutDeliverySource accepts an unsupported logType ===
AWS: refuses with HTTP 400 ValidationException whose message is 'Provided LogType is not valid. Supported options are [INSIGHTS_QUERY_LOGS].' — fixed text whose variable part is the list of log types supported for the named resourceArn's service. Smithy: PutDeliverySource.logType is required (@length 1..255, @pattern '^[\\w]*$') and the operation binds ValidationException (httpError 400); which values are legal per resource type is recorded behavior.
Emulator: answers HTTP 200 and creates the delivery source.
Why: a client learns the supported log types from this refusal; instead it gets a delivery source AWS would never have created and a later CreateDelivery against it diverges.
Fix: the logType validation branch of the PutDeliverySource handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-b2-delivery-source-destination-pipeline#task#11.

=== Cause 2 of 10: GetDeliverySource on an absent name answers 200 ===
AWS: HTTP 400 ResourceNotFoundException with the fixed text 'Requested Delivery Source does not exist in this account.' Smithy: GetDeliverySource binds ResourceNotFoundException (httpError 400).
Emulator: HTTP 200 with a body.
Why: a client cannot tell an existing delivery source from a missing one, so create-if-absent logic never creates.
Fix: the existence check in the GetDeliverySource handler.
Traces: logs-b2-delivery-source-destination-pipeline#task#12.

=== Cause 3 of 10: DeleteDeliverySource on an absent name answers 200 ===
AWS: HTTP 400 ResourceNotFoundException with 'Requested Delivery Source does not exist in this account.' Smithy: DeleteDeliverySource binds ResourceNotFoundException (httpError 400) and has no output members.
Emulator: HTTP 200 with an empty body.
Why: delete is silently idempotent in the emulator but not on AWS, so teardown logic that asserts the refusal never fires.
Fix: the existence check in the DeleteDeliverySource handler.
Traces: logs-b2-delivery-source-destination-pipeline#task#13.

=== Cause 4 of 10: DeleteDeliveryDestinationPolicy with no policy set answers 200 ===
AWS: HTTP 400 ResourceNotFoundException with 'Delivery Destination Policy does not exist for this Delivery Destination.' Smithy: DeleteDeliveryDestinationPolicy binds ResourceNotFoundException (httpError 400).
Emulator: HTTP 200 with an empty body.
Why: same idempotence divergence on the cross-account policy surface.
Fix: the existence check in the DeleteDeliveryDestinationPolicy handler.
Traces: logs-b2-delivery-source-destination-pipeline#task#10.

=== Cause 5 of 10: PutDeliverySource's name-pattern refusal echoes the ANCHORED model regex ===
AWS: the standard constraint message is "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; the variable parts are the member name and the pattern, and AWS renders the pattern WITHOUT the ^...$ anchors — '[\\w-]*' for `name`. Smithy: DeliverySourceName @pattern '^[\\w-]*$'.
Emulator: echoes the anchored trait string '^[\\w-]*$'.
Why: a user copying the regex out of the message to fix their input gets a pattern AWS never emits; assertions on the message fail. This is the same class of defect as cause 4 of the open behavioral issue 20261029T182816Z-logs-ten-cloudwatch-logs-refusal-messages-are-emulator.md, which covers logGroupName/logStreamName/logGroupNamePattern but not DeliverySourceName.
Fix: the shared request-validation message renderer — strip the anchors.
Traces: logs-delivery-source-crud-lifecycle#task#5.

=== Cause 6 of 10: GetDeliveryDestinationPolicy re-serializes the stored policy JSON ===
AWS: returns the deliveryDestinationPolicy string exactly as it was supplied to PutDeliveryDestinationPolicy — it is an opaque string member (Smithy: Policy.deliveryDestinationPolicy, @length 1..51200), not a parsed document.
Emulator: returns a re-serialized form with spaces inserted after ':' and ',', so the string differs byte-for-byte from what was stored.
Why: clients that hash, diff or signature-compare the policy document to detect drift see a change on every round trip.
Fix: store and return the policy string verbatim in the PutDeliveryDestinationPolicy/GetDeliveryDestinationPolicy state path (no parse/re-emit).
Traces: logs-delivery-destination-and-cross-account-policy-lifecycle#task#6.

=== Cause 7 of 10: DescribeLookupTables with a prefix that matches nothing answers 200 ===
AWS: HTTP 400 ResourceNotFoundException with "No lookup tables found for accountId '<accountId>' with prefix: '<lookupTableNamePrefix>'" — fixed text with two variable parts, the caller's account id and the prefix from the request. Smithy: DescribeLookupTables binds ResourceNotFoundException (httpError 400).
Emulator: HTTP 200 with a body (no error).
Why: an empty-result read is indistinguishable from AWS's refusal, so provisioning logic that keys on the refusal to create the table never runs.
Fix: the no-match branch of the DescribeLookupTables handler.
Traces: logs-b2-lookup-tables-and-integrations#task#1.

=== Cause 8 of 10: CreateLookupTable's duplicate-name refusal does not name the table or account ===
AWS: ResourceAlreadyExistsException (HTTP 400) with "Lookup table '<lookupTableName>' already exists in account '<accountId>'" — fixed text around two variable parts taken from the request and the caller identity. Smithy: CreateLookupTable binds ResourceAlreadyExistsException (httpError 400).
Emulator: 'The specified lookup table already exists' — both identifiers dropped.
Why: a caller creating several tables cannot tell which name collided.
Fix: the name-collision branch of the CreateLookupTable handler.
Traces: logs-lookup-table-crud-lifecycle#task#5.

=== Cause 9 of 10: ListTagsForResource refuses a lookup-table ARN ===
AWS: HTTP 200 with the resource's tags map; ListTagsForResource takes any CloudWatch Logs resourceArn (Smithy: AmazonResourceName @length 1..1011, @pattern '^[\\w+=/:,.@-]*$') including a lookup-table ARN created with tags.
Emulator: HTTP 400 and no tags member — the lookup-table ARN is not recognised by the tagging surface.
Why: tags set at CreateLookupTable time are unreadable, so tag-driven inventory and the tags-only update workflow both fail.
Fix: the ARN-to-resource resolver behind ListTagsForResource — register the lookup-table resource type.
Traces: logs-lookup-table-tags-and-description-only-update#task#2 (also #task#6).

=== Cause 10 of 10: GetTransformer on a log group with no transformer answers 400 ===
AWS: HTTP 200 with an EMPTY transformerConfig list (the recorded response is transformerConfig: []).
Emulator: HTTP 400 and no transformerConfig member.
Why: a client probing whether a group has a transformer gets an error instead of an empty configuration, so configure-if-absent logic aborts. (Note the Smithy model constrains Processors to @length 1..20, so the empty list is recorded behavior, not a model-derivable value — the recording is the wire truth.)
Fix: the no-transformer branch of the GetTransformer handler.
Traces: logs-b2-transformer-configuration#task#2 (also #task#6).

## Evidence
Cause 1: "status: expected 400, got 200; $.__type: missing (expected 'ValidationException', got None); $.message: missing (expected 'Provided LogType is not valid. Supported options are [INSIGHTS_QUERY_LOGS].', got None)".
Cause 2: "status: expected 400, got 200; $.__type: missing (expected 'ResourceNotFoundException', got None); $.message: missing (expected 'Requested Delivery Source does not exist in this account.', got None)".
Cause 3: "status: expected 400, got 200; $: empty_body (expected {'__type': 'ResourceNotFoundException', 'message': 'Requested Delivery Source does not exist in this account.'}, got {})".
Cause 4: "status: expected 400, got 200; $: empty_body (expected {'__type': 'ResourceNotFoundException', 'message': 'Delivery Destination Policy does not exist for this Delivery Destination.'}, got {})".
Cause 5: "$.message: value_mismatch (expected \"1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-]*\", got \"... pattern: ^[\\w-]*$\")" at HTTP 400 -> 400.
Cause 6: "$.policy.deliveryDestinationPolicy: value_mismatch (expected '{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"AllowLogDeliveryActionsOnAccount\",...}]}', got '{\"Version\": \"2012-10-17\", \"Statement\": [{\"Sid\": \"AllowLogDeliveryActionsOnAccount\", ...}]}')" — the two strings differ only by inserted whitespace; the ARNs inside are per-run values.
Cause 7: "status: expected 400, got 200; $.__type: missing (expected 'ResourceNotFoundException', got None); $.message: missing (expected \"No lookup tables found for accountId '000000000000' with prefix: '<prefix>'\", got None)" — account id and prefix are the variable parts.
Cause 8: "$.message: value_mismatch (expected \"Lookup table '<name>' already exists in account '000000000000'\", got 'The specified lookup table already exists')" at HTTP 400 -> 400 (6 affected steps).
Cause 9: "status: expected 200, got 400; $.tags: missing (expected {'env': 'probe', 'purpose': 'lookup'}, got None)" — the tag keys/values are the fixture's; the requirement is that the call succeeds and returns the tags that were set.
Cause 10: "status: expected 200, got 400; $.transformerConfig: missing (expected [], got None)".

## Steps to Reproduce
1: PutDeliverySource(name=<new>, resourceArn=<a resource>, logType='<a type the resource does not support>') — expect 400 ValidationException 'Provided LogType is not valid. Supported options are [...]' (logs-b2-delivery-source-destination-pipeline#task#11).
2/3: GetDeliverySource(name=<never created>) and DeleteDeliverySource(name=<never created>) — expect 400 ResourceNotFoundException 'Requested Delivery Source does not exist in this account.' (#task#12, #task#13).
4: PutDeliveryDestination then DeleteDeliveryDestinationPolicy without ever putting a policy — expect 400 ResourceNotFoundException 'Delivery Destination Policy does not exist for this Delivery Destination.' (#task#10).
5: PutDeliverySource(name='bad name!') — compare the echoed regex with the unanchored '[\\w-]*' (logs-delivery-source-crud-lifecycle#task#5).
6: PutDeliveryDestinationPolicy with a compact JSON document (no spaces after ':'/','), then GetDeliveryDestinationPolicy and compare the string byte-for-byte (logs-delivery-destination-and-cross-account-policy-lifecycle#task#6).
7: DescribeLookupTables(lookupTableNamePrefix=<prefix matching nothing>) — expect 400 ResourceNotFoundException (logs-b2-lookup-tables-and-integrations#task#1).
8: CreateLookupTable twice with the same lookupTableName — compare the ResourceAlreadyExistsException message (logs-lookup-table-crud-lifecycle#task#5).
9: CreateLookupTable with tags, then ListTagsForResource(resourceArn=<the lookup-table ARN>) — expect 200 and the tag map (logs-lookup-table-tags-and-description-only-update#task#2).
10: CreateLogGroup then GetTransformer on it without PutTransformer — expect 200 and transformerConfig: [] (logs-b2-transformer-configuration#task#2).
