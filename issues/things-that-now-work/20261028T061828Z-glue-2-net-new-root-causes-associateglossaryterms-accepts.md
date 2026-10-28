Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.glossary-term-lifecycle-create-associate-get-delete-404
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.glossary-term-lifecycle-create-associate-get-delete-404|_error#27|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Commit: 22cd2d7ee35d84122c85f8f0653a0b356f703d69

# Glue: AssociateGlossaryTerms accepts a deleted glossary term instead of refusing it

> _Note: this issue was identified against the code base at commit `22cd2d7ee35d84122c85f8f0653a0b356f703d69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _This record was filed with a SECOND, unrelated cause — a CloudFormation-deployed Glue
> job's run never completing or enforcing the run role's IAM permissions, check
> `glue.deploy-job-missing-output-permission-rejected`. That cause is UNFIXED and was split
> out, with its own replay key, into
> `issues/things-that-do-not-work/20261028T061828Z-glue-cfn-deployed-job-run-never-completes-or-enforces-the-run-role.md`.
> What is resolved here, and all this record now claims, is the AssociateGlossaryTerms cause
> below._

## Operation
AssociateGlossaryTerms

## Description
One net-new root cause in the Glue emulator, grounded in a failing behavioral check that the replay report marked neither ALREADY FILED nor OWNED.

=== AssociateGlossaryTerms does not resolve GlossaryTermIdentifiers, so a deleted term re-associates with HTTP 200 instead of being refused ===

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) models AssociateGlossaryTerms with required AssetIdentifier (AssetId, len 1..1087) and required GlossaryTermIdentifiers (GlossaryTermIdList, len 1..10), and declares the client errors EntityNotFoundException (HTTP 400, "A specified entity does not exist") and InvalidInputException (HTTP 400, "The input provided was not valid"). The recorded behavior is that each identifier in GlossaryTermIdentifiers is resolved against the business-glossary-term namespace at association time: a term id that names no live term makes the whole call fail with error code InvalidInputException and HTTP 400, and the message member is the Glue business-catalog JSON-wrapped form `{"Message":"Glossary term not found: <termId>"}` — fixed text `{"Message":"Glossary term not found: ` + `"}`, with <termId> being the identifier taken verbatim from the request's GlossaryTermIdentifiers. (The sibling wording on the term's own reader, `{"Message":"Business glossary term <termId> not found."}` for GetGlossaryTerm on a deleted term, is already produced correctly by the emulator and passes in the same check, which confirms the JSON-wrapped message convention on this surface.) No partial association is written.

Emulator: the association succeeds. After CreateGlossary -> CreateGlossaryTerm -> AssociateGlossaryTerms -> DeleteGlossaryTerm, re-sending AssociateGlossaryTerms with the deleted term id returns HTTP 200 with a body echoing the asset and the term list, e.g. {'AssetIdentifier': '<asset>', 'GlossaryTerms': ['<deletedTermId>']}. This is deliberate in the handler: services/service-com-amazonaws-glue-awsglue/src/ops/associate_glossary_terms.rs documents in its module header that it "does not resolve the term ids against the glossary term namespace" and stores whatever the request sent, and the handle body (around the `asset.associate_glossary_terms(&input.glossary_term_identifiers)` call) validates only the asset, the iterable-form/item pairing and the 10-term bound. There is no lookup of the ids in the glossary-term store at all.

Why: a client that uses AssociateGlossaryTerms as the authoritative check that a term is still live gets a false success. Dangling term ids accumulate on assets, GetAsset reports terms that GetGlossaryTerm says do not exist, and any client that branches on InvalidInputException (to re-create the term, to surface a validation error to a user, or to roll a transaction back) never takes that branch against the emulator while it does against AWS.

Fix: the AssociateGlossaryTerms handler's validation in services/service-com-amazonaws-glue-awsglue/src/ops/associate_glossary_terms.rs — resolve every entry of GlossaryTermIdentifiers against the glossary-term state before mutating the asset, and refuse the call with InvalidInputException (HTTP 400) and the message above, leaving the asset's term set unchanged. Note the open issue "Implement glue AssociateGlossaryTerms: unimplemented scaffold stub" (20261027T192409Z-glue-associateglossaryterms-stub.md) and "Implement glue DeleteGlossaryTerm: unimplemented scaffold stub" (20261027T234439Z-glue-deleteglossaryterm-stub.md): both operations now answer for real in this replay (associate returned 200, delete + GetGlossaryTerm produced the exact modeled miss), so neither covers this missing cross-namespace validation.

Traces: glue.glossary-term-lifecycle-create-associate-get-delete-404.

## Evidence
Check glue.glossary-term-lifecycle-create-associate-get-delete-404.
Expected (AWS): AssociateGlossaryTerms(AssetIdentifier=<asset>, GlossaryTermIdentifiers=[<deletedTermId>]) fails with HTTP 400, Error.Code = "InvalidInputException", Error.Message = '{"Message":"Glossary term not found: <deletedTermId>"}'.
Actual (emulator): HTTP 200 with body {'AssetIdentifier': 'sim2real-asset-<id>', 'GlossaryTerms': ['<deletedTermId>']} (recorded run: AssetIdentifier 'sim2real-asset-ddf740ade93f', GlossaryTerms ['21ed46874cf2f623eb2c8fc8a2456e67'] — those values are one run's fill-ins, not part of the requirement).
Harness assertion: 'AssertionError: expected a rejection, but the call succeeded and returned {...}' raised from check_glue_glossary_term_lifecycle_create_associate_get_delete_404.py line 72/93.
The immediately preceding assertions in the same check PASSED, so CreateGlossary, CreateGlossaryTerm, the first association, GetAsset's reflection of it, GetGlossaryTerm, DeleteGlossaryTerm and the post-delete EntityNotFoundException message are all already correct; the ONLY divergence is the missing term-id resolution on re-association.
Handler evidence: services/service-com-amazonaws-glue-awsglue/src/ops/associate_glossary_terms.rs lines 13-19 state the handler intentionally does not resolve term ids; lines 236-293 validate only the asset, the iterable-form/item pairing and the 10-term cap.

## Steps to Reproduce
glue.glossary-term-lifecycle-create-associate-get-delete-404:
1. glue:CreateGlossary(Name=<unique>) -> capture Id as <glossaryId>.
2. glue:CreateGlossaryTerm(GlossaryIdentifier=<glossaryId>, Name=<unique>, ShortDescription='...') -> capture Id as <termId>.
3. glue:PutFormType, glue:PutAssetType, glue:PutAsset to create an asset -> capture Id as <assetId>.
4. glue:AssociateGlossaryTerms(AssetIdentifier=<assetId>, GlossaryTermIdentifiers=[<termId>]) -> 200, GlossaryTerms == [<termId>]; glue:GetAsset reflects it.
5. glue:DeleteGlossaryTerm(Identifier=<termId>); confirm glue:GetGlossaryTerm(Identifier=<termId>) raises EntityNotFoundException with message '{"Message":"Business glossary term <termId> not found."}' (this already works).
6. glue:AssociateGlossaryTerms(AssetIdentifier=<assetId>, GlossaryTermIdentifiers=[<termId>]) again.
   Expected: HTTP 400 InvalidInputException, message '{"Message":"Glossary term not found: <termId>"}'.
   Observed: HTTP 200 echoing the deleted term id.

## Resolution
AssociateGlossaryTerms now resolves every entry of GlossaryTermIdentifiers against the glossary term namespace before it merges anything, and refuses the whole call with the modelled InvalidInputException (HTTP 400) naming the first identifier, in request order, that resolves to no live term — leaving the asset's term set unchanged. A term id an EARLIER association wrote stays on the asset when the term is deleted, which the same check asserts and which continues to hold. The second cause this record was filed with was split out, unfixed, into 20261028T061828Z-glue-cfn-deployed-job-run-never-completes-or-enforces-the-run-role.md.
