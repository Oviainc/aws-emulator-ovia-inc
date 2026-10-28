Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__rds.obs-rds-3-cloudtrail-redacts-master-password-and-records-instance-not-found
Issue-Key: cloudtrail__rds.obs-rds-3-cloudtrail-redacts-master-password-and-records-instance-not-found|_retry_delete#1|Timeout|time.sleep(interval_s)
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# CloudTrail LookupEvents never records RDS management events, so no API call is auditable

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
One cause. AWS: every RDS control-plane call is a management event recorded in CloudTrail event history and readable through LookupEvents with eventSource 'rds.amazonaws.com'. Two facts about those records are what this check grades: (a) a sensitive request member is not stored — CloudTrail substitutes the literal 'HIDDEN_DUE_TO_SECURITY_REASONS' for CreateDBInstance.requestParameters.masterUserPassword, and the plaintext appears nowhere in the record; (b) a FAILED call is recorded with an errorCode that is the fault-shape name, not the wire code the caller saw — for an unknown DB instance identifier the caller gets Error.Code 'DBInstanceNotFound' (Smithy: DBInstanceNotFoundFault, client, httpError 404, awsQueryError code DBInstanceNotFound) while the trail records errorCode 'DBInstanceNotFoundFault'. Both literals are recorded behavior, measured by the check against real AWS.

Emulator: no RDS management event is ever surfaced. The check makes its calls and then polls LookupEvents for an rds.amazonaws.com record until the harness's wall clock expires ('Check timed out after 60.0s'); its poll budget is _CLOUDTRAIL_WAIT_S=1200, so it never reaches any of its assertions. Nothing is recorded, so neither the redaction nor the errorCode mapping can be observed.

Why: auditing is absent. A client that verifies which principal created or deleted a database, that proves a master password was never persisted in a trail, or that reconciles failures from event history, finds an empty trail — and any security control built on CloudTrail (detective guardrails, incident reconstruction, compliance evidence) has nothing to read.

Fix: the CloudTrail event-history store and its LookupEvents handler must receive a management event for each served API call, with eventSource/eventName/requestParameters/responseElements/errorCode populated, sensitive request members replaced by the redaction literal, and a failed call's errorCode set from the fault shape name.

Traces: cloudtrail__rds.obs-rds-3-cloudtrail-redacts-master-password-and-records-instance-not-found.

## Evidence
cloudtrail__rds.obs-rds-3-cloudtrail-redacts-master-password-and-records-instance-not-found — verdict 'mismatch', 'Check timed out after 60.0s'. The check builds a VPC/subnets/DB subnet group, calls CreateDBInstance with a random master password, calls DescribeDBInstances on an identifier that does not exist (expecting caller-visible Error.Code 'DBInstanceNotFound'), then polls cloudtrail LookupEvents (budget _CLOUDTRAIL_WAIT_S=1200, interval 30s, up to _PAGE_CEILING=40 pages) for records with eventSource 'rds.amazonaws.com'. No record is ever returned, so the poll runs to the harness timeout and the redaction ('HIDDEN_DUE_TO_SECURITY_REASONS') and trail-errorCode ('DBInstanceNotFoundFault') assertions are never reached.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. rds: Action=CreateDBInstance with a MasterUserPassword, then Action=DescribeDBInstances with an identifier that does not exist (the caller sees 404 / DBInstanceNotFound).
3. cloudtrail: LookupEvents with LookupAttributes [{AttributeKey: EventSource, AttributeValue: rds.amazonaws.com}] over a window covering those calls.
4. No events are returned — neither the CreateDBInstance record (whose requestParameters.masterUserPassword should read 'HIDDEN_DUE_TO_SECURITY_REASONS') nor the failed DescribeDBInstances record (whose errorCode should be 'DBInstanceNotFoundFault').
