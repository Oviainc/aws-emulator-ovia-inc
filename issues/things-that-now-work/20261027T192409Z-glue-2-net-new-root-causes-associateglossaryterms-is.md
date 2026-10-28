Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.associate-glossary-terms-concurrent-modification-rejected glue.associate-glossary-terms-nonexistent-entity-rejected glue.concurrent-delete-glossary-term-and-associate-both-succeed glue.concurrent-glossary-term-association-conflict-detected glue.delete-form-type-referenced-by-asset-type-fails
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.associate-glossary-terms-concurrent-modification-rejected|run#22|AssertionError|assert len(failures) <= 1, f"expected at most one failure, got {outcomes}"
Issue-Key: glue.associate-glossary-terms-nonexistent-entity-rejected|run#21|AssertionError|assert code == "EntityNotFoundException", (
Issue-Key: glue.concurrent-delete-glossary-term-and-associate-both-succeed|run#15|AssertionError|assert associate_code is None, f"expected AssociateGlossaryTerms to succeed, got code={associate_code}"
Issue-Key: glue.concurrent-glossary-term-association-conflict-detected|run#12|AssertionError|assert len(successes) >= 1, "no racer ever succeeded at associating a term"
Issue-Key: glue.delete-form-type-referenced-by-asset-type-fails|run#22|AssertionError|assert conflict == ("ConflictException", conflict_message), (
Issue-Commit: 498a3c4496c661f7096ad33513094351185b7aba

# glue: 2 net-new root causes — AssociateGlossaryTerms is a 501 stub with no not-found/concurrency semantics, and DeleteFormType ignores asset-type references

