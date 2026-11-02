Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cur.duplicate-name-put-rejected-leaves-definition-and-tags-unchanged cur.tag-operations-on-deleted-report-are-rejected-not-silently-accepted cur.two-independent-report-definitions-have-isolated-tag-and-delete-lifecycles
Issue-Shape-Id: com.amazonaws.costandusagereportservice#AWSOrigamiServiceGatewayService
Issue-Key: cur.duplicate-name-put-rejected-leaves-definition-and-tags-unchanged|run#16|AssertionError|assert message == "", f"expected an empty message, got {message!r}"
Issue-Key: cur.tag-operations-on-deleted-report-are-rejected-not-silently-accepted|run#13|AssertionError|assert message == "", f"expected an empty message, got {message!r}"
Issue-Key: cur.two-independent-report-definitions-have-isolated-tag-and-delete-lifecycles|run#23|AssertionError|assert message == "", f"expected an empty message, got {message!r}"
Issue-Commit: c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40

# cost-and-usage-report-service: 2 net-new causes — DuplicateReportNameException and ResourceNotFoundException are returned with an emulator-authored Message where CUR returns none

> _Note: this issue was identified against the code base at commit `c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutReportDefinition

## Description
The CUR emulator answers its client-side refusals with an Error.Message it composes from the Smithy error documentation. Real Cost and Usage Report Service returns these exceptions with the optional `Message` member ABSENT, so botocore reports `Error.Message == ""`. Three behavioral checks assert exactly that and all three fail only on the message; the error codes and the state effects (the rejected duplicate put leaves definition and tags untouched; the surviving report keeps its tags) are already correct.

Cause 1 of 2: PutReportDefinition under an existing report name returns DuplicateReportNameException with a non-empty Message.
  AWS: Smithy (com.amazonaws.costandusagereportservice, awsJson1_1): PutReportDefinition errors include DuplicateReportNameException, a client error with httpError 400 (defaulted) whose only member is the OPTIONAL `Message` (target ErrorMessage). The shape documentation — "A report with the specified name already exists in the account. Specify a different report name." — is the model's description of the error CLASS, not a message AWS puts on the wire. Recorded behavior only for the wire shape: the recording of the duplicate PutReportDefinition is HTTP 400 with `__type`/Code `DuplicateReportNameException` and NO `Message` member, i.e. `Error.Message` is the empty string at the client. The requirement is therefore: code `DuplicateReportNameException`, status 400, and no Message at all — there is no fixed text and no variable part, so nothing from the request (the report name) belongs in the response.
  Emulator: returns 400 / DuplicateReportNameException with Message = "A report with the name <ReportName> already exists in the account. Specify a different report name." — fixed text lifted verbatim from the model's shape doc with the request's ReportName spliced in as its single variable part.
  Why: a client that surfaces, logs or asserts on Error.Message for this refusal sees prose AWS never sends, and a client that distinguishes "service supplied a diagnostic" from "service supplied none" (a very common pattern when a code is shared across conditions) takes the wrong branch. It also leaks the caller's report name into an error body AWS keeps empty.
  Fix: the PutReportDefinition handler's duplicate-name refusal — construct DuplicateReportNameException with `message: None`.
  Traces: cost-and-usage-report-service.duplicate-name-put-rejected-leaves-definition-and-tags-unchanged (assertion: expected an empty message, got 'A report with the name sim2real-behav-cur-95a21216a719 already exists in the account. Specify a different report name.').

Cause 2 of 2: the tagging operations (TagResource, ListTagsForResource, UntagResource) return ResourceNotFoundException with a non-empty Message for a report name the account does not hold.
  AWS: Smithy: TagResource, UntagResource and ListTagsForResource each model ResourceNotFoundException, a client error with httpError 400 (defaulted) whose only member is the OPTIONAL `Message`; the shape doc "The specified report (ReportName) in the request doesn't exist." describes the error class. Recorded behavior only for the wire shape: all three calls against a deleted report name answer 400 with Code `ResourceNotFoundException` and NO Message member (`Error.Message == ""`). The requirement is the code and the 400 with no Message; no part of the request (the report name) is echoed.
  Emulator: returns 400 / ResourceNotFoundException with Message = "The report <ReportName> in the request doesn't exist." — the model's doc sentence re-worded, with the request's ReportName as the variable part.
  Why: same as cause 1 — clients asserting on or branching off the presence/content of Error.Message for a not-found tag call diverge from AWS, and the report name is echoed in a body AWS leaves empty. This affects all three tag-plane operations, not just the one the check reached first.
  Fix: the ResourceNotFoundException refusal in each of the TagResource / ListTagsForResource / UntagResource handlers (the not-found branch that follows the store lookup) — construct ResourceNotFoundException with `message: None`; better, route all three through one shared constructor in the crate's state/error helper so the three handlers cannot drift.
  Traces: cost-and-usage-report-service.tag-operations-on-deleted-report-are-rejected-not-silently-accepted (TagResource leg; assertion: expected an empty message, got "The report sim2real-behav-cur-e2afcd364813 in the request doesn't exist."), cost-and-usage-report-service.two-independent-report-definitions-have-isolated-tag-and-delete-lifecycles (ListTagsForResource leg; same message template with the deleted report B's name).

Note: no existing open issue covers this service (list_existing_issues found none matching 'cost-and-usage'), and neither failure was marked ALREADY FILED or OWNED by the replay report.

## Evidence
Check cost-and-usage-report-service.duplicate-name-put-rejected-leaves-definition-and-tags-unchanged:
  Expected (AWS): Code == "DuplicateReportNameException", Error.Message == "" (member absent)
  Actual (emulator): Code == "DuplicateReportNameException", Error.Message == "A report with the name sim2real-behav-cur-95a21216a719 already exists in the account. Specify a different report name."
  AssertionError: expected an empty message, got 'A report with the name sim2real-behav-cur-95a21216a719 already exists in the account. Specify a different report name.'

Check cost-and-usage-report-service.tag-operations-on-deleted-report-are-rejected-not-silently-accepted:
  Expected (AWS): for each of TagResource / ListTagsForResource / UntagResource on a deleted report: Code == "ResourceNotFoundException", Error.Message == ""
  Actual (emulator): Code == "ResourceNotFoundException", Error.Message == "The report sim2real-behav-cur-e2afcd364813 in the request doesn't exist."
  AssertionError: expected an empty message, got "The report sim2real-behav-cur-e2afcd364813 in the request doesn't exist."

Check cost-and-usage-report-service.two-independent-report-definitions-have-isolated-tag-and-delete-lifecycles:
  Expected (AWS): ListTagsForResource on the deleted report B: Code == "ResourceNotFoundException", Error.Message == ""
  Actual (emulator): Error.Message == "The report sim2real-behav-cur-b-091082e8d690 in the request doesn't exist."
  AssertionError: expected an empty message, got "The report sim2real-behav-cur-b-091082e8d690 in the request doesn't exist."

The recorded report names (sim2real-behav-cur-95a21216a719 etc.) are per-run values from the harness and are not part of the requirement; the requirement is that NO Message member is serialized for either error shape. Code and HTTP status already match, and every non-message assertion in all three checks passed (the rejected duplicate put left TimeUnit=DAILY and the tag {env: staging} intact; tagging report A did not tag B; deleting B left A and A's tags intact).

Source of the emulator messages (file/line citations):
  services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/put_report_definition.rs:162-172 (DuplicateReportNameException { message: Some(format!(...)) })
  services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/tag_resource.rs:156-166 (ResourceNotFoundException { message: Some(format!("The report {} in the request doesn't exist.", ...)) }); the same construction appears in the list_tags_for_resource and untag_resource handlers.

Smithy citations:
  DuplicateReportNameException: errorKind client, httpError 400 (defaulted), members { Message: ErrorMessage } (optional).
  ResourceNotFoundException: errorKind client, httpError 400 (defaulted), members { Message: ErrorMessage } (optional).
  PutReportDefinition errors: DuplicateReportNameException, ReportLimitReachedException, ResourceNotFoundException, ValidationException.
  TagResource / UntagResource / ListTagsForResource errors: ResourceNotFoundException, ValidationException.

## Steps to Reproduce
Cause 1 (DuplicateReportNameException):
1. Create an S3 bucket and attach the billingreports.amazonaws.com bucket policy (GetBucketAcl/GetBucketPolicy + PutObject, conditioned on aws:SourceAccount and arn:aws:cur:us-east-1:<account>:definition/*).
2. cur = client("cur", region="us-east-1"); call PutReportDefinition with ReportDefinition {ReportName: R, TimeUnit: DAILY, Format: textORcsv, Compression: GZIP, AdditionalSchemaElements: [RESOURCES], S3Bucket, S3Prefix, S3Region}.
3. TagResource(ReportName=R, Tags=[{env: staging}]).
4. Call PutReportDefinition again with the SAME ReportName R but TimeUnit=MONTHLY.
5. Inspect the ClientError: Error.Code is DuplicateReportNameException (correct) but Error.Message is non-empty; AWS returns no Message. (The definition and tags are correctly left unchanged.)

Cause 2 (ResourceNotFoundException on the tag plane):
1. Same bucket/policy setup; PutReportDefinition for report R, then DeleteReportDefinition(ReportName=R).
2. Call each of TagResource(ReportName=R, Tags=[{env: x}]), ListTagsForResource(ReportName=R), UntagResource(ReportName=R, TagKeys=["env"]).
3. Each raises ResourceNotFoundException (correct) but with a non-empty Error.Message; AWS returns no Message.
4. Equivalent path: create reports A and B, DeleteReportDefinition(B), then ListTagsForResource(ReportName=B) — same non-empty message.
