Issue-Source: trace-sweep-divergence
Issue-Trace: iam-service-access-analysis-and-role-template-reports
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-service-access-analysis-and-role-template-reports
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.IsTruncated
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.Marker
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.ServicesLastAccessed.member[*]
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.ServicesLastAccessed.member[*].ServiceName
Issue-Key: iam|GetServiceLastAccessedDetails|2xx->2xx|$.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.ServicesLastAccessed.member[*].ServiceNamespace
Issue-Commit: a7d3bb56a8b20ad6b0cfb258ffd31cc1523a40ca

# iam GetServiceLastAccessedDetails ignores MaxItems and serves a service-namespace list that is not AWS's

> _Note: this issue was identified against the code base at commit `a7d3bb56a8b20ad6b0cfb258ffd31cc1523a40ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetServiceLastAccessedDetails

## Description
1 net-new cause.

Cause 1 of 1: GetServiceLastAccessedDetails for a completed access-advisor job (fixture iam-service-access-analysis-and-role-template-reports, step task#4).
AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json) — GetServiceLastAccessedDetails input is JobId (required, @length 36..36), MaxItems (maxItemsType, @range 1..1000) and Marker (markerType, @length 1..320, @pattern ^[\u0020-\u00FF]+$); output is JobStatus (required), JobType, JobCreationDate (required), JobCompletionDate (required), ServicesLastAccessed (required, list of com.amazonaws.iam#ServiceLastAccessed), IsTruncated (booleanType, default false), Marker (responseMarkerType) and Error. ServiceLastAccessed requires ServiceName and ServiceNamespace (@length 1..64, @pattern ^[\w-]*$). Two requirements follow, and the recording exercises both. (a) The paging contract: when the report holds more entries than the caller's MaxItems, AWS returns exactly MaxItems entries, sets IsTruncated true and returns a Marker the caller feeds back to get the next page; it does not return the whole report in one response. (b) The report's content: the entry list is the account's access-advisor service set, ordered by ServiceNamespace, and ServiceName is AWS's display name for that namespace — the pair at each position is the service, so a position-by-position read is the documented way to consume the list. The model does not fix the namespace set, so for the set itself this is recorded behavior only: the recording captures the namespaces AWS reports and their display names. Status is 200 on both sides; no error code and no error message are involved (InvalidInputException/NoSuchEntityException are modeled for a bad or unknown JobId and are not what diverges).
Emulator: answers 200 but ignores the page size — it returns the entire report in a single response, so IsTruncated is false where AWS sets it true and Marker is absent entirely where AWS returns one, and the response carries members past the page boundary AWS stopped at (members at the index positions beyond the requested page are present in the emulator's body and absent from the recording). Independently, the namespace set it projects is not AWS's: two namespaces AWS does not report in this list ('amplifybackend' / "AWS Amplify Admin" and 'amplifyuibuilder' / "AWS Amplify UI Builder") are emitted ahead of 'aoss', which shifts every subsequent (ServiceNamespace, ServiceName) pair by two positions for the whole remainder of the list, and one display name differs from AWS's for a namespace both sides report ('apigateway' is "Manage - Amazon API Gateway" in the recording, "Amazon API Gateway" in the emulator).
Why: a client that pages this report drives the loop on IsTruncated/Marker. Getting IsTruncated false and no Marker on the first call, the loop exits after one iteration — which happens to be harmless only because the emulator over-serves; the same client code written against the emulator and then pointed at AWS silently processes only the first page and reports the account as never having accessed the services on every later page, which is precisely the wrong answer for an access-advisor-driven least-privilege prune (unused services look unused because they were never read). Conversely a client that asserts the page is at most MaxItems long, or that sizes a buffer or a UI page from it, is handed an oversized page. The content defect compounds it: a consumer that reads the list positionally gets the wrong service at every index from the first extra entry onward, and a consumer that matches on ServiceName sees a name AWS does not use, so a namespace-to-display-name mapping built from the emulator does not match AWS's.
Fix: the GetServiceLastAccessedDetails handler — it must apply MaxItems to ServicesLastAccessed, set IsTruncated when entries remain and emit a Marker that resumes at the next entry (the shape is already modeled on the output) — plus the service-namespace catalog its report generation projects, which must carry AWS's namespace set and AWS's display name per namespace.
Traces: iam-service-access-analysis-and-role-template-reports#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true; /__admin/health answered 200 and the data listener accepts). One cluster, verdict mismatch, expected status 200 / actual status 200 — status line, error code and error message are not involved. stub_path empty, owned_by empty.

GetServiceLastAccessedDetails, fixture iam-service-access-analysis-and-role-template-reports step task#4, diverging keys:
  $.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.IsTruncated: value_mismatch (expected 'true', got 'false')
  $.GetServiceLastAccessedDetailsResponse.GetServiceLastAccessedDetailsResult.Marker: missing (expected '<shaped-not-asserted>', got None)
  ServicesLastAccessed.member[11].ServiceNamespace: expected 'aoss', got 'amplifybackend'
  ServicesLastAccessed.member[11].ServiceName: expected 'Amazon OpenSearch Serverless', got 'AWS Amplify Admin'
  ServicesLastAccessed.member[12].ServiceNamespace: expected 'apigateway', got 'amplifyuibuilder'
  ServicesLastAccessed.member[12].ServiceName: expected 'Manage - Amazon API Gateway', got 'AWS Amplify UI Builder'
  ServicesLastAccessed.member[13].ServiceNamespace: expected 'appconfig', got 'aoss'
  ServicesLastAccessed.member[13].ServiceName: expected 'AWS AppConfig', got 'Amazon OpenSearch Serverless'
  ServicesLastAccessed.member[14].ServiceNamespace: expected 'appfabric', got 'apigateway'
  ServicesLastAccessed.member[14].ServiceName: expected 'AWS AppFabric', got 'Amazon API Gateway'
  ServicesLastAccessed.member[15].ServiceNamespace: expected 'appflow', got 'appconfig'
  ... the same two-position shift continues for every later index, and the cluster's key list additionally carries whole-member keys ServicesLastAccessed.member[200] through member[219] with no member suffix, i.e. entries at positions past the end of the recorded page.

Reading of that evidence: the two namespaces 'amplifybackend' and 'amplifyuibuilder' are emitted by the emulator at positions 11 and 12 and never appear in the recorded list, after which the emulator's entry at index n+2 is AWS's entry at index n for the rest of the list — one insertion of two catalog entries, not a per-entry corruption. The bare member[200..219] keys together with IsTruncated 'true' -> 'false' and the absent Marker are the paging defect: AWS closed the page and handed back a continuation token, the emulator ran past it. The exact namespace positions, the recorded page length and the generated JobId are values of this recording; the requirements are that MaxItems bounds the page, that IsTruncated/Marker are set when entries remain, and that each (ServiceNamespace, ServiceName) pair is AWS's namespace and AWS's display name for it.

## Steps to Reproduce
1. Clone this checkout and boot the emulator built from it.
2. Replay fixture iam-service-access-analysis-and-role-template-reports (it calls GenerateServiceLastAccessedDetails for the account/entity, waits for the job, then reads it back).
3. At step task#4 read the GetServiceLastAccessedDetails response: IsTruncated is false and no Marker is present where the recorded AWS response has IsTruncated true and a Marker, and ServicesLastAccessed is longer than the page AWS returned (members at the index positions past the recorded page end are present).
4. Compare ServicesLastAccessed positionally from index 11: the emulator emits 'amplifybackend' and 'amplifyuibuilder' where AWS has 'aoss' and 'apigateway', shifting every later pair by two; also compare ServiceName for namespace 'apigateway'.
5. Or directly: GenerateServiceLastAccessedDetails for the account, then GetServiceLastAccessedDetails with MaxItems well below the number of entries in the report and assert len(ServicesLastAccessed) <= MaxItems, IsTruncated true and Marker present; then page with that Marker until IsTruncated is false and check no entry is served twice or skipped.