> _Note: this issue was identified against the code base at commit `498a3c4496c661f7096ad33513094351185b7aba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateGlossaryTerms

## Description
Two distinct, independent root causes in the Glue business-glossary / metadata-form surface. Both are in service `glue`, which is bootstrapped, so both are fidelity findings. Each cause is numbered and carries its own failing check ids; a fixer must close BOTH before submitting.

---

Cause 1 of 2: `AssociateGlossaryTerms` answers every request with the scaffold's `NotImplemented`, so it has none of the modeled not-found or concurrency behavior.
Failing checks: glue.associate-glossary-terms-nonexistent-entity-rejected, glue.associate-glossary-terms-concurrent-modification-rejected, glue.concurrent-glossary-term-association-conflict-detected, glue.concurrent-delete-glossary-term-and-associate-both-succeed.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) models AssociateGlossaryTerms with required input members `AssetIdentifier` (AssetId, @length 1..1087, @pattern ^[a-zA-Z0-9\-\:\/\.\_\*]+$, @httpLabel) and `GlossaryTermIdentifiers` (GlossaryTermIdList, @length 1..10), optional `IterableFormName`, `ItemIdentifier` and an @idempotencyToken `ClientToken`; output `AssociateGlossaryTermsResponse` returns `AssetIdentifier`, `IterableFormName`, `ItemIdentifier` and `GlossaryTerms`. Modeled errors: AccessDeniedException, ConcurrentModificationException ("Two processes are trying to modify a resource simultaneously."), EntityNotFoundException, InternalServiceException, InvalidInputException, ThrottlingException — every client error HTTP 400. The behavior the recordings show, beyond the model:
  * A happy-path call on an existing asset returns 200 with `GlossaryTerms` listing exactly the term ids the request named, and a subsequent independent `GetAsset` reports those ids in its `GlossaryTerms`.
  * An `AssetIdentifier` that names no asset in the account is refused with HTTP 400, code `EntityNotFoundException`, message `{"Message":"Asset not found: <AssetIdentifier>"}`. Fixed text: the JSON envelope plus `Asset not found: `; variable part: the `AssetIdentifier` taken verbatim from the request. `GetAsset` on the same absent identifier refuses with the same code and the same message template.
  * Concurrency: N simultaneous AssociateGlossaryTerms calls against one asset leave AT MOST one loser; any loser is refused with code `ConcurrentModificationException`, every reported winner's term is present on a later `GetAsset`, and no loser's term is present. A fully serialized outcome (zero losers) is also correct — losing is not guaranteed, only that a loser is reported as ConcurrentModificationException and that state matches the reported outcomes. Retrying a loser sequentially then succeeds.
  * AssociateGlossaryTerms and DeleteGlossaryTerm are NOT cross-locked: an associate naming a term and a delete of that same term both succeed; the term then 404s from `GetGlossaryTerm` with `EntityNotFoundException` / `{"Message":"Business glossary term <term-id> not found."}` while the asset's `GlossaryTerms` still names the deleted term id.

Emulator: every call returns the scaffold refusal — error code `NotImplemented`, message `operation AssociateGlossaryTerms is scaffolded but not implemented in this emulator`. Replay evidence: the nonexistent-asset check got code `NotImplemented` where `EntityNotFoundException` is required; the 2-way race got TWO failures, both `NotImplemented`, where at most one failure (and that one `ConcurrentModificationException`) is allowed; the 8-way race got zero successes; the sequenced associate/delete check got `code=NotImplemented` where the associate must succeed.

Why: a client cannot attach glossary terms to a catalog asset at all, and cannot distinguish a genuinely missing asset (EntityNotFoundException, retry is pointless) from a lost optimistic-concurrency race (ConcurrentModificationException, retry is the documented remedy) from an unimplemented endpoint. `NotImplemented` is not a modeled error for this operation, so SDK retry/error-handling branches never fire and glossary-term governance code fails closed against the emulator.

Fix: implement the handler in services/service-com-amazonaws-glue-awsglue/src/ops/associate_glossary_terms.rs — resolve `AssetIdentifier` against the asset store, refuse a miss with the modeled EntityNotFoundException and the message template above, merge the requested term ids into the asset's glossary-term set under the state lock so concurrent mutators either serialize or report ConcurrentModificationException, and serialize the modeled output members. The asset store and its glossary-term field live in services/service-com-amazonaws-glue-awsglue/src/state.rs. NOTE: an auto-filed issue already covers AssociateGlossaryTerms as a bare unimplemented stub (it was reported as blocking glue.e2e-glossary-term-association-lifecycle-flow). That issue does NOT carry the four check ids listed here, so merely making the operation answer 200 will not satisfy them — the not-found message and the concurrency semantics above are part of this cause.

Traces: glue.associate-glossary-terms-nonexistent-entity-rejected (expected EntityNotFoundException for a nonexistent asset id, got 'NotImplemented'); glue.associate-glossary-terms-concurrent-modification-rejected (expected at most one failure, got [('NotImplemented', ...), ('NotImplemented', ...)]); glue.concurrent-glossary-term-association-conflict-detected (no racer ever succeeded at associating a term); glue.concurrent-delete-glossary-term-and-associate-both-succeed (expected AssociateGlossaryTerms to succeed, got code=NotImplemented).

---

Cause 2 of 2: `DeleteFormType` deletes a metadata form type that an asset type still references, instead of refusing it with ConflictException.
Failing check: glue.delete-form-type-referenced-by-asset-type-fails.

AWS: the Smithy model documents DeleteFormType as "Deletes a form type from Glue Data Catalog. A form type cannot be deleted if it is still referenced by an asset type." and models `ConflictException` (HTTP 400) among its errors (alongside AccessDeniedException, ConcurrentModificationException, InternalServiceException, InvalidInputException, ThrottlingException). Input is the required `Identifier` (FormTypeId, @length 1..256); output is empty. Recorded behavior for the refusal: HTTP 400, code `ConflictException`, message `{"Message":"Form type <form-type-name> cannot be deleted because of the following dependencies: gluetype glueAsset#glue_<account-id>#<asset-type-name>"}`. Fixed text: the JSON envelope, `Form type `, ` cannot be deleted because of the following dependencies: gluetype glueAsset#glue_`, `#`; variable parts: the form type name from the request's `Identifier`, the caller's account id, and the name of each asset type in state that still references it. The guard is a LIVE reference check, not a permanent lock: once the referencing asset and asset type are deleted, the identical DeleteFormType call succeeds and `GetFormType` then refuses with `EntityNotFoundException` / `{"Message":"Metadata form type <form-type-name> not found."}`. A form type no asset type references deletes cleanly (the check's positive control).

