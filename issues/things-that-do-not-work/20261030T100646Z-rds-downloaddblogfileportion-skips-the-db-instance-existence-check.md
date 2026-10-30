Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-core-lifecycle
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-core-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DownloadDBLogFilePortion|4xx->2xx|$.ErrorResponse
Issue-Key: rds|DownloadDBLogFilePortion|4xx->2xx|status
Issue-Commit: 456b27b09fa8a3c140813b137ed67b93b9b1fa71

# RDS DownloadDBLogFilePortion skips the DB instance existence check and answers 200

> _Note: this issue was identified against the code base at commit `456b27b09fa8a3c140813b137ed67b93b9b1fa71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DownloadDBLogFilePortion

## Description
DownloadDBLogFilePortion never resolves the requested DBInstanceIdentifier against stored state, so a request naming a DB instance that does not exist (here, one that the fixture had already deleted earlier in the same lifecycle) is served as a successful log read instead of being refused.

Cause 1 of 1: DownloadDBLogFilePortion on a non-existent DB instance returns 200 instead of 404 DBInstanceNotFound (fixture rds-db-instance-core-lifecycle, step task#9).

AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19) models DownloadDBLogFilePortion with required input member DBInstanceIdentifier and the error DBInstanceNotFoundFault, errorKind client, httpError 404, awsQueryError code DBInstanceNotFound / httpResponseCode 404 (the operation also models DBInstanceNotReadyFault 400 and DBLogFileNotFoundFault 404). AWS therefore resolves the instance FIRST and, when no DB instance with that identifier exists in the account and region, answers HTTP 404 with the query-protocol ErrorResponse envelope: Error.Type = Sender, Error.Code = DBInstanceNotFound, and Error.Message of the form "DBInstance <DBInstanceIdentifier> not found." In that message the fixed text is "DBInstance " and " not found."; the only variable part is the DBInstanceIdentifier copied verbatim from the request (in the recording that value was the probe instance identifier). RequestId is a per-run value and is not part of the requirement.

Emulator: the replay shows HTTP 200 with no ErrorResponse element at all (`status: expected 404, got 200`; `$.ErrorResponse: missing`), i.e. the handler produced a DownloadDBLogFilePortionResponse for an identifier it holds no instance for.

Why: a client that reads DB logs relies on the 404/DBInstanceNotFound refusal to detect a deleted or mistyped instance. Against this emulator the call succeeds, so error-handling and retry/cleanup branches are never taken, a deleted instance appears to still be readable, and log-tailing loops silently consume an empty/synthetic log instead of surfacing the failure.

Fix: the DownloadDBLogFilePortion handler in the rds service crate — add the instance lookup as the first step of the handler (before any log-file or Marker handling) and raise the modeled DBInstanceNotFoundFault (404, Sender/DBInstanceNotFound) with RDS's wording when the identifier resolves to nothing; the DBLogFileNotFoundFault/DBInstanceNotReadyFault checks follow it.

Related open issues that do NOT cover this step: "RDS accepts nine requests AWS refuses: missing request and state preconditions" (20261029T214632Z-rds-accepts-nine-requests-aws-refuses-missing-request.md) and "RDS not-found refusals for instances, clusters, subnet and security groups use emulator wording" (20261029T214632Z-rds-not-found-refusals-for-instances-clusters-subnet.md) — the replay reports this cluster as unowned by either, and here the emulator emits no refusal at all rather than a mis-worded one.

Traces: rds-db-instance-core-lifecycle#task#9.

## Evidence
Replay cluster divergence_key=service=rds|DownloadDBLogFilePortion|status=404->200|keys=$.ErrorResponse,status; verdict=mismatch; affected=[rds-db-instance-core-lifecycle#task#9].

Expected (recorded AWS): HTTP 404, body ErrorResponse { Error: { Type: "Sender", Code: "DBInstanceNotFound", Message: "DBInstance <DBInstanceIdentifier> not found." }, RequestId: <per-run> }.

Actual (emulator): HTTP 200, $.ErrorResponse missing entirely.

Model evidence: DownloadDBLogFilePortion errors include DBInstanceNotFoundFault -> httpError 404, awsQueryError {code: DBInstanceNotFound, httpResponseCode: 404}; input DBInstanceIdentifier is required.

## Steps to Reproduce
1. Replay fixture rds-db-instance-core-lifecycle against the emulator.
2. At step task#9 the fixture calls DownloadDBLogFilePortion with a DBInstanceIdentifier that no longer exists (the instance was deleted earlier in the fixture).
3. Observe the emulator answers HTTP 200 with a success envelope; AWS answers HTTP 404 ErrorResponse with Error.Type=Sender, Error.Code=DBInstanceNotFound and Message "DBInstance <identifier> not found."
4. Equivalent minimal repro: call DownloadDBLogFilePortion with DBInstanceIdentifier set to any identifier never created in the emulator and LogFileName set to any value.
