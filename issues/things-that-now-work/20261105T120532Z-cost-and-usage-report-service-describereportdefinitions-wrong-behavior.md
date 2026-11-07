Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: be6ee02eee1f74a4d76a707d845417a118b7d413

# DescribeReportDefinitions reports ReportStatus values the model (and this crate's own validator) rejects

_Filed: 2026-11-05T12:05:32Z_
_Operation: DescribeReportDefinitions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `be6ee02eee1f74a4d76a707d845417a118b7d413`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a report no delivery has been recorded against, `StoredReport::delivery_status` fills BOTH members of the modelled `ReportStatus` with the empty string, and `wire_definition` puts that on every `ReportDefinition` `DescribeReportDefinitions` answers with. The empty string is outside the constraints the model declares for both members:

* `ReportStatus$lastDelivery` targets `LastDelivery`, which carries `length {min: 16, max: 20}` and `pattern ^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$` — `""` is 0 characters and matches no part of that expression.
* `ReportStatus$lastStatus` targets the `LastStatus` enum, whose whole catalog is `SUCCESS | ERROR_PERMISSIONS | ERROR_NO_BUCKET` — `""` is not a member of it.

The same crate enforces those very constraints in the opposite direction: a request carrying a `ReportStatus.lastDelivery` outside the pattern, or a `lastStatus` outside the catalog, is refused with HTTP 400 `ValidationException` (pinned by this crate's own test table). So the definition the service hands back for a report created through `PutReportDefinition` is a definition the same service will not accept back: a client that reads a definition with `DescribeReportDefinitions` and replays it through `PutReportDefinition`/`ModifyReportDefinition` (the natural read-modify-write of this API, since `ModifyReportDefinition` replaces the definition wholesale) gets a 400 on a value it never invented. Both members are OPTIONAL in the model, so a never-delivered report has a representation that does not break its own shape's constraints; filling them with a value the shape forbids does not.

## Evidence

Model (smithy_query(shape="LastDelivery,LastStatus")):
- `"LastDelivery": {"type": "string", "length": {"min": 16, "max": 20}, "pattern": "^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$"}`
- `"LastStatus": {"type": "enum", "values": ["SUCCESS", "ERROR_PERMISSIONS", "ERROR_NO_BUCKET"]}`

Handler side — services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/state.rs:293-296:
```
            None => ReportStatus {
                last_delivery: Some(String::new()),
                last_status: Some(String::new()),
            },
```
reached on every read through src/state.rs:273 (`report_status: Some(self.delivery_status()),`) and src/ops/describe_report_definitions.rs:173-176 (`.map(crate::state::StoredReport::wire_definition)`).

The crate's opposite-direction rule, tests/all/modelled_constraints.rs:169-188:
```
        // ReportStatus$lastDelivery: length 16..20, pattern
        // ^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$.
...
        (
            "ReportStatus lastDelivery as an ISO-8601 instant, outside the pattern",
            definition_with("ReportStatus", json!({ "lastDelivery": "2024-01-01T00:00:00Z" })),
            false,
        ),
        (
            "ReportStatus lastStatus outside its catalog",
            definition_with("ReportStatus", json!({ "lastStatus": "PENDING" })),
            false,
        ),
```
(`false` = refused with `(400, "ValidationException")`, per the `REFUSED` constant at tests/all/modelled_constraints.rs:37.)

Test coverage of the current behaviour: tests/all/report_definition_lifecycle.rs:213-216 (`expected_read_back` inserts `json!({ "lastDelivery": "", "lastStatus": "" })`) and tests/all/service_populated_read_back.rs:48-50 (`never_delivered()`); the stated reason there is the `ReportStatus` doc "A two element dictionary with a lastDelivery and lastStatus key" — which argues the keys are present, not that their values may be outside the two members' own declared constraints.

## Steps to Reproduce

1. Start the emulator and call PutReportDefinition with ReportDefinition{ReportName:"demo-report", TimeUnit:"DAILY", Format:"textORcsv", Compression:"GZIP", AdditionalSchemaElements:["RESOURCES"], S3Bucket:"my-bucket", S3Prefix:"cur/", S3Region:"us-east-1"} — HTTP 200.
2. Call DescribeReportDefinitions and read the returned definition: `ReportStatus` comes back as {"lastDelivery": "", "lastStatus": ""} (src/state.rs:293-296), i.e. a lastDelivery of length 0 against the modelled length 16..20 / pattern ^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$, and a lastStatus that is not one of SUCCESS | ERROR_PERMISSIONS | ERROR_NO_BUCKET.
3. Take that returned definition unchanged and send it back as ModifyReportDefinition{ReportName:"demo-report", ReportDefinition:<the definition just read>} (or PutReportDefinition under a new name): the boundary validator refuses it with HTTP 400 ValidationException for ReportStatus.lastDelivery / ReportStatus.lastStatus — the same values the service itself just reported, as tests/all/modelled_constraints.rs:179-188 pins for out-of-constraint values of those members.