Emulator: the handler unconditionally removes the record and returns 200. services/service-com-amazonaws-glue-awsglue/src/ops/delete_form_type.rs states this as intended ("It does not refuse a form type an ASSET TYPE still references") and its `handle` calls `remove_form_type_by_reference` with no dependency scan. The check polls DeleteFormType six times over ~15s against a form type referenced by a live asset type and every attempt is accepted: `expected ConflictException, got ('<succeeded>', '')`.

Why: the emulator is overly permissive on a documented integrity guard. A client that relies on AWS refusing the delete will, against the emulator, silently destroy a form type that asset types and assets still point at, leaving asset types whose `Forms` entries name a form type that no longer exists. Code with a ConflictException handler (surface the dependency list, delete the dependents first, retry) is never exercised, and teardown ordering bugs stay invisible until they hit real AWS.

Fix: add the reference guard to the DeleteFormType handler's validation in services/service-com-amazonaws-glue-awsglue/src/ops/delete_form_type.rs — before removing the record, scan the asset-type store in services/service-com-amazonaws-glue-awsglue/src/state.rs for asset types whose `Forms` reference the resolved form type, and on any hit return the modeled `ConflictException` with the dependency message above instead of deleting. The inline module doc asserting the delete is unconditional must be corrected along with it.

Traces: glue.delete-form-type-referenced-by-asset-type-fails.

## Evidence
Cause 1 of 2 — AssociateGlossaryTerms is a 501 scaffold stub.
Expected (AWS): 200 with `GlossaryTerms` echoing the requested term ids; HTTP 400 `EntityNotFoundException` / `{"Message":"Asset not found: <AssetIdentifier>"}` for an asset id that does not exist; at most one loser in a concurrent round and that loser refused with `ConcurrentModificationException`; no cross-lock with DeleteGlossaryTerm.
Actual (emulator): every call fails with code `NotImplemented`, message `operation AssociateGlossaryTerms is scaffolded but not implemented in this emulator`.
Replay output:
  glue.associate-glossary-terms-nonexistent-entity-rejected -> AssertionError: expected EntityNotFoundException for a nonexistent asset id, got 'NotImplemented'
  glue.associate-glossary-terms-concurrent-modification-rejected -> AssertionError: expected at most one failure, got [('NotImplemented', '609fd46af90770995bf4285a632de704'), ('NotImplemented', '326101d38851dfa451b49c4359305f3f')]
  glue.concurrent-glossary-term-association-conflict-detected -> AssertionError: no racer ever succeeded at associating a term
  glue.concurrent-delete-glossary-term-and-associate-both-succeed -> AssertionError: expected AssociateGlossaryTerms to succeed, got code=NotImplemented
Model citation: Smithy AssociateGlossaryTerms — required AssetIdentifier (AssetId, len 1..1087) and GlossaryTermIdentifiers (len 1..10); errors AccessDeniedException, ConcurrentModificationException, EntityNotFoundException, InternalServiceException, InvalidInputException, ThrottlingException (all client errors HTTP 400). `NotImplemented` is not among them.

Cause 2 of 2 — DeleteFormType ignores asset-type references.
Expected (AWS): HTTP 400, code `ConflictException`, message `{"Message":"Form type <form-type-name> cannot be deleted because of the following dependencies: gluetype glueAsset#glue_<account-id>#<asset-type-name>"}`; the form type remains readable via GetFormType; the same delete succeeds after the referencing asset type is removed.
Actual (emulator): HTTP 200, the form type is deleted. Six retries over ~15s all succeeded.
Replay output:
  glue.delete-form-type-referenced-by-asset-type-fails -> AssertionError: expected ConflictException, got ('<succeeded>', '')
Model citation: Smithy DeleteFormType doc — "A form type cannot be deleted if it is still referenced by an asset type."; modeled error ConflictException (HTTP 400). Source confirmation: services/service-com-amazonaws-glue-awsglue/src/ops/delete_form_type.rs lines 12-19 (module doc: "It does not refuse a form type an ASSET TYPE still references") and lines 177-191 (`remove_form_type_by_reference` then `Ok(DeleteFormTypeOutput {})`, no dependency check).

