Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 16daefbe1706062ff28b8f9cce774afe5469742b

# DeleteReportDefinition never populates ResponseMessage, the member the model documents as carrying the deletion outcome

_Filed: 2026-11-07T11:15:03Z_
_Operation: DeleteReportDefinition | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `16daefbe1706062ff28b8f9cce774afe5469742b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's `DeleteReportDefinitionResponse` has exactly one member, `ResponseMessage`, whose target shape `DeleteResponseMessage` is documented as "Whether the deletion was successful or not." The handler computes precisely that fact - it looks the name up in the store and knows whether it removed an entry - and then throws it away, hard-coding `response_message: None`, so the single output member the operation models is never serialized on either leg. Every DeleteReportDefinition call answers the same empty JSON object `{}`, whether it removed a report or found nothing to remove, which is exactly the one distinction the modelled member exists to convey. The handler's own doc states this outcome as intended ("nothing distinguishes the body of the delete that removed a report from the body of the one that found nothing to remove"), but the stated reason - "`ResponseMessage` is an OPTIONAL output member and the service serializes none" - is circular: optionality explains that absence is well-formed, not why the service declines to report the outcome its model says this member reports. The operation declares no not-found error (OPERATIONS.md:31 lists only `InternalErrorException` and `ValidationException`), so `ResponseMessage` is the ONLY channel the modelled output offers for the success/no-op distinction, and the emulator closes it.

## Evidence

Smithy, the member's own target shape (`smithy_query(query="doc", shape="DeleteResponseMessage")`):
  {"shape": "DeleteResponseMessage", "documentation": "<p>Whether the deletion was successful or not.</p>"}
and the output shape carrying it (`smithy_query(query="shape", shape="DeleteReportDefinitionResponse")`):
  "DeleteReportDefinitionResponse": {"type": "structure", "members": {"ResponseMessage": {"target": "DeleteResponseMessage"}}, "doc": "<p>If the action is successful, the service sends back an HTTP 200 response.</p>"}
The crate's own denormalized surface agrees - services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/OPERATIONS.md:30: "- **Output:** `ResponseMessage` (string)".

The handler computes the outcome and discards it - services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/delete_report_definition.rs:160-166:
```
        if let Some(at) = store.position(&input.report_name) {
            store.reports.remove(at);
        }

        Ok(DeleteReportDefinitionOutput {
            response_message: None,
        })
```
`store.position(...)` on line 160 IS the success/no-op answer, and nothing after line 162 uses it.

The same file's handler doc states the rule the model contradicts - src/ops/delete_report_definition.rs:140-143:
"/// The response is the empty object on both legs. `ResponseMessage` is an OPTIONAL
/// output member and the service serializes none, so nothing distinguishes the body of
/// the delete that removed a report from the body of the one that found nothing to
/// remove, and no part of the request is echoed back in it."

Test coverage that pins the always-empty body (the only expectations a fix touches):
- tests/all/service_populated_read_back.rs:506-510: `assert_eq!(body, json!({}), "case {case:?}: expected an empty body carrying no ResponseMessage, got {body:?}");` in `delete_answers_the_empty_object_on_both_legs`, whose table covers both legs ("a report the account holds" / "a name never created").
- tests/all/report_definition_lifecycle.rs:992-996 and :1021-1026: `assert_eq!(body, json!({}), "case delete: expected an empty body, got {body:?}")`, and the same for the repeated delete.
No test asserts anything about the VALUE of `ResponseMessage`, and no comment in the crate cites model text or a recording as the reason the member is withheld.

## Steps to Reproduce

1. Start the emulator and point a `cur` client at it (awsJson1_1, `POST /`, `X-Amz-Target: AWSOrigamiServiceGatewayService.<Operation>`).
2. PutReportDefinition with ReportDefinition{ReportName:"demo-report", TimeUnit:"DAILY", Format:"textORcsv", Compression:"GZIP", AdditionalSchemaElements:["RESOURCES"], S3Bucket:"my-bucket", S3Prefix:"cur/", S3Region:"us-east-1"} - expect HTTP 200.
3. DeleteReportDefinition(ReportName="demo-report") - the report is removed, but the body is `{}`: the modelled `ResponseMessage` member, documented as "Whether the deletion was successful or not.", is absent (src/ops/delete_report_definition.rs:164-166).
4. DeleteReportDefinition(ReportName="demo-report") again, and DeleteReportDefinition(ReportName="never-created") - both answer 200 with a byte-identical `{}`, so no client can tell the deletion from the no-op even though the handler knew the difference at src/ops/delete_report_definition.rs:160.
