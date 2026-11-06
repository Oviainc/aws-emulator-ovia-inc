Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-application-saved-object-migration
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-application-saved-object-migration opensearch-describe-reserved-instance-offering-not-found opensearch-reserved-instance-offering-describe-and-purchase-errors
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|StartMigration|4xx->4xx|$.message
Issue-Key: es|DescribeReservedInstanceOfferings|4xx->4xx|$.message
Issue-Related: 20261029T190009Z-es-application-plane-mints-constant-ids-wrong-arns.md
Issue-Related: 20261029T190009Z-es-package-vpc-endpoint-tagging-and-reserved-instance.md
Issue-Commit: 7f3f2187a2100f71560b62bea9ff35bd72db691b

# es not-found refusals on the application-migration and reserved-instance-offering readers answer HTTP 409 with reworded messages

> _Note: this issue was identified against the code base at commit `7f3f2187a2100f71560b62bea9ff35bd72db691b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMigration

## Description
2 net-new root causes on the `es` service token (the OpenSearch/Elasticsearch control plane served on endpoint prefix `es`). Both are the same class of defect: a request naming a resource that does not exist is refused with the wrong HTTP status (409 instead of 404) AND with emulator-authored message text instead of the sentence AWS emits for that error class. Each cause below is a separate replay unit and must be re-replayed separately before this issue is closed.

RELATED OPEN WORK (does NOT cover these steps — none of them carries these replay keys): 20261029T190009Z-es-resourcenotfoundexception-answered-as-http-409-instead-of.md, 20261029T190009Z-es-missing-resource-reads-answer-the-wrong-error.md and 20261101T121508Z-opensearch-getmigration-s-not-found-refusal-names-the.md. The two steps below are reported as unowned clusters by the replay and are filed here.

Model note that applies to both causes: in the on-disk model ResourceNotFoundException is declared with @httpError 409 (Smithy: StartMigration errors ResourceNotFoundException httpError 409; DescribeReservedInstanceOfferings likewise). The RECORDINGS show AWS answering HTTP 404 for these refusals, so the wire status is 404 and the modeled 409 must not be used for it — serializing the modeled code verbatim is exactly what reproduces this defect.

Cause 1 of 2 — StartMigration naming an application that does not exist (fixture opensearch-application-saved-object-migration, step task#2).
AWS: Smithy: StartMigration POST /2021-01-01/opensearch/app-migrations, input applicationId (required) @pattern("^[a-z0-9]{3,30}$"), migrationOptions (required); errors ResourceNotFoundException, ConflictException, ValidationException, DisabledOperationException, AccessDeniedException. The recording shows AWS answering HTTP 404 with message `Application <applicationId> does not exist` — fixed text `Application ` + ` does not exist`, with the applicationId taken verbatim from the request.
Emulator: answers HTTP 409 and the message `Application does not exist` — the request's applicationId is dropped from the sentence entirely.
Why: two separate client breakages. A 409 is a conflict/retryable-state signal, so a client branching on status treats a permanently-absent application as a transient conflict and retries or reports the wrong condition instead of taking its not-found path; and the message loses the identifier, so logs and user-facing errors cannot say WHICH application was missing when a caller migrates several.
Fix: the StartMigration handler's not-found refusal — the status it maps ResourceNotFoundException to on the wire (404, not the modeled 409) and the message template, which must interpolate the requested applicationId.
Traces: opensearch-application-saved-object-migration#task#2.

Cause 2 of 2 — DescribeReservedInstanceOfferings filtered by an offeringId that does not exist (fixture opensearch-describe-reserved-instance-offering-not-found, step task#1).
AWS: Smithy: DescribeReservedInstanceOfferings GET /2021-01-01/opensearch/reservedInstanceOfferings, ReservedInstanceOfferingId @httpQuery("offeringId") @length(36,36) @pattern GUID; errors ResourceNotFoundException, ValidationException, DisabledOperationException, InternalException. The recording shows AWS answering HTTP 404 with the message `Offering id not found` — that sentence is ENTIRELY fixed text: AWS does not echo the requested offering id into it.
Emulator: answers HTTP 409 with `Reserved Instance offering not found: <offeringId>` — a reworded sentence that additionally appends the requested id AWS deliberately omits.
Why: same status breakage as cause 1 (a client cannot distinguish 'no such offering' from a conflict), and the message is emulator-authored prose, so any client or test asserting on AWS's `Offering id not found` text fails. A purchase flow that probes an offering before PurchaseReservedInstanceOffering takes the wrong branch here.
Fix: the DescribeReservedInstanceOfferings handler's unknown-offering refusal — the wire status for ResourceNotFoundException (404) and the fixed message text.
Traces: opensearch-describe-reserved-instance-offering-not-found#task#1 (the same cluster is reached again by opensearch-reserved-instance-offering-describe-and-purchase-errors#task#2).

The application name and offering GUID in the evidence are this recording's inputs; the requirements are the 404 status, the ResourceNotFoundException class and the message templates above (`Application <applicationId> does not exist`, with the id from the request; `Offering id not found`, fixed).

## Evidence
Cause 1 — opensearch-application-saved-object-migration#task#2, StartMigration: expected status 404, got 409; `$.message: value_mismatch (expected 'Application sim2realprobenoapp001 does not exist', got 'Application does not exist')`. Divergence key service=es|StartMigration|status=404->409|keys=$.message,status. 'sim2realprobenoapp001' is the applicationId this recording submitted — the requirement is that the submitted id appears in the sentence, not that literal value.

Cause 2 — opensearch-describe-reserved-instance-offering-not-found#task#1, DescribeReservedInstanceOfferings: expected status 404, got 409; `$.message: value_mismatch (expected 'Offering id not found', got 'Reserved Instance offering not found: 8912712f-2a4b-49bf-96df-744acaa6c4c5')`. Divergence key service=es|DescribeReservedInstanceOfferings|status=404->409|keys=$.message,status; affected also opensearch-reserved-instance-offering-describe-and-purchase-errors#task#2. The GUID is this recording's query value; AWS's message contains no id at all.

Neither cluster is marked stub-blocked or owned by an open issue in the replay report.

## Steps to Reproduce
1. Replay fixture opensearch-application-saved-object-migration step task#2 (POST /2021-01-01/opensearch/app-migrations with an applicationId that exists in no application) and check the response: HTTP 404, x-amzn-errortype ResourceNotFoundException, message `Application <applicationId> does not exist` with the submitted applicationId interpolated.
2. Replay fixture opensearch-describe-reserved-instance-offering-not-found step task#1 (GET /2021-01-01/opensearch/reservedInstanceOfferings?offeringId=<absent GUID>) and check the response: HTTP 404 and the exact message `Offering id not found`, with no id appended.
3. Re-replay opensearch-reserved-instance-offering-describe-and-purchase-errors step task#2, which reaches the same refusal, to confirm the fix covers that path too.
4. Both causes must pass before this issue is closed.
