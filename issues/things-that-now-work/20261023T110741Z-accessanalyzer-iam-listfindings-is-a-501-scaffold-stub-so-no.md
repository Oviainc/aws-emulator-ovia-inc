Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer__iam.obs-iam-3-external-trust-finding-resolves-when-trust-narrowed accessanalyzer__iam.obs-iam-3-service-principal-trust-yields-no-external-access-finding
Issue-Key: accessanalyzer__iam|ListFindings|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# ListFindings is a 501 scaffold stub, so no external-access finding can ever be read back

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFindings

## Description
One cause. (Service attribution note: the failing checks' leading token is the cross-service token `accessanalyzer__iam`; the diverging operation itself belongs to accessanalyzer, whose crate services/service-com-amazonaws-accessanalyzer-accessanalyzer is bootstrapped, as is iam's.)

Cause 1 of 1: ListFindings answers HTTP 501 `NotImplemented` for every request, so an analyzer's findings are unreadable and nothing an analyzer scan produces can be observed.

AWS: Smithy model (services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json) binds ListFindings to POST /finding, success 200, required output member `findings` (FindingsList) plus optional `nextToken`, input `analyzerArn` (required, pattern ^[^:]*:[^:]*:[^:]*:[^:]*:[^:]*:analyzer/.{1,255}$) with optional `filter` (FilterCriteriaMap), `sort`, `nextToken`, `maxResults`, and it is @paginated (items=findings). Its modeled errors are AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400 — there is no 501 in the contract. Recorded behavior: against an ACTIVE ACCOUNT analyzer, ListFindings with filter={'resource': {'eq': [<role arn>]}} returns 200 and, for a role whose trust policy names another account's root principal, one finding with status ACTIVE and principal {'AWS': '<external account id>'}; for a role trusting only a service principal (e.g. lambda.amazonaws.com) it returns 200 with an empty findings list.

Emulator: every ListFindings request returns HTTP 501 with error code `NotImplemented` and message "operation ListFindings is scaffolded but not implemented in this emulator". Both checks die on the first ListFindings call, so neither the positive control (external trust reported) nor the negative claim (service-principal trust NOT reported) can be evaluated.

Why: a client cannot enumerate external-access findings at all, and the 501 is outside the operation's modeled error set, so SDK retry/handling classifies it as a server fault rather than any documented condition. Pagination over `findings` (botocore paginator) also fails outright.

Fix: the operation's handler — services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/list_findings.rs (line 4 carries the SCAFFOLDED marker; line 171 is `Err(crate::scaffolded::Scaffolded::not_implemented("ListFindings"))`). It needs the findings projection out of the service's state, the FilterCriteriaMap filter (at least `resource.eq`), sort, and nextToken/maxResults pagination.

Traces: accessanalyzer__iam.obs-iam-3-external-trust-finding-resolves-when-trust-narrowed, accessanalyzer__iam.obs-iam-3-service-principal-trust-yields-no-external-access-finding.

## Evidence
Both checks [error]:

  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListFindings operation: operation ListFindings is scaffolded but not implemented in this emulator

Expected (recorded AWS): HTTP 200 with a `findings` list — one ACTIVE finding whose principal is {'AWS': '<external account id>'} for the cross-account-trust role, and an empty list for the service-principal-trust role.
Actual (emulator): HTTP 501, error code `NotImplemented`.

These two checks were NOT marked ALREADY FILED or OWNED in the suite report, and list_existing_issues has no open issue mentioning accessanalyzer, so no filed issue covers ListFindings.

Handler evidence: services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/list_findings.rs line 4 `//! SCAFFOLDED, NOT IMPLEMENTED`, line 171 `Err(crate::scaffolded::Scaffolded::not_implemented("ListFindings"))`.

## Steps to Reproduce
1. Boot the emulator; create two IAM roles — one whose AssumeRolePolicyDocument trusts another account's root principal, one whose trust names only a service principal such as lambda.amazonaws.com.
2. CreateAnalyzer(analyzerName=N, type='ACCOUNT') and poll GetAnalyzer until status ACTIVE.
3. Call ListFindings(analyzerArn=<arn>, filter={'resource': {'eq': [<role arn>]}}) (or paginate it).
4. Observe HTTP 501 / NotImplemented for every call. Real AWS returns 200 with findings (one ACTIVE finding naming the external account for the cross-account role, empty for the service-principal role).