## Steps to Reproduce
Cause 1 of 2 (AssociateGlossaryTerms):
1. glue.create_glossary(Name=<glossary>) -> Id; glue.create_glossary_term(GlossaryIdentifier=<glossary-id>, Name=<term>) -> Id.
2. glue.put_form_type(Name=<FormTypeName>, Schema="structure <FormTypeName> { note: String }"); glue.put_asset_type(Name=<AssetTypeName>, Forms={<FormTypeName>: {"FormTypeIdentifier": <FormTypeName>}}); glue.put_asset(AssetTypeId=<AssetTypeName>, Identifier=<asset-id>, Name=<asset-id>, Forms={...}).
3. glue.associate_glossary_terms(AssetIdentifier=<an-identifier-that-was-never-created>, GlossaryTermIdentifiers=[<term-id>]).
   Expected: ClientError EntityNotFoundException, Message {"Message":"Asset not found: <that identifier>"}. Observed: ClientError NotImplemented.
4. glue.associate_glossary_terms(AssetIdentifier=<asset-id>, GlossaryTermIdentifiers=[<term-id>]).
   Expected: 200 with GlossaryTerms == [<term-id>], and glue.get_asset(Identifier=<asset-id>)["GlossaryTerms"] containing <term-id>. Observed: ClientError NotImplemented.
5. Fire 2 (and separately 8) AssociateGlossaryTerms calls on <asset-id> simultaneously, each naming a distinct term id.
   Expected: at least one success; any failure is ConcurrentModificationException; GetAsset reflects exactly the winners. Observed: every call fails NotImplemented.
6. Call associate_glossary_terms(<asset-id>, [<term-id>]) then delete_glossary_term(Identifier=<term-id>).
   Expected: both succeed; get_glossary_term(<term-id>) then refuses EntityNotFoundException with {"Message":"Business glossary term <term-id> not found."}; get_asset(<asset-id>)["GlossaryTerms"] still == [<term-id>]. Observed: the associate fails NotImplemented.
Replay: run_behavioral_suite and re-run glue.associate-glossary-terms-nonexistent-entity-rejected, glue.associate-glossary-terms-concurrent-modification-rejected, glue.concurrent-glossary-term-association-conflict-detected, glue.concurrent-delete-glossary-term-and-associate-both-succeed.

Cause 2 of 2 (DeleteFormType):
1. glue.put_form_type(Name=<FormRef>, Schema="structure <FormRef> { value: String }") and glue.put_form_type(Name=<FormCtrl>, ...).
2. glue.put_asset_type(Name=<AssetType>, Forms={<FormRef>: {"FormTypeIdentifier": <FormRef>}}); glue.put_asset(AssetTypeId=<AssetType>, Identifier=<asset-id>, Name=<asset-id>, Forms={<FormRef>: {"FormTypeId": <FormRef>, "Content": "{}"}}).
3. glue.delete_form_type(Identifier=<FormCtrl>) — positive control, must succeed; get_form_type(<FormCtrl>) then refuses EntityNotFoundException with {"Message":"Metadata form type <FormCtrl> not found."}.
4. glue.delete_form_type(Identifier=<FormRef>) while <AssetType> still references it.
   Expected: ClientError ConflictException, Message {"Message":"Form type <FormRef> cannot be deleted because of the following dependencies: gluetype glueAsset#glue_<account-id>#<AssetType>"}, and glue.get_form_type(Identifier=<FormRef>) still returns the form type. Observed: HTTP 200, the form type is deleted.
5. glue.delete_asset(Identifier=<asset-id>); glue.delete_asset_type(Identifier=<AssetType>); glue.delete_form_type(Identifier=<FormRef>) — must now succeed, and get_form_type(<FormRef>) refuses EntityNotFoundException.
Replay: run_behavioral_suite and re-run glue.delete-form-type-referenced-by-asset-type-fails.
