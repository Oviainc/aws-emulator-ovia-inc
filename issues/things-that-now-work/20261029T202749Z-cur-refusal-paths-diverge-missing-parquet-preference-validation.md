Issue-Source: trace-sweep-divergence
Issue-Trace: cur-create-describe-modify-roundtrip
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cur-create-describe-modify-roundtrip cur-additional-artifacts-schema-versioning-and-name-pattern-errors cur-delete-idempotency-and-post-delete-error-propagation cur-idempotent-put-vs-duplicate-name-conflict cur-tag-untag-list-lifecycle
Issue-Shape-Id: com.amazonaws.costandusagereportservice#AWSOrigamiServiceGatewayService
Issue-Key: cur|ModifyReportDefinition|4xx->2xx|$
Issue-Key: cur|ModifyReportDefinition|4xx->2xx|status
Issue-Key: cur|PutReportDefinition|4xx->4xx|$.message
Issue-Key: cur|ModifyReportDefinition|4xx->4xx|$.Message
Issue-Key: cur|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: cur|PutReportDefinition|4xx->4xx|$.Message
Issue-Key: cur|TagResource|4xx->4xx|$.Message
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# cur refusal paths diverge: missing Parquet-preference validation and emulator-authored ValidationException/ResourceNotFoundException message text

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyReportDefinition

## Description
Six net-new root causes on the CUR emulator's REFUSAL paths: one refusal AWS makes is missing entirely (the request is accepted and mutates state), one refusal carries a different message template than AWS's, one framework-rendered constraint message quotes the regex with its anchors, and three refusals carry an emulator-authored Message where AWS serializes none. Error codes and HTTP statuses are otherwise correct.

Cause 1 of 6: ModifyReportDefinition accepts a Parquet-inconsistent report definition that AWS refuses with ValidationException (400 answered as 200).
  AWS: Smithy (awsJson1_1) ModifyReportDefinition { ReportName, ReportDefinition } -> ModifyReportDefinitionResponse, errors [ValidationException (client, httpError 400 defaulted, member Message optional), InternalErrorException]. The model states no cross-member rule, so recorded behavior only for the rule itself: AWS validates the report's Parquet preference as a unit -- Format and Compression must BOTH be Parquet, and a Parquet report may not also subscribe to the QuickSight/Redshift additional artifacts. A definition that mixes them is refused HTTP 400, __type ValidationException, with the FIXED message text "For valid Parquet preference, report format and compression must be 'Parquet' and cannot subscribe to QuickSight and/or Redshift" -- the whole string is fixed, there are no variable parts taken from the request.
  Emulator: ModifyReportDefinition has no cross-member validation at all. services/.../src/ops/modify_report_definition.rs:155-185 checks only that the named report exists and that a rename does not collide, then does `store.reports[at].definition = input.report_definition` and answers 200 with an empty body. The invalid Parquet combination is therefore PERSISTED and visible to every later DescribeReportDefinitions.
  Why: a client that relies on the service to reject an unusable report configuration gets a success, and the stored definition is one AWS would never hold -- a report that can never be delivered. Code written against AWS catches ValidationException here; against the emulator the call returns 200 and the state is silently corrupted, so the error-handling path is never exercised and the later read-backs disagree with AWS. PutReportDefinition shares the same input shape and almost certainly needs the same gate, but only the modify path is evidenced by this corpus.
  Fix: ModifyReportDefinition's validation (the cross-member check that must run before the store write), ideally a shared validator over ReportDefinition reached by both PutReportDefinition and ModifyReportDefinition so the two cannot drift.
  Traces: cur-create-describe-modify-roundtrip#task#7 (representative; expected 400 + ValidationException, got 200 + {}).

