Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes cloudtrail__rds.obs-rds-3-cloudtrail-redacts-master-password-and-records-instance-not-found
Issue-Key: cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes|_await_records#31|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: cloudtrail__rds.obs-rds-3-cloudtrail-redacts-master-password-and-records-instance-not-found|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Commit: 27d60cb41c89860474fb4eac178b022e94c750b0

# CloudTrail records no management event for ELBv2 or RDS calls, so LookupEvents can never show a refused TargetGroup call or a redacted CreateDBInstance

> _Note: this issue was identified against the code base at commit `27d60cb41c89860474fb4eac178b022e94c750b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
One cause, grounded in two failing checks that no filed issue's replay set carries.

AWS: every management call of every service - successful and refused, read and write - is recorded in the Region's event history and readable through LookupEvents (Smithy: LookupEvents, input LookupAttributes/StartTime/EndTime/EventCategory/MaxResults, output Events carrying CloudTrailEvent). Three per-record properties these two checks grade are recorded behavior measured against real AWS (us-west-2, 2026-10-01): (a) a refused call's errorCode is the fault SHAPE name, not the wire code the caller saw - elbv2 DescribeTargetGroups on an absent ARN gives the caller Error.Code 'TargetGroupNotFound' while the record carries 'TargetGroupNotFoundException', CreateTargetGroup re-using a live name gives 'DuplicateTargetGroupName' while the record carries 'DuplicateTargetGroupNameException', and rds DescribeDBInstances on an unknown identifier gives 'DBInstanceNotFound' (Smithy DBInstanceNotFoundFault, client, httpError 404, awsQueryError DBInstanceNotFound) while the record carries 'DBInstanceNotFoundFault'; (b) a sensitive request member is never stored - CreateDBInstance.requestParameters.masterUserPassword reads the literal 'HIDDEN_DUE_TO_SECURITY_REASONS'; (c) a successful record carries no errorCode key at all, a non-null responseElements (the minted targetGroupArn) and requestParameters reproducing the submitted tags, and every record's requestID equals the caller's own ResponseMetadata.RequestId.

Emulator: no elbv2 or rds call is ever recorded. Both checks make their calls and then poll LookupEvents under their own budgets (elbv2 _LOOKUP_DEADLINE_S=1200 at 15 s intervals; rds _CLOUDTRAIL_WAIT_S=1200 at 30 s, up to 40 pages), never see a matching record, and are therefore still polling when the harness's 60 s cap ends them ('Check timed out after 60.0s'), so none of the errorCode/redaction/requestID assertions is ever reached. LookupEvents itself serves pages out of the shared management-event log, but that log is only written by CloudTrail's own mutating handlers.

Why: the emulator cannot answer 'who did this'. A client that reconstructs an incident, proves a master password was never persisted in a trail, or reconciles failed calls from event history reads an empty trail and cannot distinguish 'nothing happened' from 'nothing is recorded' - every detective control built on CloudTrail silently passes.

Fix: the shared management-event recording seam CloudTrail's state.rs::record_management_event already uses (service_common::management_events) must be driven from every served operation of every service, for read-only and FAILED calls as well, populating eventSource/eventName/readOnly/requestParameters/responseElements/requestID, setting a refused call's errorCode from the fault shape name, and substituting the redaction literal for sensitive request members.

Already-filed issues that do NOT cover these two checks: 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md names the same root cause but its replay set lists the OTHER elbv2 check (obs-elbv2-3-idempotent-create-replay-...) and not this one; 20261024T030130Z-cloudtrail-rds-cloudtrail-lookupevents-never-records-rds-management-events-so.md names the rds check id but was keyed at a different frame (_retry_delete#1) and the suite no longer binds this failure to it, so neither issue would cause these two checks to be re-replayed. Fix them together.

Traces: cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes, cloudtrail__rds.obs-rds-3-cloudtrail-redacts-master-password-and-records-instance-not-found.

## Evidence
Both checks report verdict 'mismatch' with detail 'Check timed out after 60.0s'.
cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes: after a tagged VPC, one successful CreateTargetGroup, a DescribeTargetGroups on an absent ARN and a duplicate-name CreateTargetGroup, it polls lookup_events by EventName (CreateTargetGroup / DescribeTargetGroups) joining records to calls on ResponseMetadata.RequestId, under _LOOKUP_DEADLINE_S=1200 / _POLL_INTERVAL_S=15; it asserts the audited errorCodes are exactly 'TargetGroupNotFoundException' and 'DuplicateTargetGroupNameException' while the wire codes were 'TargetGroupNotFound' / 'DuplicateTargetGroupName'. No record is ever returned, so it never reaches those assertions.
cloudtrail__rds.obs-rds-3-cloudtrail-redacts-master-password-and-records-instance-not-found: after CreateDBInstance with a random MasterUserPassword and a DescribeDBInstances on a nonexistent identifier, it polls lookup_events for eventSource 'rds.amazonaws.com' under _CLOUDTRAIL_WAIT_S=1200 / 30 s / 40 pages, expecting requestParameters.masterUserPassword == 'HIDDEN_DUE_TO_SECURITY_REASONS' and errorCode 'DBInstanceNotFoundFault'. No record is ever returned.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. elbv2: create a VPC, CreateTargetGroup (TargetType instance, Port 8080, with tags), DescribeTargetGroups(TargetGroupArns=[a well-formed ARN that does not exist]) - the caller sees 400 TargetGroupNotFound - and CreateTargetGroup again with the same Name but Port 9090 - the caller sees 400 DuplicateTargetGroupName. Record each call's ResponseMetadata.RequestId.
3. rds: CreateDBInstance with a MasterUserPassword, then DescribeDBInstances with an identifier that does not exist (the caller sees DBInstanceNotFound).
4. cloudtrail: LookupEvents over a window covering those calls, with LookupAttributes [{AttributeKey: EventName, AttributeValue: CreateTargetGroup}] / [{AttributeKey: EventSource, AttributeValue: rds.amazonaws.com}].
5. Expect one record per call - the refused ones carrying errorCode 'TargetGroupNotFoundException' / 'DuplicateTargetGroupNameException' / 'DBInstanceNotFoundFault' with null responseElements, the successful ones carrying no errorCode and the minted targetGroupArn, CreateDBInstance's requestParameters.masterUserPassword reading 'HIDDEN_DUE_TO_SECURITY_REASONS', and every record's requestID equal to the caller's RequestId. Observe Events: [] for every lookup.
