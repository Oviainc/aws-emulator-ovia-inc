Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-export-aws-managed-schema
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-export-aws-managed-schema
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|DescribeSchema|2xx->2xx|$.Content
Issue-Key: schemas|ExportSchema|2xx->2xx|$.Content
Issue-Commit: b27510b4d97b7d3408c23fe8ccf3618e6d5ed1db

# schemas DescribeSchema and ExportSchema return the wrong Content document for AWS-managed schemas

> _Note: this issue was identified against the code base at commit `b27510b4d97b7d3408c23fe8ccf3618e6d5ed1db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSchema

## Description
Both read paths for an AWS-managed schema's body return status 200 with a Content string that is not the document AWS serves for that schema. This bundle has TWO causes (one per operation); both must be closed before submitting. They very likely share one root — the stored body for the AWS-managed schema — but they are separate response paths and each has its own replay key.

Cause 1 of 2: DescribeSchema returns the wrong Content for aws.events@AWSAPICallviaCloudTrail.
- AWS: recorded behavior only for the document text; Smithy (schemas) models DescribeSchema as GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}, code 200, output {Content: __string, SchemaName, SchemaArn, SchemaVersion, Type, Description, LastModified/@timestampFormat date-time, VersionCreatedDate, Tags@jsonName tags}, errors BadRequest 400 / Forbidden 403 / NotFound 404 / Unauthorized 401 / InternalServerError 500 / ServiceUnavailable 503. The recording's Content is the AWS-published OpenAPI 3.0 document for that schema: `{"openapi":"3.0.0","info":{"title":"AWS API Call via CloudTrail published by aws.events","version":"1.0.0"},"paths":{},"components":{"schemas":{"AWSAPICallviaCloudTrail":{"type":"object","properties":{...}}}}}`, carrying the full CloudTrail property set (addendum, additionalEventData, annotation, apiVersion, awsRegion, edgeDeviceDetails, errorCode, errorMessage, eventCategory, eventContext, eventID, eventName, eventSource, eventTime with pattern yyyy-MM-dd'T'HH:mm:ss'Z' and format date-time, eventType, eventVersion, managementEvent, readOnly, recipientAccountId, requestID, requestParameters, resources[{accountId,ARN,ARNPrefix,type}], responseElements, sessionCredentialFromConsole, sharedEventID, sourceIPAddress, tlsDetails{cipherSuite,clientProvidedHostHeader,keyExchange,tlsVersion}, userAgent, userIdentity{...}), with nullable:true on the object-or-null members.
- Emulator: status 200 and the right envelope, but $.Content is a different document (value_mismatch on $.Content; the whole string differs, so it is not a whitespace or key-order difference).
- Why: Content is the entire point of DescribeSchema. A client that fetches it to generate event bindings, validate an event payload, or render the schema in a console gets a document that does not describe the AWS event, so generated types and validation of a real CloudTrail event are wrong even though the call succeeded.
- Fix: the schemas service's AWS-managed schema content state — the stored OpenAPI 3.0 body for the seeded `aws.*` schemas that the DescribeSchema handler returns; the handler's envelope and status are already correct.
- Traces: schemas-export-aws-managed-schema#task#2.

Cause 2 of 2: ExportSchema returns the wrong Content for the JSONSchemaDraft4 export of the same schema.
- AWS: recorded behavior only for the document text; Smithy models ExportSchema as GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/export with required query `type`, code 200, output {Content, SchemaName, SchemaArn, SchemaVersion, Type}, errors BadRequest 400 / Forbidden 403 / NotFound 404 / TooManyRequests 429 / Unauthorized 401 / InternalServerError 500 / ServiceUnavailable 503. The recorded Content is the draft-04 rendering of the same schema: `{"$schema":"http://json-schema.org/draft-04/schema#","title":"AWS API Call via CloudTrail published by aws.events","definitions":{"AWSAPICallviaCloudTrail":{"properties":{...}}}}`, i.e. the OpenAPI document converted — `definitions` instead of `components.schemas`, union types such as `"type":["object","null"]` in place of `nullable:true`, and alphabetically ordered property keys.
- Emulator: status 200, $.Content value_mismatch — the exported document is not that draft-04 conversion.
- Why: a client exporting a schema to feed a JSON-Schema validator or a code generator gets a document that neither matches AWS's export nor the event it is meant to validate, and cannot tell from the 200 that the conversion is not AWS's.
- Fix: the ExportSchema handler's content conversion in the schemas service (OpenAPI 3.0 -> JSON Schema draft-04: definitions mapping, nullable -> ["type","null"] union, key ordering), reading the corrected stored body from Cause 1.
- Traces: schemas-export-aws-managed-schema#task#3.

Not covered by any existing issue: 20261023T142626Z-schemas-empty-result-responses-use-the-wrong-status.md covers only empty-result statuses/Content-Length on DeleteResourcePolicy and DeleteSchema and does not carry either of these replay keys.

## Evidence
Replay summary: steps_total 289, steps_passed 55, steps_mismatched 227, score_pct 19.0, emulator_up true, bootstrap_signal false, state_reset_failures 0.

Cause 1 — fixture schemas-export-aws-managed-schema, step task#2, operation DescribeSchema, verdict mismatch, expected_status 200, actual_status 200, divergence_key "service=schemas|DescribeSchema|status=200->200|keys=$.Content", affected_count 1. detail: "$.Content: value_mismatch (expected '{\"openapi\":\"3.0.0\",\"info\":{\"title\":\"AWS API Call via CloudTrail published by aws.events\",\"version\":\"1.0.0\"},\"paths\":{},\"components\":{\"schemas\":{\"AWSAPICallviaCloudTrail\":{\"type\":\"object\",\"properties\":{\"addendum\":{\"type\":\"object\",\"nullable\":true},... \"eventTime\":{\"pattern\":\"yyyy-MM-dd'T'HH:mm:ss'Z'\",\"type\":\"string\",\"format\":\"date-time\"},... \"tlsDetails\":{...},\"userIdentity\":{...}'...)". Only $.Content diverges — status, SchemaName, SchemaArn, SchemaVersion and Type matched.

Cause 2 — fixture schemas-export-aws-managed-schema, step task#3, operation ExportSchema, verdict mismatch, expected_status 200, actual_status 200, divergence_key "service=schemas|ExportSchema|status=200->200|keys=$.Content", affected_count 1. detail: "$.Content: value_mismatch (expected '{\"$schema\":\"http://json-schema.org/draft-04/schema#\",\"title\":\"AWS API Call via CloudTrail published by aws.events\",\"definitions\":{\"AWSAPICallviaCloudTrail\":{\"properties\":{\"addendum\":{\"type\":[\"object\",\"null\"]},... \"requestParameters\":{\"type\":[\"object\",\"null\"]},... }'...)".

Smithy (services/service-com-amazonaws-schemas-schemas): DescribeSchema GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName} (code 200, output Content/Description/LastModified/SchemaArn/SchemaName/SchemaVersion/Tags(jsonName tags)/Type/VersionCreatedDate); ExportSchema GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/export with required @httpQuery type (code 200, output Content/SchemaArn/SchemaName/SchemaVersion/Type). The model does not specify the document text, so the recorded documents are the reference.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture schemas-export-aws-managed-schema step task#2 — GET /v1/registries/name/aws.events/schemas/name/aws.events@AWSAPICallviaCloudTrail — and diff the returned Content against the recorded OpenAPI 3.0 document (expect openapi 3.0.0, info.title "AWS API Call via CloudTrail published by aws.events", components.schemas.AWSAPICallviaCloudTrail with the full CloudTrail property set and nullable:true on the object-or-null members).
3. Cause 2: replay the same fixture's step task#3 — GET .../schemas/name/aws.events@AWSAPICallviaCloudTrail/export?type=JSONSchemaDraft4 — and diff Content against the recorded draft-04 document (expect $schema http://json-schema.org/draft-04/schema#, top-level definitions, and "type":["object","null"] unions in place of nullable:true).
4. Both steps return 200 today; the divergence is the Content body only, so assert on the parsed document rather than the status.
