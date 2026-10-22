Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pi.list-reports-shows-running-status-immediately-after-create pi.cross-service-type-mismatch-rejected pi.tag-resource-visible-in-list-tags-for-report pi.untag-resource-removes-tag-for-report
Issue-Shape-Id: com.amazonaws.pi#PerformanceInsightsv20180227
Issue-Key: pi.list-reports-shows-running-status-immediately-after-create|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: pi.cross-service-type-mismatch-rejected|run#5|AssertionError|assert code == "InvalidArgumentException", (
Issue-Key: pi.tag-resource-visible-in-list-tags-for-report|run#12|AssertionError|assert code2 == "NotAuthorizedException", (
Issue-Key: pi.untag-resource-removes-tag-for-report|run#11|AssertionError|assert code2 == "NotAuthorizedException", (
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# PI control plane: ServiceType is never cross-checked against the Identifier's namespace, and GetResourceMetadata / ListTagsForResource / UntagResource answer 501 where AWS answers a modeled 400 (4 causes)

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPerformanceAnalysisReports

## Description
Four distinct root causes in the Performance Insights control plane, all exercised against the shared PI-enabled RDS instance (DbiResourceId db-..., engine mysql). Each cause must be closed before this issue is resubmitted.

Cause 1 of 4: ListPerformanceAnalysisReports does not reject an RDS DbiResourceId queried under ServiceType=DOCDB.
AWS: Smithy ListPerformanceAnalysisReports requires ServiceType (enum RDS | DOCDB) and Identifier (IdentifierString, pattern ^[a-zA-Z0-9-]+$); InvalidArgumentException (HTTP 400) is modeled on the operation ("One of the arguments provided is invalid for this request"). An identifier belonging to RDS's namespace queried under ServiceType=DOCDB is refused with InvalidArgumentException, HTTP 400 (recorded message example: 'The specified ServiceType is invalid for engine mysql.'). The requirement is the 400 + InvalidArgumentException; the text naming the engine is an example.
Emulator: the call is ACCEPTED and answered HTTP 200 with {'AnalysisReports': []}. The gate the handler applies, state::is_data_source_identifier, only tests the identifier's PREFIX and alphabet ('db-'/'cluster-' plus uppercase/digits) and is ServiceType-blind, so an RDS identifier passes under DOCDB.
Why: "wrong service namespace" is indistinguishable from "this resource has no reports" — a client that passes the wrong ServiceType (a very common copy/paste error between RDS and DocDB) silently sees an empty list instead of the argument error, and negative tests asserting the rejection pass vacuously.
Fix: the (ServiceType, Identifier) validation shared by this crate's handlers (state::is_data_source_identifier and its callers) — decide the identifier's owning namespace and refuse a mismatch with the modeled InvalidArgumentException.
Traces: pi.list-reports-shows-running-status-immediately-after-create.

Cause 2 of 4: GetResourceMetadata cannot perform that same namespace rejection because its handler is an unimplemented scaffold stub answering 501.
AWS: Smithy GetResourceMetadata(ServiceType required, Identifier required) returns Identifier + Features, and models InvalidArgumentException (HTTP 400). An RDS DbiResourceId under ServiceType=DOCDB is refused with InvalidArgumentException, HTTP 400 (recorded message example: 'The specified ServiceType is invalid for engine mysql.'); under its real ServiceType=RDS the same identifier is echoed back in Identifier.
Emulator: error code NotImplemented (HTTP 501) for the DOCDB call, so neither the rejection nor the positive control is observable.
Why: a client gets a non-AWS error code for every GetResourceMetadata call and cannot discover feature metadata or validate its ServiceType wiring.
Fix: implement the GetResourceMetadata handler, including the ServiceType-vs-identifier-namespace refusal from Cause 1. NOTE: a separate already-filed issue covers the bare 'GetResourceMetadata is an unimplemented stub' (raised from pi.get-resource-metadata-reflects-sql-digest-feature); the net-new requirement recorded HERE is the cross-namespace InvalidArgumentException, which that stub issue does not state.
Traces: pi.cross-service-type-mismatch-rejected.

Cause 3 of 4: ListTagsForResource answers 501 for a request that passes modeled shape validation.
AWS: Smithy ListTagsForResource requires ServiceType and ResourceARN (AmazonResourceName, len 1..1011, pattern ^arn:.*:pi:.*$) and models InvalidArgumentException and NotAuthorizedException (both HTTP 400). Two independent gates are observable: an ARN that fails the pattern is refused by shape validation with ValidationException HTTP 400 (the emulator already does this correctly), and a well-formed ARN naming a report that was never issued is then refused at the existence/authorization stage with NotAuthorizedException, HTTP 400 (recorded message example: 'The specified resource is not authorized for this account' — PI deliberately returns one uniform sentence so it discloses nothing about existence).
Emulator: the second stage returns error code NotImplemented (HTTP 501). The handler is the generated stub (services/.../src/ops/list_tags_for_resource.rs carries the 'SCAFFOLDED, NOT IMPLEMENTED' marker and handle() returns Scaffolded::not_implemented).
Why: tags on PI analysis reports cannot be read at all, and the two-stage validation contract collapses — a client cannot tell a malformed ARN from an unauthorized/absent resource because the second case is a 501.
Fix: implement the ListTagsForResource handler against state.tags_by_arn, returning NotAuthorizedException (with the service-wide state::NOT_AUTHORIZED_MESSAGE) for an ARN that names no report this account holds.
Traces: pi.tag-resource-visible-in-list-tags-for-report.