Cause 2 of 6: the ValidationException message for a reportName that fails its @pattern quotes the regex WITH the ^ and $ anchors.
  AWS: Smithy ReportName is @length(0..256) and @pattern "^[0-9A-Za-z!\\-_.*\\'()]+$" ("The name must be unique, is case sensitive, and can't include spaces"). AWS's framework refusal for a constraint violation is HTTP 400 / ValidationException whose message follows the standard template "<N> validation error detected: Value '<supplied value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" -- fixed text as quoted, with the count, the supplied value, the lowerCamel member path (reportDefinition.reportName) and the pattern as its variable parts. The recorded pattern rendering is the UNANCHORED body [0-9A-Za-z!\\-_.*\\'()]+ : AWS quotes the expression without the leading ^ and trailing $.
  Emulator: emits the same template but renders the pattern straight from the @pattern trait value, anchors included: "... Member must satisfy regular expression pattern: ^[0-9A-Za-z!\\-_.*\\'()]+$".
  Why: the message is the only machine-readable description of WHICH constraint failed for this error class; a client (or test) that matches the pattern it reports against the one the service documents sees a string AWS never sends, and anything parsing the quoted expression gets two extra metacharacters. The divergence is systematic -- it is in the shared constraint renderer, so it affects every @pattern-constrained member of every service, not just reportName.
  Fix: the shared constraint-validation message renderer in the server runtime (the layer that builds the "N validation error detected ... Member must satisfy regular expression pattern" text; this crate reaches it through crate::scaffolded::Scaffolded::validation) -- strip a leading ^ and a trailing $ when quoting a @pattern value, leaving the rest of the expression untouched.
  Traces: cur-additional-artifacts-schema-versioning-and-name-pattern-errors#task#5 (cluster also covers #task#6).

Cause 3 of 6: ModifyReportDefinition's not-found refusal uses a different message template than AWS's.
  AWS: recorded behavior only (the model gives ValidationException one optional Message and no text). Modifying a report name the account does not hold is refused HTTP 400 / ValidationException with the message template "The report name [<ReportName>] and report type [<ReportType>] to be update doesn't exist in account [<AccountId>]" -- fixed text as spelled (including the bracketed placeholders and the "to be update" wording), with three variable parts: the ReportName from the request, the report type (CUR), and the caller's 12-digit account id.
  Emulator: services/.../src/ops/modify_report_definition.rs:155-165 returns ValidationException with "The report <ReportName> in the request doesn't exist." -- a re-wording of the ResourceNotFoundException shape doc, naming neither the report type nor the account.
  Why: the status and code are right but the diagnostic is text AWS never sends; a client that greps the message to distinguish "no such report" from the other ValidationException conditions this operation multiplexes (invalid Parquet preference, rename collision, constraint failures) cannot do so, since AWS's only discriminator here IS the message text.
  Fix: the ModifyReportDefinition handler's not-found branch -- build the message from AWS's template, filling the report name from the request, the report type, and the caller's account id from the request context.
  Traces: cur-delete-idempotency-and-post-delete-error-propagation#task#7.

