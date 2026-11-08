Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: entityresolution.matching-workflow-job-full-lifecycle
Issue-Shape-Id: com.amazonaws.entityresolution#AWSVeniceService
Issue-Key: entityresolution.matching-workflow-job-full-lifecycle|run#28|AssertionError|assert len(dup_match_ids) == 2 and len(set(dup_match_ids)) == 1, dup_match_ids
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# entityresolution matchId is minted as a hyphenated UUID, not AWS's 32-character lowercase hex id

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMatchingJob

## Description
Cause 1 of 1: every `matchId` this service mints carries four hyphens, so it is not in the form AWS serves.

AWS: `MatchGroup$matchId` / the output table's match-id column is a plain `String` in the Smithy model with no modelled pattern (`smithy_query(query="shape",shape="MatchGroup")` on services/service-com-amazonaws-entityresolution-awsveniceservice/smithy-model/com-amazonaws-entityresolution-awsveniceservice.json states no length or pattern constraint), so for the exact form this is RECORDED BEHAVIOR ONLY: the recording shows a matching job's output table carrying, for each matched record, a match id made of exactly 32 lowercase hexadecimal characters and NOTHING else — the same shape the model does fix for this service's sibling identifier `JobId` (`^[a-f0-9]{32}$`, which `StartMatchingJob`/`GetMatchingJob` both carry). Records sharing a resolved group carry one such id; distinct groups carry distinct ones. The recorded run writes 4 records in two groups of two, so the output text contains two distinct 32-hex ids, each appearing on the two rows of its group.

Emulator: the id generator emits the HYPHENATED form of a v4 UUID verbatim — 8-4-4-4-12 lowercase hex characters separated by four '-' — i.e. a 36-character value whose longest unbroken hex run is 12 characters. The job itself behaves correctly otherwise: it reaches SUCCEEDED, reports inputRecords=4 and totalRecordsProcessed=4, and writes an output object under the configured outputS3Path whose header is the output attributes plus InputSourceARN, MatchID, MatchRule, with the email values present on each row. Only the MatchID value's FORM diverges. Note the contrast inside the same crate: the `jobId` generator strips the hyphens before answering (to satisfy the modelled `JobId` pattern) while the match-id generator does not, so the two identifiers this service mints from the same entropy source disagree in form.

Why: a client cannot recognise or join on the id it is given. Anything that parses, validates, or indexes a matchId against AWS's 32-hex form — a regex/format check, a fixed-width column, a join key against ids recorded from real AWS, or a downstream workflow that reads one run's output table as its input — sees a value that does not match and silently finds zero matched records, exactly as the replayed check does: it scanned the written output table for match ids and collected NONE for either email group, so it could not tell that the two shared-email records were grouped together at all. The same id reaches clients through GetMatchId/GenerateMatchId, so the divergence is not confined to the S3 output table.

Fix: the match-id minting helper in this service's state module — the `new_match_id` generator (and only it; the grouping, the output-table writer and the match index all faithfully carry whatever it returns). It must produce the same hyphen-free 32-lowercase-hex form the neighbouring `new_job_id` already produces, so every consumer of a matchId (the output table's MatchID column, the match index behind GetMatchId, and GenerateMatchId's MatchGroup) serves AWS's form.

Traces: entityresolution.matching-workflow-job-full-lifecycle — fails at `assert len(dup_match_ids) == 2 and len(set(dup_match_ids)) == 1` with `AssertionError: []` after the job reached SUCCEEDED with the correct metrics and a non-empty output object (the check's own 'no output objects were written under the output S3 prefix' guard did not fire, so the table was written and only the id form is wrong).

File/line citations: services/service-com-amazonaws-entityresolution-awsveniceservice/src/state.rs lines 2170-2172 (`new_match_id`, returns `BatchId::random().as_str().to_string()` — hyphens kept) versus lines 3165-3171 (`new_job_id`, same source with `.filter(|ch| *ch != '-')`); services/service-com-amazonaws-entityresolution-awsveniceservice/src/state/match_groups.rs lines 125-147 (one id per group, minted through the injected generator); services/service-com-amazonaws-entityresolution-awsveniceservice/src/state/matching_run.rs lines 104-110 and 844-878 (the MatchID column of the output document).

Neither open entityresolution issue covers this: 20261102T065110Z-entityresolution-id-namespace-lifecycle-unusable-createidnamespace-answers-403.md is about ID namespace CRUD status codes, and 20261103T143443Z-entityresolution-provider-service-catalog-is-incomplete-listproviderservices-omits.md is about the provider-service catalog.

## Evidence
Expected (recorded AWS): the matching job's output table under outputS3Path carries, per record, a MatchID of exactly 32 lowercase hex characters (`[0-9a-f]{32}`); the two records sharing dup@example.com carry one and the same such id, the two sharing other@example.com carry one and the same DIFFERENT such id. Matching AWS's recorded output text with `[0-9a-f]{32}` therefore yields 2 ids for each email group.

Actual (emulator): the MatchID column carries the hyphenated v4 UUID form (8-4-4-4-12 lowercase hex, 36 characters, four '-' separators). No substring of 32 unbroken hex characters exists in any output row, so scanning the written output table for AWS's id form yields NOTHING: `dup_match_ids == []` and the check fails with `AssertionError: []` at check_entityresolution_matching_workflow_job_full_lifecycle.py line 86. Everything leading up to that assertion matched AWS: ConflictException while the job was non-terminal with message 'Job associated with workflow <workflowName> is currently running, failed to delete' (fixed text, with <workflowName> taken from the request), status SUCCEEDED, metrics inputRecords=4 and totalRecordsProcessed=4, and a non-empty object written under the output prefix.

Same-crate contradiction: `new_job_id()` strips the hyphens (yielding the modelled `JobId` form `^[a-f0-9]{32}$`); `new_match_id()` does not, from the identical generator.

## Steps to Reproduce
1. Stage a 4-row comma-separated object (columns id,name,email) in S3 where two rows share one email address and the other two share a different one, and define a Glue Data Catalog table over its prefix with skip.header.line.count=1.
2. CreateSchemaMapping mapping id as UNIQUE_ID and email as EMAIL_ADDRESS with matchKey 'email'.
3. CreateMatchingWorkflow over that Glue table with resolutionTechniques RULE_MATCHING, one rule whose matchingKeys is ['email'], and outputSourceConfig writing to s3://<output-bucket>/output with output attributes id, name, email.
4. StartMatchingJob, then poll GetMatchingJob until status is SUCCEEDED.
5. ListObjectsV2 + GetObject under the output prefix and read the MatchID column of each row.
6. Observe the MatchID values are hyphenated 36-character UUIDs; searching the output text for `[0-9a-f]{32}` (AWS's recorded matchId form) finds no match, so the two shared-email rows cannot be recognised as one group. Expected: each row's MatchID is 32 lowercase hex characters with no hyphens, equal within a group and distinct across groups.
7. Equivalently, GenerateMatchId / GetMatchId return the same hyphenated form on MatchGroup$matchId.
