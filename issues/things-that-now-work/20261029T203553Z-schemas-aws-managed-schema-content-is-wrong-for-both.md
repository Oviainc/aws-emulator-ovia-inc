Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-export-aws-managed-schema
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-export-aws-managed-schema
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|DescribeSchema|2xx->2xx|$.Content
Issue-Key: schemas|ExportSchema|2xx->2xx|$.Content
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# AWS-managed schema Content is wrong for both DescribeSchema and ExportSchema

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSchema

## Description
Reading an AWS-managed schema out of the service-owned catalog returns a Content document that is not the one AWS publishes, on both the describe path and the export path. Two causes.

Cause 1 of 2 - DescribeSchema Content (the OpenApi3 document). AWS: Smithy models DescribeSchema GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName} -> 200 with Content as __string; what the document must BE is recorded behavior only. The recording shows, for the catalog entry aws.events@AWSAPICallviaCloudTrail, an OpenAPI 3.0.0 document of this shape: {"openapi":"3.0.0","info":{"title":"<Detail Type> published by <source>","version":"1.0.0"},"paths":{},"components":{"schemas":{"<EventName>":{"type":"object","properties":{...}}}}} - the title being the detail type in its spelled-out form followed by ' published by ' and the publishing source (here 'AWS API Call via CloudTrail published by aws.events'), paths empty, and the event's component carrying the full CloudTrail property set: addendum and additionalEventData and requestParameters and responseElements as {"type":"object","nullable":true}, annotation, apiVersion, awsRegion, edgeDeviceDetails, errorCode, errorMessage, eventCategory, eventContext, eventID, eventName, eventSource, eventTime as {"pattern":"yyyy-MM-dd'T'HH:mm:ss'Z'","type":"string","format":"date-time"}, eventType, eventVersion, managementEvent, readOnly, recipientAccountId, requestID, resources as an array of objects with accountId/ARN/ARNPrefix/type, sessionCredentialFromConsole, sharedEventID, sourceIPAddress, tlsDetails with cipherSuite/clientProvidedHostHeader/keyExchange/tlsVersion, userAgent, and userIdentity with accessKeyId/accountId/arn/credentialId/identityProvider/inScopeOf/invokedBy/invokedByDelegate/onBehalfOf{identityStoreArn,userId}/principalId/sessionContext. Emulator: serves the document rendered by event_schema_document() from the constant component JSON of the seeded catalog row (state.rs, reached through ManagedEventType::schema() -> ManagedSchema.content, ~line 2410), and the replay reports the whole $.Content as a value_mismatch against the recorded document. Why: Content is the entire point of the operation - it is the document a client validates events against or generates code from, so a client that reads the registry gets a contract that does not describe the events AWS actually delivers. Fix: the seeded OpenApi3 document for the AWS-managed catalog entries (the component rows and event_schema_document()/managed_schema_description() in the service's state module) that the DescribeSchema handler serves. Traces: schemas-export-aws-managed-schema#task#2.

Cause 2 of 2 - ExportSchema Content (the JSONSchemaDraft4 conversion). AWS: Smithy models ExportSchema GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/export with a required `type` query member -> 200 with Content as __string; the document is recorded behavior only. The recording shows the draft-4 conversion as {"$schema":"http://json-schema.org/draft-04/schema#","title":"<Detail Type> published by <source>","definitions":{"<EventName>":{"properties":{...},"type":"object"}},...} - i.e. components.schemas becomes definitions, the OpenAPI nullable form {"type":"object","nullable":true} becomes the draft-4 union {"type":["object","null"]} (addendum, additionalEventData, requestParameters, responseElements), each object's members are emitted in draft-4 key order (properties before type, format before pattern on eventTime), and the OpenAPI-only keyword readOnly is dropped from the event component. Emulator: the replay reports the whole $.Content as a value_mismatch against that document. Why: a client exporting to draft-4 to feed a JSON-schema validator gets a document that is not the published one, so validation of real events disagrees with AWS. Fix: the ExportSchema handler's OpenApi3 -> JSONSchemaDraft4 conversion, plus the seeded source document from cause 1 that it converts. The open issue 20261029T085227Z-schemas-exportschema-wrong-behavior--r2.md reports one facet of this conversion (the draft-4 export carrying no root schema) but does not carry this fixture step, so it does not cover this divergence. Traces: schemas-export-aws-managed-schema#task#3.

## Evidence
Replay of fixture schemas-export-aws-managed-schema, service=schemas:

Step task#2, DescribeSchema, expected 200 / actual 200: $.Content value_mismatch. Expected document begins '{"openapi":"3.0.0","info":{"title":"AWS API Call via CloudTrail published by aws.events","version":"1.0.0"},"paths":{},"components":{"schemas":{"AWSAPICallviaCloudTrail":{"type":"object","properties":{"addendum":{"type":"object","nullable":true},"additionalEventData":{"type":"object","nullable":true},"annotation":{"type":"string"},...,"eventTime":{"pattern":"yyyy-MM-dd\'T\'HH:mm:ss\'Z\'","type":"string","format":"date-time"},...,"readOnly":{"type":"boolean"},...,"tlsDetails":{"type":"object","properties":{"cipherSuite":...}},"userIdentity":{"type":"object","properties":{"accessKeyId":...,"inScopeOf":{"type":"object"},"invokedByDelegate":{"type":"object"},"onBehalfOf":{"type":"object","properties":{"identityStoreArn":...,"userId":...}}}}}}}}}'. The served document differs over the whole value.

Step task#3, ExportSchema, expected 200 / actual 200: $.Content value_mismatch. Expected document begins '{"$schema":"http://json-schema.org/draft-04/schema#","title":"AWS API Call via CloudTrail published by aws.events","definitions":{"AWSAPICallviaCloudTrail":{"properties":{"addendum":{"type":["object","null"]},"additionalEventData":{"type":["object","null"]},...,"eventTime":{"format":"date-time","pattern":"yyyy-MM-dd\'T\'HH:mm:ss\'Z\'","type":"string"},...,"requestParameters":{"type":["object","null"]},"responseElements":{"type":["object","null"]},...},"type":"object"}}}' - note no readOnly member and the nullable union form. The served document differs over the whole value.

Smithy: DescribeSchema output Content: __string, GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}; ExportSchema output Content: __string, GET .../export with required httpQuery `type`.

Code: services/service-com-amazonaws-schemas-schemas/src/state.rs - ManagedEventType::schema() ~line 2410 builds ManagedSchema.content via event_schema_document(source, detail_type, event_name, components_json); aws_managed_registries() ~line 2756 stores that one document as every version's content.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture schemas-export-aws-managed-schema step task#2: GET /v1/registries/name/aws.events/schemas/name/aws.events@AWSAPICallviaCloudTrail. Observe 200 whose Content is not the published OpenAPI 3.0.0 document.
3. Replay step task#3: GET the same schema's /export?type=JSONSchemaDraft4. Observe 200 whose Content is not the published draft-4 document (definitions keyed by the event name, nullable rendered as a ["object","null"] union, no readOnly keyword).
4. Both steps must be re-replayed after the fix: task#3 converts the document task#2 serves, so fixing the conversion alone leaves task#2 failing and fixing the seed alone leaves task#3 failing.