Cause 4 of 6: ListTagsForResource's ResourceNotFoundException carries a Message where AWS serializes none.
  AWS: Smithy ListTagsForResource errors [ResourceNotFoundException, ValidationException]; ResourceNotFoundException is a client error, httpError 400 (defaulted), whose only member is the OPTIONAL Message. Its shape doc ("The specified report (ReportName) in the request doesn't exist.") describes the error CLASS, not wire text. Recorded behavior only for the body: the call against a deleted report answers 400 with __type ResourceNotFoundException and NO Message member at all. The requirement is the code plus the 400 with the Message member absent -- no fixed text, no variable part, and in particular no echo of the report name.
  Emulator: answers 400 / ResourceNotFoundException with Message = "The report <ReportName> in the request doesn't exist." (the shape doc re-worded, with the request's ReportName spliced in).
  Why: a client that branches on whether the service supplied a diagnostic takes the wrong branch, anything asserting Error.Message == "" fails, and the caller's report name is leaked into a body AWS leaves empty.
  Fix: the ListTagsForResource handler's not-found branch -- construct ResourceNotFoundException with message: None; better, route the tag plane's three not-found refusals through one shared constructor.
  Traces: cur-additional-artifacts-schema-versioning-and-name-pattern-errors#task#8 (cluster also covers cur-create-describe-modify-roundtrip#task#10, cur-delete-idempotency-and-post-delete-error-propagation#task#5, cur-idempotent-put-vs-duplicate-name-conflict#task#7, cur-tag-untag-list-lifecycle#task#12).

Cause 5 of 6: PutReportDefinition's DuplicateReportNameException carries a Message where AWS serializes none.
  AWS: Smithy PutReportDefinition errors [DuplicateReportNameException, ReportLimitReachedException, ResourceNotFoundException, ValidationException]; DuplicateReportNameException is a client error, httpError 400 (defaulted), only member the OPTIONAL Message, and its doc sentence is a class description. Recorded behavior only for the body: a put under a name the account already holds answers 400 with __type DuplicateReportNameException and NO Message member. Requirement: that code and status with no Message -- nothing from the request is echoed.
  Emulator: services/.../src/ops/put_report_definition.rs:162-172 returns Message = "A report with the name <ReportName> already exists in the account. Specify a different report name." -- the model's shape doc verbatim with the request's ReportName spliced in.
  Why: same as cause 4 -- a diagnostic AWS never sends, a wrong branch for clients that test for its presence, and the report name leaked into an empty body.
  Fix: the PutReportDefinition handler's duplicate-name refusal -- DuplicateReportNameException { message: None }.
  Traces: cur-idempotent-put-vs-duplicate-name-conflict#task#4.

Cause 6 of 6: TagResource's ResourceNotFoundException carries a Message where AWS serializes none.
  AWS: Smithy TagResource errors [ResourceNotFoundException, ValidationException]; same shape as cause 4. Recorded behavior only: tagging a report the account does not hold answers 400 / ResourceNotFoundException with NO Message member.
  Emulator: services/.../src/ops/tag_resource.rs returns Message = "The report <ReportName> in the request doesn't exist."
  Why: same as cause 4, on the write leg of the tag plane. (UntagResource shares the construction and will diverge identically, though this corpus evidences the TagResource and ListTagsForResource legs.)
  Fix: the TagResource handler's not-found branch -- ResourceNotFoundException { message: None }, via the shared constructor proposed in cause 4.
  Traces: cur-tag-untag-list-lifecycle#task#8.

Relation to the one open CUR issue: 20261029T172101Z-cost-and-usage-report-service-2-net-new.md ("DuplicateReportNameException and ResourceNotFoundException are returned with an emulator-authored Message where CUR returns none") describes the same emulator fault as causes 4, 5 and 6, but it was filed from the BEHAVIORAL corpus and carries only check ids -- the replay report marked none of these wire clusters as owned_by it, so none of them has a wire replay key. They are filed here so the fix is gated on the fixture steps above; a fixer closing that issue should re-replay these steps too. Causes 1, 2 and 3 are not covered by any open issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; 54 steps, 32 passed, 22 mismatched, score 59.3%). Per cluster:

