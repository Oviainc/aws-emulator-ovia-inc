Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__stepfunctions.obs-sfn-2-data-selector-logs-start-sync-execution-to-the-trail-prefix-only-after-it-is-added
Issue-Key: cloudtrail__stepfunctions.obs-sfn-2-data-selector-logs-start-sync-execution-to-the-trail-prefix-only-after-it-is-added|run#48|AssertionError|assert rec.get("eventCategory") == "Data", (
Issue-Commit: 8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f

# cloudtrail__stepfunctions: StartSyncExecution matched by an AWS::StepFunctions::StateMachine advanced data selector is delivered as a Management event (eventCategory 'Management'), so Step Functions data events never exist

> _Note: this issue was identified against the code base at commit `8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSyncExecution

## Description
Cause 1 of 1: a CloudTrail trail carrying an advanced event selector for Step Functions data events delivers the matching StartSyncExecution call to S3, but classifies the delivered record as a management event instead of a data event.

AWS: Smithy (cloudtrail): AdvancedEventSelector.FieldSelectors (required, min 1) of AdvancedFieldSelector{Field (required, pattern ^[\w|\d|\.|_]+$), Equals}; the member docs state that Equals is "the only valid operator that you can use with the readOnly, eventCategory, and resources.type fields", i.e. eventCategory and resources.type are first-class selector fields and a selector may therefore be written as eventCategory Equals ["Data"] + resources.type Equals ["AWS::StepFunctions::StateMachine"] + resources.ARN Equals [<stateMachineArn>]. The CloudTrail log-record document itself is not in any Smithy model, so recorded behavior only for the record contents: the recording shows that, after such a selector is added, a StartSyncExecution call against the named EXPRESS state machine is delivered under the trail's S3 key prefix as a DATA event — the record carries eventCategory "Data", managementEvent false, readOnly false, eventSource "states.amazonaws.com", a resources entry whose ARN is the state machine and whose type is AWS::StepFunctions::StateMachine, requestParameters.stateMachineArn equal to that ARN, requestParameters.input replaced by the literal sentinel "HIDDEN_DUE_TO_SECURITY_REASONS", responseElements absent, and requestID equal to the request id the SDK observed on that call. Those are the shape of the requirement; the ARNs, names, request ids and timestamps are taken from the request and from the trail's own state, never from one run.

Emulator: the record IS delivered and IS found by eventName/requestParameters.name, but it is classified as a management event: the delivered JSON carries "eventCategory": "Management" where AWS writes "eventCategory": "Data". The check's first data-event assertion therefore fails with: expected eventCategory 'Data', actual 'Management'. Because the emulator never produces the Data classification for this call, none of the data-event-only members the check goes on to require (managementEvent false, readOnly false, the resources[] entry naming the state machine as AWS::StepFunctions::StateMachine, the redaction of requestParameters.input to HIDDEN_DUE_TO_SECURITY_REASONS, the absent responseElements) can be reached or confirmed; the classification is the diverging behavior. Note that the record was delivered BEFORE/independently of the selector being added as well, which is consistent with the call being accounted as management traffic rather than as selector-matched data traffic.

Why: a client that enables Step Functions data events to audit who started which EXPRESS execution gets records that are indistinguishable from management traffic. Queries and Athena/Lake partitions filtered on eventCategory = 'Data' return nothing; any pipeline that routes data events separately from management events (different trails, different retention, different cost accounting) silently drops every StartSyncExecution; and consumers relying on the data-event contract also lose the resources[] attribution and the input redaction guarantee, so a tool could not even tell that the input was withheld deliberately.

Fix: CloudTrail's event-classification/selector-evaluation path — the component that decides a recorded API call's eventCategory and applies AdvancedEventSelectors before emitting the record — must recognise states:StartSyncExecution on an AWS::StepFunctions::StateMachine as a data event, set eventCategory "Data"/managementEvent false, attach the resources[] entry for the state machine, and apply the input redaction; the record serializer that writes the gzipped JSON into the trail's S3 prefix must carry those members through.

Traces: cloudtrail__stepfunctions.obs-sfn-2-data-selector-logs-start-sync-execution-to-the-trail-prefix-only-after-it-is-added.

## Evidence
Failing check: cloudtrail__stepfunctions.obs-sfn-2-data-selector-logs-start-sync-execution-to-the-trail-prefix-only-after-it-is-added [mismatch].

Expected (AWS): the StartSyncExecution record delivered under the trail prefix after PutEventSelectors added {Name: 'sfn-subject-data', FieldSelectors: [{Field: 'eventCategory', Equals: ['Data']}, {Field: 'resources.type', Equals: ['AWS::StepFunctions::StateMachine']}, {Field: 'resources.ARN', Equals: [<subject state machine ARN>]}]} has eventCategory 'Data' (and managementEvent false, readOnly false, resources[] naming the subject machine, requestParameters.input == 'HIDDEN_DUE_TO_SECURITY_REASONS', responseElements absent).

Actual (emulator): AssertionError at check line 213 — "StartSyncExecution under a AWS::StepFunctions::StateMachine selector is a data event: expected eventCategory 'Data', actual 'Management'". The delivered record read back from s3 begins:
{"eventVersion": "1.09", "userIdentity": {"type": "IAMUser", ...}, "eventTime": "...Z", "eventSource": "states.amazonaws.com", "eventName": "StartSyncExecution", "awsRegion": "<region>", "userAgent": "Boto3/..."} — i.e. eventSource/eventName are right, the classification is not. (The account id, region, timestamps, user agent and request ids in that dump are per-run values, not part of the requirement.)

## Steps to Reproduce
1. Create an S3 log bucket with a CloudTrail-writable policy and a key prefix; create an IAM execution role and TWO EXPRESS state machines (a subject and a bystander) whose definition is a single Pass state.
2. CreateTrail(Name, S3BucketName, S3KeyPrefix=<prefix>, IsMultiRegionTrail=false, IncludeGlobalServiceEvents=false); PutEventSelectors with only a management selector ({Field:'eventCategory', Equals:['Management']}); StartLogging; confirm GetTrailStatus.IsLogging is true.
3. Poll the trail prefix until a management marker event (e.g. DescribeStateMachine) is delivered, proving the trail is capturing.
4. PutEventSelectors again with the management selector PLUS the data selector {Field:'eventCategory' Equals ['Data'], Field:'resources.type' Equals ['AWS::StepFunctions::StateMachine'], Field:'resources.ARN' Equals [<subject ARN>]}; confirm GetEventSelectors returns it verbatim.
5. Call StartSyncExecution on the subject machine (and on the bystander) and poll the trail's S3 prefix for a record with eventName 'StartSyncExecution' whose requestParameters.name is one of the subject calls.
6. Read that record's eventCategory: the emulator returns 'Management'; AWS returns 'Data' (with managementEvent false, readOnly false, resources[] naming the subject machine and requestParameters.input redacted to 'HIDDEN_DUE_TO_SECURITY_REASONS').
