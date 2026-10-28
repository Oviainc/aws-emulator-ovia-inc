Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.glossary-term-lifecycle-create-associate-get-delete-404 glue.deploy-job-missing-output-permission-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.glossary-term-lifecycle-create-associate-get-delete-404|_error#27|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: glue.deploy-job-missing-output-permission-rejected|_run_job_run_with_retry#16|Timeout|time.sleep(interval_s)
Issue-Commit: 22cd2d7ee35d84122c85f8f0653a0b356f703d69

# Glue: 2 net-new root causes — AssociateGlossaryTerms accepts a deleted glossary term instead of refusing it, and a CloudFormation-deployed Glue job's run never completes or enforces the run role's IAM permissions

> _Note: this issue was identified against the code base at commit `22cd2d7ee35d84122c85f8f0653a0b356f703d69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateGlossaryTerms

## Description
Two net-new root causes in the Glue emulator, both grounded in failing behavioral checks that the replay report marked neither ALREADY FILED nor OWNED.

=== Cause 1 of 2: AssociateGlossaryTerms does not resolve GlossaryTermIdentifiers, so a deleted term re-associates with HTTP 200 instead of being refused ===

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) models AssociateGlossaryTerms with required AssetIdentifier (AssetId, len 1..1087) and required GlossaryTermIdentifiers (GlossaryTermIdList, len 1..10), and declares the client errors EntityNotFoundException (HTTP 400, "A specified entity does not exist") and InvalidInputException (HTTP 400, "The input provided was not valid"). The recorded behavior is that each identifier in GlossaryTermIdentifiers is resolved against the business-glossary-term namespace at association time: a term id that names no live term makes the whole call fail with error code InvalidInputException and HTTP 400, and the message member is the Glue business-catalog JSON-wrapped form `{"Message":"Glossary term not found: <termId>"}` — fixed text `{"Message":"Glossary term not found: ` + `"}`, with <termId> being the identifier taken verbatim from the request's GlossaryTermIdentifiers. (The sibling wording on the term's own reader, `{"Message":"Business glossary term <termId> not found."}` for GetGlossaryTerm on a deleted term, is already produced correctly by the emulator and passes in the same check, which confirms the JSON-wrapped message convention on this surface.) No partial association is written.

Emulator: the association succeeds. After CreateGlossary -> CreateGlossaryTerm -> AssociateGlossaryTerms -> DeleteGlossaryTerm, re-sending AssociateGlossaryTerms with the deleted term id returns HTTP 200 with a body echoing the asset and the term list, e.g. {'AssetIdentifier': '<asset>', 'GlossaryTerms': ['<deletedTermId>']}. This is deliberate in the handler: services/service-com-amazonaws-glue-awsglue/src/ops/associate_glossary_terms.rs documents in its module header that it "does not resolve the term ids against the glossary term namespace" and stores whatever the request sent, and the handle body (around the `asset.associate_glossary_terms(&input.glossary_term_identifiers)` call) validates only the asset, the iterable-form/item pairing and the 10-term bound. There is no lookup of the ids in the glossary-term store at all.

Why: a client that uses AssociateGlossaryTerms as the authoritative check that a term is still live gets a false success. Dangling term ids accumulate on assets, GetAsset reports terms that GetGlossaryTerm says do not exist, and any client that branches on InvalidInputException (to re-create the term, to surface a validation error to a user, or to roll a transaction back) never takes that branch against the emulator while it does against AWS.

Fix: the AssociateGlossaryTerms handler's validation in services/service-com-amazonaws-glue-awsglue/src/ops/associate_glossary_terms.rs — resolve every entry of GlossaryTermIdentifiers against the glossary-term state before mutating the asset, and refuse the call with InvalidInputException (HTTP 400) and the message above, leaving the asset's term set unchanged. Note the open issue "Implement glue AssociateGlossaryTerms: unimplemented scaffold stub" (20261027T192409Z-glue-associateglossaryterms-stub.md) and "Implement glue DeleteGlossaryTerm: unimplemented scaffold stub" (20261027T234439Z-glue-deleteglossaryterm-stub.md): both operations now answer for real in this replay (associate returned 200, delete + GetGlossaryTerm produced the exact modeled miss), so neither covers this missing cross-namespace validation.

Traces: glue.glossary-term-lifecycle-create-associate-get-delete-404.

=== Cause 2 of 2: a CloudFormation-declared AWS::Glue::Job plus StartJobRun never yields a completed run, so the run role's IAM permissions are never enforced ===

AWS: Smithy models StartJobRun (input JobName required, Arguments GenericMap; output JobRunId) and GetJobRun (input JobName + RunId required; output JobRun), with EntityNotFoundException/ConcurrentRunsExceededException as the client refusals. Recorded behavior: a CFN stack declaring two AWS::Glue::Job resources of type pythonshell reaches CREATE_COMPLETE and both jobs are then addressable by glue:StartJobRun under their declared names; each started run progresses to a terminal JobRunState within the run budget; the run executes the script at Command.ScriptLocation under the job's Role, so (a) the run whose role carries s3:PutObject on the output prefix ends SUCCEEDED and the output object exists with the script's body, (b) the run whose role lacks exactly that statement ends FAILED with ErrorMessage whose fixed text is "ClientError: An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws:sts::<account>:assumed-role/<roleName>/GlueJobRunnerSession is not authorized to perform: s3:PutObject on resource: \"arn:aws:s3:::<bucket>/<key>\" because no identity-based policy allows the s3:PutObject action" (account, role name, bucket and key are the request/state values, the rest is fixed), and the output object is absent so s3:GetObject answers NoSuchKey / "The specified key does not exist.", and (c) after iam:PutRolePolicy adds the missing s3:PutObject statement, a fresh run of the SAME job ends SUCCEEDED and writes its output.

Emulator: nothing in that sequence completes — the check is killed by its 60s budget ("Check timed out after 60.0s") with no terminal job run ever observed, so no JobRunState, no ErrorMessage and no S3 side effect is produced. The stuck step is in the CFN-deploy -> StartJobRun -> GetJobRun-until-terminal path (both poll loops only ever see a non-terminal state).

Why: a client cannot observe any outcome of a Glue job run against the emulator — neither success and its side effects nor an IAM-denied failure and its ErrorMessage. Every pipeline that waits on GetJobRun hangs rather than finishing, and every negative test of a least-privilege Glue role passes vacuously because the denial never materializes.

Fix: the Glue job-run execution path — the StartJobRun handler and the job-run state machine that GetJobRun reports from (plus the CloudFormation AWS::Glue::Job provisioning that must make the declared job name addressable before the run starts), including evaluation of the run role's identity-based policy for the run's own S3 calls. Related open issues describe the same family but do NOT carry this check's replay key, so none of them covers it: 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md, 20261025T072821Z-glue-job-runs-are-terminated-failed-without-executing.md, 20261025T022820Z-glue-cfn-declared-resources-and-job-run-execution.md, 20261025T050831Z-cloudformation-glue-iam-s3-cloudformation-never-provisions-aws-glue-job-after-create.md.

Traces: glue.deploy-job-missing-output-permission-rejected.

## Evidence
Cause 1 — check glue.glossary-term-lifecycle-create-associate-get-delete-404.
Expected (AWS): AssociateGlossaryTerms(AssetIdentifier=<asset>, GlossaryTermIdentifiers=[<deletedTermId>]) fails with HTTP 400, Error.Code = "InvalidInputException", Error.Message = '{"Message":"Glossary term not found: <deletedTermId>"}'.
Actual (emulator): HTTP 200 with body {'AssetIdentifier': 'sim2real-asset-<id>', 'GlossaryTerms': ['<deletedTermId>']} (recorded run: AssetIdentifier 'sim2real-asset-ddf740ade93f', GlossaryTerms ['21ed46874cf2f623eb2c8fc8a2456e67'] — those values are one run's fill-ins, not part of the requirement).
Harness assertion: 'AssertionError: expected a rejection, but the call succeeded and returned {...}' raised from check_glue_glossary_term_lifecycle_create_associate_get_delete_404.py line 72/93.
The immediately preceding assertions in the same check PASSED, so CreateGlossary, CreateGlossaryTerm, the first association, GetAsset's reflection of it, GetGlossaryTerm, DeleteGlossaryTerm and the post-delete EntityNotFoundException message are all already correct; the ONLY divergence is the missing term-id resolution on re-association.
Handler evidence: services/service-com-amazonaws-glue-awsglue/src/ops/associate_glossary_terms.rs lines 13-19 state the handler intentionally does not resolve term ids; lines 236-293 validate only the asset, the iterable-form/item pairing and the 10-term cap.

Cause 2 — check glue.deploy-job-missing-output-permission-rejected.
Expected (AWS): the CFN stack with two AWS::Glue::Job (pythonshell) resources reaches CREATE_COMPLETE; the good job's run ends JobRunState=SUCCEEDED and s3:GetObject of results/good.txt returns b'ok'; the bad job's run ends JobRunState=FAILED with ErrorMessage 'ClientError: An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws:sts::<account>:assumed-role/<badRoleName>/GlueJobRunnerSession is not authorized to perform: s3:PutObject on resource: "arn:aws:s3:::<bucket>/results/bad.txt" because no identity-based policy allows the s3:PutObject action' and s3:GetObject of that key answers ('NoSuchKey', 'The specified key does not exist.'); after PutRolePolicy grants s3:PutObject, a new run of the same job ends SUCCEEDED and writes results/bad-restored.txt.
Actual (emulator): 'Error: Check timed out after 60.0s' — no terminal JobRunState is ever reported, so none of the three outcomes (SUCCEEDED + object, FAILED + AccessDenied ErrorMessage + absent object, restored SUCCEEDED) is produced and no S3 side effect appears.

## Steps to Reproduce
Cause 1 (glue.glossary-term-lifecycle-create-associate-get-delete-404):
1. glue:CreateGlossary(Name=<unique>) -> capture Id as <glossaryId>.
2. glue:CreateGlossaryTerm(GlossaryIdentifier=<glossaryId>, Name=<unique>, ShortDescription='...') -> capture Id as <termId>.
3. glue:PutFormType, glue:PutAssetType, glue:PutAsset to create an asset -> capture Id as <assetId>.
4. glue:AssociateGlossaryTerms(AssetIdentifier=<assetId>, GlossaryTermIdentifiers=[<termId>]) -> 200, GlossaryTerms == [<termId>]; glue:GetAsset reflects it.
5. glue:DeleteGlossaryTerm(Identifier=<termId>); confirm glue:GetGlossaryTerm(Identifier=<termId>) raises EntityNotFoundException with message '{"Message":"Business glossary term <termId> not found."}' (this already works).
6. glue:AssociateGlossaryTerms(AssetIdentifier=<assetId>, GlossaryTermIdentifiers=[<termId>]) again.
   Expected: HTTP 400 InvalidInputException, message '{"Message":"Glossary term not found: <termId>"}'.
   Observed: HTTP 200 echoing the deleted term id.

Cause 2 (glue.deploy-job-missing-output-permission-rejected):
1. Create an S3 bucket and put the pythonshell script at s3://<bucket>/scripts/write_output.py (it reads --bucket/--output_key and calls s3:PutObject).
2. iam:CreateRole twice with a glue.amazonaws.com trust policy: the 'good' role gets inline s3:GetObject on the script plus s3:PutObject on arn:aws:s3:::<bucket>/results/*; the 'bad' role gets only s3:GetObject on the script.
3. cloudformation:CreateStack with two AWS::Glue::Job resources (Command.Name=pythonshell, ScriptLocation=the script, MaxCapacity=0.0625, GlueVersion=3.0), one per role; poll DescribeStacks to CREATE_COMPLETE.
4. glue:StartJobRun on the good job with Arguments {'--bucket': <bucket>, '--output_key': 'results/good.txt'} and poll glue:GetJobRun to a terminal JobRunState.
   Expected: SUCCEEDED and s3:GetObject('results/good.txt') == b'ok'.
   Observed: no terminal state is ever reported; the check is killed at its 60s budget.
5. glue:StartJobRun on the bad job with --output_key 'results/bad.txt' and poll to terminal.
   Expected: FAILED with the AccessDenied ErrorMessage quoted in the evidence and NoSuchKey for the object.
6. iam:PutRolePolicy adds s3:PutObject on arn:aws:s3:::<bucket>/results/* to the bad role; re-run the same job with --output_key 'results/bad-restored.txt'.
   Expected: SUCCEEDED and the object written.
