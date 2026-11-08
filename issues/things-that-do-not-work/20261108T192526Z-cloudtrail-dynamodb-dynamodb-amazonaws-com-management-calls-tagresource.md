Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__dynamodb.dynamodb-data-resource-selector-is-stored-and-its-data-events-stay-out-of-event-history
Issue-Key: cloudtrail__dynamodb.dynamodb-data-resource-selector-is-stored-and-its-data-events-stay-out-of-event-history|_lookup_all#44|Timeout|return out
Issue-Commit: b668b970d0d9c7dee5ca992802cd99dd932aec82

# cloudtrail__dynamodb: dynamodb.amazonaws.com management calls (TagResource) are never recorded in event history, so a trail's AWS::DynamoDB::Table data-selector check can never reach a verdict

> _Note: this issue was identified against the code base at commit `b668b970d0d9c7dee5ca992802cd99dd932aec82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Cause 1 of 1: no DynamoDB control-plane call is recorded into CloudTrail event history, so LookupEvents can never return a dynamodb.amazonaws.com Management record and the data-event/management-event partition of event history cannot be observed at all.

AWS: every management (control-plane) call of every service is written to the Region's 90-day event history and is readable through LookupEvents. Smithy (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101): LookupEvents input LookupAttributes/StartTime/EndTime/EventCategory/MaxResults (@range 1..50)/NextToken, output Events (EventsList) + NextToken, paginated on NextToken; errors InvalidLookupAttributesException, InvalidTimeRangeException, InvalidMaxResultsException, InvalidNextTokenException, InvalidEventCategoryException, OperationNotPermittedException, UnsupportedOperationException (all client, HTTP 400). dynamodb:TagResource is a control-plane act, so AWS records it with eventSource "dynamodb.amazonaws.com", eventName "TagResource", eventCategory "Management", and requestParameters naming the table ARN the call addressed. Conversely dynamodb:PutItem / GetItem are DATA events: even when a trail's basic event selector names the table with DataResources [{Type: "AWS::DynamoDB::Table", Values: [<table arn>]}] and the trail is logging, those records go to the trail only and must NOT appear in LookupEvents - event history is management-only (recorded behavior, measured by the check against real AWS).

Emulator: nothing dynamodb-sourced is ever recorded. services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/tag_resource.rs serves TagResource entirely out of its own table store (resolve_resource_arn -> table.tags.merge) and never writes a management event to the shared audit seam; CloudTrail's LookupEvents reads only the management-event log its own mutating handlers populate. The check therefore issues CreateTable, PutEventSelectors with the AWS::DynamoDB::Table data resource, StartLogging, PutItem, GetItem and then TagResource, and polls LookupEvents(LookupAttributes=[{AttributeKey: "EventName", AttributeValue: "TagResource"}]) for its own marker under a 255 s index budget; Events never contains the record, so the check is still polling when the 180 s harness cap ends it ("Check timed out after 180.0s") and never reaches any of its field or absence assertions.

Why: a client cannot answer "who tagged/changed this table". Every detective control built on event history (incident reconstruction, compliance evidence, reconciliation of refused calls) reads an empty result and cannot distinguish "nothing happened" from "nothing is recorded". Worse, the management-only guarantee of event history is unverifiable in both directions: because the positive control never appears, a client also cannot tell whether the emulator is correctly withholding PutItem/GetItem from LookupEvents or simply records nothing at all.

Fix: the shared management-event recording seam CloudTrail's state.rs::record_management_event / service_common::management_events already reads must be driven from DynamoDB's served operations (TagResource and the rest of its control plane), populating eventSource=dynamodb.amazonaws.com, eventName, eventCategory=Management, readOnly, requestParameters (including the ResourceArn the call addressed), responseElements and requestID equal to the caller's own request id; and the LookupEvents read path must keep serving Management events only, so data events selected by a trail's AWS::DynamoDB::Table DataResources never leak into event history.

Related open issue: 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md describes the same family of gap (its Cause 1) and even lists this check id, but the replay suite does not report this check as owned by it - its recorded replay key for this check no longer matches the failure observed here - so this check is not covered and is filed on its own. The per-service variants already closed (RDS, IAM, ELBv2, Cognito Identity, CodePipeline, CloudFormation, STS) show the seam exists and simply has no DynamoDB caller.

Traces: cloudtrail__dynamodb.dynamodb-data-resource-selector-is-stored-and-its-data-events-stay-out-of-event-history.

## Evidence
Check cloudtrail__dynamodb.dynamodb-data-resource-selector-is-stored-and-its-data-events-stay-out-of-event-history - verdict 'mismatch', detail 'Check timed out after 180.0s'.

Expected: after ddb.tag_resource(ResourceArn=<table arn>, Tags=[{Key: 'sim2real-marker', Value: <token>}]), a LookupEvents over a window containing the call with LookupAttributes=[{AttributeKey: 'EventName', AttributeValue: 'TagResource'}] returns an event whose CloudTrailEvent JSON has eventSource == 'dynamodb.amazonaws.com', eventCategory == 'Management' and requestParameters naming the table ARN; and a LookupEvents for EventName 'PutItem' / 'GetItem' over the same window returns no event naming that table, its ARN or the item token, with every event an unfiltered LookupEvents returns carrying eventCategory 'Management'.

Actual: LookupEvents returns no TagResource record at all for the table. The check's poll loop (interval 2 s, budget _INDEX_BUDGET_S = 255 s) never satisfies its positive control, so it is killed by the 180 s harness cap before any assertion runs. The check declares DATAPLANE_MARKER = 'DATAPLANE_UNIMPLEMENTED' precisely for a target that produces no audit record, and its own message states that without the management marker 'the data-event absences below cannot be distinguished from a target that audits no DynamoDB activity at all'.

Corroborating code in the checkout: services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/tag_resource.rs lines 186-242 - handle() resolves the ARN, bounds the tag list and merges the tags into the table's stored tag set, then returns TagResourceOutput; there is no call into any CloudTrail/management-event recording seam. Earlier CloudTrail work closed the same gap service by service (e.g. 20261024T030130Z for RDS, 20261025T121312Z for IAM, 20261026T054653Z for Cognito Identity, 20261029T064922Z for CloudFormation), so the recording seam is present and DynamoDB simply never drives it.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. dynamodb CreateTable (KeySchema pk HASH, BillingMode PAY_PER_REQUEST) and wait for TableStatus ACTIVE; keep the TableArn.
3. s3 CreateBucket, PutBucketPolicy with the standard CloudTrail AclCheck/Write statements; cloudtrail CreateTrail(Name=<trail>, S3BucketName=<bucket>, IsMultiRegionTrail=false, IncludeGlobalServiceEvents=false).
4. cloudtrail PutEventSelectors(TrailName=<trail>, EventSelectors=[{ReadWriteType: 'All', IncludeManagementEvents: true, DataResources: [{Type: 'AWS::DynamoDB::Table', Values: ['<TableArn>']}]}]) and cloudtrail StartLogging(Name=<trail>); GetTrailStatus must report IsLogging true.
5. dynamodb PutItem then GetItem against the table (data events), then dynamodb TagResource(ResourceArn='<TableArn>', Tags=[{Key: 'sim2real-marker', Value: '<token>'}]) (management event).
6. cloudtrail LookupEvents(StartTime=<before step 5>, EndTime=now+5m, MaxResults=50, LookupAttributes=[{AttributeKey: 'EventName', AttributeValue: 'TagResource'}]), paging to exhaustion.
   Expect: one event whose CloudTrailEvent carries eventSource 'dynamodb.amazonaws.com', eventCategory 'Management' and the table ARN in requestParameters.
   Observe: Events is empty - no dynamodb record is ever indexed, however long the poll runs.
7. Then LookupEvents for EventName 'PutItem' and 'GetItem' over the same closed window, and an unfiltered LookupEvents over it: AWS returns no data-event record naming the table and only eventCategory 'Management' events; this half cannot be judged at all today because step 6 never produces the barrier record.