Cause 4 of 4: UntagResource answers 501 for a request that passes modeled shape validation.
AWS: Smithy UntagResource requires ServiceType, ResourceARN (pattern ^arn:.*:pi:.*$) and TagKeys (TagKeyList, len 0..200), and models InvalidArgumentException and NotAuthorizedException (HTTP 400). TagKeys over the 200-entry bound is refused by shape validation with ValidationException HTTP 400 (the emulator already does this correctly); an in-range TagKeys list on a well-formed ARN that names no existing report is then refused with NotAuthorizedException, HTTP 400 (recorded message example: 'The specified resource is not authorized for this account').
Emulator: the in-range call returns error code NotImplemented (HTTP 501). The handler is the generated stub (services/.../src/ops/untag_resource.rs, 'SCAFFOLDED, NOT IMPLEMENTED').
Why: tags cannot be removed from a PI resource, and as in Cause 3 the later authorization gate is unreachable, so a client sees 501 where AWS gives a modeled 400.
Fix: implement the UntagResource handler against state.tags_by_arn, with the same NotAuthorizedException refusal for an ARN naming no report this account holds.
Traces: pi.untag-resource-removes-tag-for-report.

## Evidence
Cause 1 — pi.list-reports-shows-running-status-immediately-after-create:
  call:     pi.list_performance_analysis_reports(ServiceType='DOCDB', Identifier='db-…')
  expected: ClientError code InvalidArgumentException (HTTP 400), e.g. message 'The specified ServiceType is invalid for engine mysql.'
  actual:   HTTP 200 {'AnalysisReports': []}
  assertion: "expected the call to be rejected, but it succeeded and returned {'AnalysisReports': [], ...}"
  source: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/state.rs:317-326 (is_data_source_identifier ignores ServiceType entirely); list_performance_analysis_reports.rs:8-11 documents it as the only gate.

Cause 2 — pi.cross-service-type-mismatch-rejected:
  call:     pi.get_resource_metadata(ServiceType='DOCDB', Identifier='db-…')
  expected: InvalidArgumentException (HTTP 400); positive control pi.get_resource_metadata(ServiceType='RDS', …) echoes Identifier
  actual:   error code 'NotImplemented' (HTTP 501)
  source: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/get_resource_metadata.rs (SCAFFOLDED, NOT IMPLEMENTED)

Cause 3 — pi.tag-resource-visible-in-list-tags-for-report:
  stage 1 (passes): list_tags_for_resource(ResourceARN='not-a-pi-arn-at-all') -> ValidationException, HTTP 400
  stage 2 (fails):  list_tags_for_resource(ResourceARN='arn:aws:pi:<region>:<account>:perf-reports/rds/db-…/report-00000000000000000')
  expected: NotAuthorizedException (HTTP 400), e.g. message 'The specified resource is not authorized for this account'
  actual:   error code 'NotImplemented' (HTTP 501)
  source: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/list_tags_for_resource.rs:4 and :163

Cause 4 — pi.untag-resource-removes-tag-for-report:
  stage 1 (passes): untag_resource(TagKeys=[201 keys]) -> ValidationException, HTTP 400
  stage 2 (fails):  untag_resource(ServiceType='RDS', ResourceARN=<well-formed perf-reports ARN>, TagKeys=['sim2real-key-0'])
  expected: NotAuthorizedException (HTTP 400), e.g. message 'The specified resource is not authorized for this account'
  actual:   error code 'NotImplemented' (HTTP 501)
  source: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/untag_resource.rs:4

## Steps to Reproduce
Preconditions: an RDS mysql instance with EnablePerformanceInsights=true and DatabaseInsightsMode=advanced, available; take its DbiResourceId as DBI; ACCOUNT from sts.get_caller_identity().

Cause 1: pi.list_performance_analysis_reports(ServiceType='DOCDB', Identifier=DBI) -> emulator answers HTTP 200 {'AnalysisReports': []}; AWS answers HTTP 400 InvalidArgumentException. Then pi.list_performance_analysis_reports(ServiceType='RDS', Identifier=DBI) must still succeed with a list.

Cause 2: pi.get_resource_metadata(ServiceType='DOCDB', Identifier=DBI) -> emulator answers 501 NotImplemented; AWS answers HTTP 400 InvalidArgumentException. Then pi.get_resource_metadata(ServiceType='RDS', Identifier=DBI) must echo Identifier == DBI.

Cause 3: pi.list_tags_for_resource(ServiceType='RDS', ResourceARN='not-a-pi-arn-at-all') -> 400 ValidationException (already correct). Then pi.list_tags_for_resource(ServiceType='RDS', ResourceARN=f'arn:aws:pi:<region>:{ACCOUNT}:perf-reports/rds/{DBI}/report-00000000000000000') -> emulator answers 501 NotImplemented; AWS answers HTTP 400 NotAuthorizedException.

Cause 4: pi.untag_resource(ServiceType='RDS', ResourceARN=<that same ARN>, TagKeys=[f'k-{i}' for i in range(201)]) -> 400 ValidationException (already correct). Then pi.untag_resource(ServiceType='RDS', ResourceARN=<that same ARN>, TagKeys=['k-0']) -> emulator answers 501 NotImplemented; AWS answers HTTP 400 NotAuthorizedException.
