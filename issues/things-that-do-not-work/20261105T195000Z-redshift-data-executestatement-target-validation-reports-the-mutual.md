Issue-Source: trace-sweep-divergence
Issue-Trace: redshiftdata-target-selection-validation
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshiftdata-target-selection-validation
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data|ExecuteStatement|4xx->4xx|$.Message
Issue-Commit: 16c1636731f072135c908023c0b09afa04d105ac

# redshift-data ExecuteStatement target validation reports the mutual-exclusivity ValidationException instead of the nonexistent-endpoint refusal

> _Note: this issue was identified against the code base at commit `16c1636731f072135c908023c0b09afa04d105ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Cause 1 of 1: ExecuteStatement's target-selection validation answers the wrong ValidationException message when the request names a single Redshift target that does not resolve.

AWS: Redshift Data API ExecuteStatement takes exactly one connection target — ClusterIdentifier (provisioned), WorkgroupName (Serverless), a Glue resource, or an existing SessionId (Smithy: ExecuteStatement input members ClusterIdentifier/WorkgroupName/SessionId, all optional; modeled errors ValidationException httpError 400 'The Amazon Redshift Data API operation failed due to invalid input.', ResourceNotFoundException httpError 404). AWS distinguishes two failure modes inside that 400 ValidationException. (1) Too many targets supplied: the message is the fixed text 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId' — entirely fixed, no request-derived values. (2) Exactly one target supplied, but it names no Redshift endpoint in the request's region: the message is the fixed text "Redshift endpoint doesn't exist in this region." — also entirely fixed text, with no request-derived values (the cluster/workgroup name is NOT interpolated). The recorded response for this step is HTTP 400 with the latter message, so that is the requirement for a lone, unresolvable target.

Emulator: for the same request the emulator answers HTTP 400 (status matches) but with $.Message = 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId' — the too-many-targets wording — where AWS answers "Redshift endpoint doesn't exist in this region.". Hypothesis: the handler's target check counts members that are credentials/context rather than targets (e.g. SecretArn, DbUser, Database alongside ClusterIdentifier), so the mutual-exclusivity branch fires and short-circuits before the endpoint-resolution branch is ever reached; alternatively the two branches share one message constant.

Why: a client cannot tell a malformed request from a missing endpoint. SDK and CLI callers (and retry/provisioning logic) branch on this message: 'only one of the following' says 'fix your request parameters', while "Redshift endpoint doesn't exist in this region." says 'the cluster/workgroup is not there yet or you are in the wrong region'. Code that waits for a cluster to come up, or that falls back to another region, instead treats a perfectly well-formed single-target request as a programming error and aborts; and conversely a genuinely over-specified request is indistinguishable from a missing endpoint.

Fix: the ExecuteStatement input-validation stage of the redshift-data handler. Order the checks so the mutual-exclusivity check counts only the four real target members (ClusterIdentifier, WorkgroupName, Glue resource, SessionId) and runs ONLY when more than one of them is present; when exactly one is present but it resolves to no cluster/workgroup/session in the emulator's state, raise ValidationException with the fixed text "Redshift endpoint doesn't exist in this region." (note the apostrophe and the trailing period). Both messages are fixed strings and must be emitted verbatim; the same ordering applies to BatchExecuteStatement, which shares this target-selection rule.

Traces: redshiftdata-target-selection-validation#task#2 (expected $.Message "Redshift endpoint doesn't exist in this region.", got 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'; both HTTP 400).

## Evidence
Fixture redshiftdata-target-selection-validation, step task#2, operation ExecuteStatement (service redshift-data), verdict=mismatch.
Expected: HTTP 400, $.Message = "Redshift endpoint doesn't exist in this region."
Actual:   HTTP 400, $.Message = 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'
Divergence key: service=redshift-data|ExecuteStatement|status=400->400|keys=$.Message
The status and error class agree; only the message diverges, and it is the message for a different ValidationException cause. Fixed text per AWS: both messages are wholly fixed — no names, ids, limits or timestamps are interpolated into either — so neither carries per-run values and the recorded strings are directly the requirement.
No existing open issue carries this replay key (the divergence reports owned_by=[] and no stub_path). The nearest open issues — '20261022T180846Z-redshift-data-data-api-surface-diverges-resultformat-echoed.md' and '20261029T193810Z-redshift-data-api-statement-records-and-result-metadata.md' — cover ResultFormat echoing/GetStatementResult acceptance and ColumnMetadata/statement-record fields respectively, not ExecuteStatement target-selection refusal wording.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or replay fixture redshiftdata-target-selection-validation alone.
2. The fixture's step task#2 calls redshift-data ExecuteStatement with a single connection target that names no existing Redshift endpoint (and no second target member).
3. Observe the emulator's 400 response body: $.Message is the mutual-exclusivity text 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'.
4. AWS (and the recording) answers 400 with $.Message = "Redshift endpoint doesn't exist in this region."
5. Cross-check the inverse case: an ExecuteStatement naming two targets (e.g. ClusterIdentifier and WorkgroupName together) must be the only request that yields the 'only one of the following' message.