Cause 1 -- divergence_key service=cur|ModifyReportDefinition|status=400->200|keys=$,status (cur-create-describe-modify-roundtrip#task#7):
  Expected (AWS): status 400, body {'__type': 'ValidationException', 'Message': "For valid Parquet preference, report format and compression must be 'Parquet' and cannot subscribe to QuickSight and/or Redshift", 'Type': None}
  Actual (emulator): status 200, body {} (empty_body) -- and the invalid definition is stored.
  Fixed vs variable: the quoted message is entirely fixed text; nothing in it comes from the request.

Cause 2 -- divergence_key service=cur|PutReportDefinition|status=400->400|keys=$.message (cur-additional-artifacts-schema-versioning-and-name-pattern-errors#task#5, affected_count 2):
  Expected (AWS) $.message: "1 validation error detected: Value 'bad name with spaces' at 'reportDefinition.reportName' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9A-Za-z!\\-_.*\\'()]+"
  Actual (emulator) $.message: "1 validation error detected: Value 'bad name with spaces' at 'reportDefinition.reportName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z!\\-_.*\\'()]+$"
  Fixed vs variable: the template text and the member path reportDefinition.reportName are fixed; 'bad name with spaces' is the value from the request (any name violating the pattern reproduces it). The ONLY divergence is the leading ^ and trailing $ around the quoted pattern.

Cause 3 -- divergence_key service=cur|ModifyReportDefinition|status=400->400|keys=$.Message (cur-delete-idempotency-and-post-delete-error-propagation#task#7):
  Expected (AWS) $.Message: "The report name [<ReportName>] and report type [CUR] to be update doesn't exist in account [<AccountId>]" (recorded: report name sim2real-probe-cur-7167ce35, account 000000000000)
  Actual (emulator) $.Message: "The report <ReportName> in the request doesn't exist."
  Fixed vs variable: fixed text is "The report name [", "] and report type [", "] to be update doesn't exist in account [", "]"; variable parts are the ReportName from the request, the report type CUR, and the caller's account id (the recorded sim2real-probe-cur-7167ce35 / 000000000000 are per-run values, not requirements).

Cause 4 -- divergence_key service=cur|ListTagsForResource|status=400->400|keys=$.Message (cur-additional-artifacts-schema-versioning-and-name-pattern-errors#task#8, affected_count 5):
  Expected (AWS) $.Message: absent (None)
  Actual (emulator) $.Message: "The report <ReportName> in the request doesn't exist." (unexpected_present)

Cause 5 -- divergence_key service=cur|PutReportDefinition|status=400->400|keys=$.Message (cur-idempotent-put-vs-duplicate-name-conflict#task#4):
  Expected (AWS) $.Message: absent (None)
  Actual (emulator) $.Message: 'A report with the name <ReportName> already exists in the account. Specify a different report name.' (unexpected_present)

Cause 6 -- divergence_key service=cur|TagResource|status=400->400|keys=$.Message (cur-tag-untag-list-lifecycle#task#8):
  Expected (AWS) $.Message: absent (None)
  Actual (emulator) $.Message: "The report <ReportName> in the request doesn't exist." (unexpected_present)

For causes 4-6 the status (400) and the error code (__type) already match; the requirement is the ABSENCE of the Message member, and the report names in the emulator's text are per-run request values, not part of any requirement.

Source lines:
  services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/modify_report_definition.rs:155-185 (not-found message; no Parquet/cross-member validation before `store.reports[at].definition = input.report_definition`)
  services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/put_report_definition.rs:162-172 (DuplicateReportNameException { message: Some(..) })
  services/service-com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice/src/ops/tag_resource.rs (ResourceNotFoundException { message: Some(..) }; same construction in list_tags_for_resource.rs and untag_resource.rs)

Smithy citations: ModifyReportDefinition errors [ValidationException, InternalErrorException]; ValidationException and ResourceNotFoundException and DuplicateReportNameException are all errorKind client, httpError 400 (defaulted), single OPTIONAL member Message -> ErrorMessage; ReportName @length(0..256) @pattern ^[0-9A-Za-z!\\-_.*\\'()]+$.

## Steps to Reproduce
Cause 1 (missing Parquet validation):
1. Point a cur client at the emulator; PutReportDefinition for report R with Format textORcsv, Compression ZIP, AdditionalArtifacts [REDSHIFT].
2. ModifyReportDefinition(ReportName=R, ReportDefinition=<R's definition with a Parquet-inconsistent preference: Format/Compression not both 'Parquet' while the report still subscribes to the Redshift/QuickSight artifacts>).
3. Emulator answers 200 with {} and stores the definition; AWS answers 400 ValidationException "For valid Parquet preference, report format and compression must be 'Parquet' and cannot subscribe to QuickSight and/or Redshift".
4. DescribeReportDefinitions() then shows the invalid definition. Replay: cur-create-describe-modify-roundtrip task#7.

Cause 2 (anchored pattern in the constraint message):
1. PutReportDefinition with ReportDefinition.ReportName = 'bad name with spaces' (any value violating ^[0-9A-Za-z!\\-_.*\\'()]+$).
2. Both answer 400, but compare $.message: the emulator quotes the pattern with ^ ... $, AWS without. Replay: cur-additional-artifacts-schema-versioning-and-name-pattern-errors task#5 (and task#6).

Cause 3 (ModifyReportDefinition not-found text):
1. PutReportDefinition for R, then DeleteReportDefinition(ReportName=R).
2. ModifyReportDefinition(ReportName=R, ReportDefinition=<any valid definition>).
3. 400 ValidationException in both, but the emulator's Message is "The report R in the request doesn't exist." where AWS sends "The report name [R] and report type [CUR] to be update doesn't exist in account [<account>]". Replay: cur-delete-idempotency-and-post-delete-error-propagation task#7.

Causes 4 and 6 (tag-plane not-found Message):
1. PutReportDefinition for R, then DeleteReportDefinition(ReportName=R).
2. ListTagsForResource(ReportName=R) and TagResource(ReportName=R, Tags=[{env: staging}]).
3. Both raise 400 ResourceNotFoundException (correct) but with a non-empty Error.Message; AWS serializes no Message member. Replay: cur-additional-artifacts-schema-versioning-and-name-pattern-errors task#8 and cur-tag-untag-list-lifecycle task#8.

Cause 5 (duplicate-name Message):
1. PutReportDefinition for R, then PutReportDefinition again with the SAME ReportName R.
2. 400 DuplicateReportNameException (correct) with a non-empty Error.Message; AWS serializes no Message member. Replay: cur-idempotent-put-vs-duplicate-name-conflict task#4.
