Issue-Source: trace-sweep-divergence
Issue-Trace: backup-account-and-region-settings-overview
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-account-and-region-settings-overview backup-catalog-get-supported-resource-types backup-global-region-settings-and-supported-types
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|GetSupportedResourceTypes|2xx->5xx|$.ResourceTypes
Issue-Key: backup|GetSupportedResourceTypes|2xx->5xx|status
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# GetSupportedResourceTypes answers 501 instead of returning the supported resource-type list

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSupportedResourceTypes

## Description
Cause 1 of 1 — GetSupportedResourceTypes is not served.
AWS: Smithy: GetSupportedResourceTypes GET /supported-resource-types, input Unit, output GetSupportedResourceTypesOutput with a single member ResourceTypes (list of ResourceType); the only modeled error is ServiceUnavailableException (httpError 500). HTTP 501 is not a response this operation can produce, and the operation takes no input so it cannot be refused for a bad request. The recorded response is HTTP 200 with ResourceTypes listing the resource types AWS Backup supports in the region (recording shows DynamoDB, EBS, RDS, Storage Gateway, EFS, Aurora, Redshift, DocumentDB, Neptune, EC2, FSx, VirtualMachine, 'SAP HANA on Amazon EC2', S3, Timestream, CloudFormation, 'Redshift Serverless', DSQL, EKS — the exact membership is region/era dependent, so treat the recording as the example and the non-empty ResourceTypes list as the requirement).
Emulator: HTTP 501 with no ResourceTypes member at all.
Why: this is the capability-discovery call for AWS Backup. A client that enumerates supported resource types before building a backup plan or a selection gets an unmodeled 501 its SDK cannot map to any declared error, so capability discovery fails outright rather than returning a list.
Fix: implement the GetSupportedResourceTypes handler to project the service's supported resource-type list, returning 200.
Traces: backup-account-and-region-settings-overview#task#1, backup-catalog-get-supported-resource-types#task#1, backup-global-region-settings-and-supported-types#task#3.

## Evidence
backup-account-and-region-settings-overview#task#1 (GetSupportedResourceTypes): status expected 200, got 501; $.ResourceTypes expected a non-empty list (recorded: ['DynamoDB', 'EBS', 'RDS', 'Storage Gateway', 'EFS', 'Aurora', 'Redshift', 'DocumentDB', 'Neptune', 'EC2', 'FSx', 'VirtualMachine', 'SAP HANA on Amazon EC2', 'S3', 'Timestream', 'CloudFormation', 'Redshift Serverless', 'DSQL', 'EKS']), got None. The cluster also covers backup-catalog-get-supported-resource-types#task#1 and backup-global-region-settings-and-supported-types#task#3.
Smithy citation: GetSupportedResourceTypes, GET /supported-resource-types, code 200, output member ResourceTypes (list); sole modeled error ServiceUnavailableException (500) — 501 is unmodeled.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (trace set simparity-wire-aws-v1).
2. Fixture backup-account-and-region-settings-overview, step task#1 — GET /supported-resource-types with no input.
3. Observe HTTP 501 with no body members; AWS returns 200 with a populated ResourceTypes list.
