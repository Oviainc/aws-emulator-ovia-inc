Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer.apply-archive-rule-archives-real-finding-end-to-end
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: accessanalyzer|CreateQueue|NotImplemented
Issue-Commit: c6f9974c04d0d99d1dbb9b00d2d222f6d449381b

# accessanalyzer ApplyArchiveRule end-to-end finding-archival path unexercisable: its SQS dependency answers NotImplemented on CreateQueue

> _Note: this issue was identified against the code base at commit `c6f9974c04d0d99d1dbb9b00d2d222f6d449381b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyArchiveRule

## Description
One cause. The accessanalyzer cross-service check that proves real finding generation and retroactive archival cannot reach any accessanalyzer code: the resource it analyzes is an SQS queue, and the emulator refuses the very first setup call with its scaffold refusal.

AWS: Smithy (services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/...): ApplyArchiveRule is `PUT /archive-rule` taking required `analyzerArn` and `ruleName`, returning an empty body on success and modeling `ResourceNotFoundException` (404), `ValidationException` (400), `AccessDeniedException` (403). Recorded behavior only for the end-to-end semantics: a queue policy granting a foreign principal produces a real ACTIVE ExternalAccess finding once the analyzer scans it, and ApplyArchiveRule retroactively flips that existing finding's status to ARCHIVED. The prerequisite SQS CreateQueue call is expected to succeed and return a queue URL.
Emulator: the check dies on the first call — CreateQueue returns error code `NotImplemented` ("operation CreateQueue is scaffolded but not implemented in this emulator"), so no queue, no resource policy, no scan, and ApplyArchiveRule is never invoked. Nothing is known about the accessanalyzer side of this path either way.
Why: a client cannot drive the archive-rule workflow against any analyzable resource, so the whole findings-lifecycle capability (generate -> ACTIVE -> ApplyArchiveRule -> ARCHIVED) is unverifiable on this emulator even though the accessanalyzer operations involved may already be present.
Fix: the dependency, not accessanalyzer: the SQS CreateQueue handler must exist for this path to run. The SQS service is not stood up in this checkout and the open issue "Bootstrap the SQS emulator" (20261021T221625Z-bootstrap-service-com-amazonaws-sqs-amazonsqs.md) owns that; this issue is filed under accessanalyzer because the failing check is an accessanalyzer check (its service token is `accessanalyzer`, which IS bootstrapped) and should be closed as covered once SQS answers CreateQueue and the listed check is re-replayed — if it then still fails, the residual divergence is in accessanalyzer's finding generation or ApplyArchiveRule and belongs here.
Traces: accessanalyzer.apply-archive-rule-archives-real-finding-end-to-end.

## Evidence
Replay of suite simparity-behavioral-aws-v1:

accessanalyzer.apply-archive-rule-archives-real-finding-end-to-end [error] (NOT marked ALREADY FILED / OWNED by the report)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateQueue operation: operation CreateQueue is scaffolded but not implemented in this emulator

Expected: CreateQueue succeeds (queue URL returned), a queue policy granting a foreign-account principal yields an ACTIVE ExternalAccess finding for that queue, and ApplyArchiveRule(analyzerArn, ruleName) returns 200 with an empty body and flips that finding to ARCHIVED.
Actual:  the first setup call is refused with Error.Code NotImplemented; the analyzer, the finding scan and ApplyArchiveRule are never exercised.

Related already-open issue (does not carry this check's replay key): 20261021T221625Z-bootstrap-service-com-amazonaws-sqs-amazonsqs.md ("Bootstrap the SQS emulator").

## Steps to Reproduce
Boot the emulator. Call SQS CreateQueue(QueueName=<any>) — it answers Error.Code NotImplemented instead of returning a QueueUrl. Because of that, the downstream legs cannot be reached: attach a queue policy granting a foreign-account principal sqs:SendMessage, create an ACCOUNT analyzer, wait for the ExternalAccess finding for that queue to appear with status ACTIVE, create an archive rule matching it, call ApplyArchiveRule(analyzerArn, ruleName) and expect the finding's status to become ARCHIVED. Replay accessanalyzer.apply-archive-rule-archives-real-finding-end-to-end after SQS CreateQueue is implemented to confirm.
