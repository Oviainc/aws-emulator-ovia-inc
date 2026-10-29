Issue-Source: trace-sweep-divergence
Issue-Trace: glue-glossary-and-term-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-glossary-and-term-lifecycle glue-glossary-crud-lifecycle glue-glossary-term-lifecycle glue-glossary-term-crud-lifecycle glue-glossary-term-lifecycle-asset-association glue-asset-crud-with-forms glue-attachment-lifecycle glue-asset-type-lifecycle-put-get-delete glue-form-type-and-asset-type-lifecycle glue-formtype-assettype-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetGlossary|2xx->2xx|$.CreatedAt
Issue-Key: glue|GetGlossary|2xx->2xx|$.UpdatedAt
Issue-Key: glue|GetGlossaryTerm|2xx->2xx|$.CreatedAt
Issue-Key: glue|GetGlossaryTerm|2xx->2xx|$.UpdatedAt
Issue-Key: glue|ListGlossaryTerms|2xx->2xx|$.GlossaryId
Issue-Key: glue|AssociateGlossaryTerms|2xx->2xx|$.Identifier
Issue-Key: glue|CreateGlossary|2xx->2xx|$.Id
Issue-Key: glue|CreateGlossaryTerm|2xx->2xx|$.Id
Issue-Key: glue|GetAsset|2xx->2xx|$.AssetForms
Issue-Key: glue|GetAsset|2xx->2xx|$.Attachments
Issue-Key: glue|GetAsset|2xx->2xx|$.Forms.catalog
Issue-Key: glue|GetAsset|2xx->2xx|$.Forms.primaryForm.Content
Issue-Key: glue|GetAsset|2xx->2xx|$.GlossaryTerms
Issue-Key: glue|GetAsset|2xx->2xx|$.IterableForms
Issue-Key: glue|GetAsset|2xx->2xx|$.MetadataForms
Issue-Key: glue|GetAsset|2xx->2xx|$.Type
Issue-Key: glue|PutAssetType|2xx->2xx|$.Id
Issue-Key: glue|GetAssetType|2xx->2xx|$.Id
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue metadata-catalog reads omit modeled members and mint non-AWS identifiers

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGlossary

## Description
This issue bundles 10 net-new causes in the Glue metadata-catalog surface (glossaries, glossary terms, assets, asset types). Shared fault: the emulator stores only the handful of fields it needs for its own round-trip, so these responses drop members AWS returns and mint identifiers in its own format (a 32-character hex digest) instead of AWS's identifier shape.

Cause 1 of 10: GetGlossary omits CreatedAt/UpdatedAt.
AWS: Smithy: GetGlossary output models Id, Name, Description only — the timestamps are recorded behavior only: AWS returns CreatedAt and UpdatedAt epoch-second timestamps for the glossary (set at creation, bumped on update).
Emulator: both members absent.
Why: a client that sorts or displays glossary age gets nothing.
Fix: GetGlossary handler + glossary state (record and serialize creation/update timestamps).
Traces: glue-glossary-and-term-lifecycle#task#2 (also glue-glossary-crud-lifecycle#task#2, glue-glossary-term-lifecycle#task#2).

Cause 2 of 10: GetGlossaryTerm omits CreatedAt/UpdatedAt.
AWS: recorded behavior only — the read returns CreatedAt and UpdatedAt for the term.
Emulator: both members absent.
Why: as cause 1.
Fix: GetGlossaryTerm handler + glossary-term state.
Traces: glue-glossary-and-term-lifecycle#task#5 (also glue-glossary-term-crud-lifecycle#task#4, glue-glossary-term-lifecycle#task#5, glue-glossary-term-lifecycle-asset-association#task#8).

Cause 3 of 10: ListGlossaryTerms omits GlossaryId.
AWS: Smithy: ListGlossaryTerms output models Items and NextToken; GlossaryId is recorded behavior only — AWS echoes the GlossaryIdentifier it listed under as GlossaryId.
Emulator: GlossaryId absent.
Why: a client paging several glossaries cannot attribute a page to its glossary.
Fix: ListGlossaryTerms handler/serializer (echo the requested glossary id).
Traces: glue-glossary-and-term-lifecycle#task#6 (also glue-glossary-term-crud-lifecycle#task#5, glue-glossary-term-lifecycle#task#6).

Cause 4 of 10: AssociateGlossaryTerms omits the asset identifier it associated.
AWS: Smithy: AssociateGlossaryTerms output models AssetIdentifier, IterableFormName, ItemIdentifier, GlossaryTerms; the recording additionally shows an 'Identifier' member carrying the asset id from the request (recorded behavior only).
Emulator: Identifier absent.
Why: the caller cannot correlate the association response with the asset it just modified.
Fix: AssociateGlossaryTerms handler/serializer.
Traces: glue-glossary-term-lifecycle-asset-association#task#6.

Cause 5 of 10: CreateGlossary returns an Id that is not AWS's identifier shape.
AWS: Smithy: GlossaryId. Recorded behavior: the Id AWS returns matches the recorded glossary-id shape (the fixture asserts the shape, not the value).
Emulator: returns a 32-character lowercase hex digest, which fails the shape assertion.
Why: a client that validates or re-derives glossary ids (or routes on their form) rejects the emulator's id.
Fix: the glossary id generator used by CreateGlossary (id minting in the glossary handler/state).
Traces: glue-glossary-term-crud-lifecycle#setup#1.

Cause 6 of 10: CreateGlossaryTerm returns an Id that is not AWS's identifier shape.
AWS: as cause 5, for glossary-term ids.
Emulator: 32-character hex digest; shape assertion fails.
Why: as cause 5.
Fix: the glossary-term id generator in CreateGlossaryTerm.
Traces: glue-glossary-term-crud-lifecycle#task#2 (also task#3).

Cause 7 of 10: GetAsset drops forms, attachments, glossary terms and type.
AWS: Smithy: GetAsset output models Forms, Attachments, IterableForms, GlossaryTerms (plus Id/Name/Description/CreatedAt/UpdatedAt/AssetTypeId); AssetForms, MetadataForms and Type are recorded behavior only — AWS returns the asset's forms list, its empty metadata-form list, empty GlossaryTerms list, empty Attachments/IterableForms maps and the asset's Type. AWS also returns each form's Content as the exact JSON string the client supplied.
Emulator: AssetForms, Attachments, Forms.<catalog form>, GlossaryTerms, IterableForms, MetadataForms and Type are all absent, and the one Content it does return has been re-serialized with spaces after ':' and ',' instead of being echoed byte-for-byte.
Why: a client reading an asset's metadata forms sees none of them, and a client hashing or diffing Content sees a spurious change.
Fix: GetAsset handler/serializer and asset state (store the submitted form Content verbatim; emit the empty collections).
Traces: glue-asset-crud-with-forms#task#4 (also glue-attachment-lifecycle#task#6, #task#8).

Cause 8 of 10: GetAsset drops the same members for a glossary-associated asset.
AWS: as cause 7.
Emulator: AssetForms, Attachments, Forms.<catalog form>, IterableForms, MetadataForms, Type all absent.
Why: as cause 7.
Fix: same component as cause 7; distinct cluster, must be re-replayed.
Traces: glue-glossary-term-lifecycle-asset-association#task#7.

Cause 9 of 10: PutAssetType returns a digest instead of the asset type's id.
AWS: Smithy: PutAssetType/GetAssetType use AssetTypeId (length 1..256). PutAssetType returns the asset type's own id, which is the Name the caller supplied.
Emulator: returns a 32-character hex digest unrelated to the name.
Why: a client that uses the returned Id as the Identifier for GetAssetType/DeleteAssetType (the documented flow) addresses a resource under an id AWS would not mint.
Fix: PutAssetType handler's id assignment (derive from the submitted Name).
Traces: glue-asset-type-lifecycle-put-get-delete#task#2 (also glue-form-type-and-asset-type-lifecycle#task#5, glue-formtype-assettype-lifecycle#task#3).

Cause 10 of 10: GetAssetType returns a digest instead of the catalog-qualified id.
AWS: GetAssetType returns Id in the catalog-qualified form 'glue_<accountId>::<assetTypeName>' — fixed 'glue_' prefix, the account id, the '::' separator and the asset type's name.
Emulator: returns the same 32-character hex digest.
Why: as cause 9; cross-references built from this id do not resolve.
Fix: GetAssetType handler/serializer (compose the qualified id).
Traces: glue-asset-type-lifecycle-put-get-delete#task#3 (also glue-form-type-and-asset-type-lifecycle#task#6, glue-formtype-assettype-lifecycle#task#4).

## Evidence
Replay of simparity-wire-aws-v1, all steps 200/200 unless noted:
1. GetGlossary: $.CreatedAt and $.UpdatedAt missing (expected timestamps, got None).
2. GetGlossaryTerm: $.CreatedAt and $.UpdatedAt missing.
3. ListGlossaryTerms: $.GlossaryId missing (expected the glossary's id, got None).
4. AssociateGlossaryTerms: $.Identifier missing (expected the asset id from the request, got None).
5. CreateGlossary: $.Id expected '<shaped-not-asserted>' (shape assertion) got 'ccaf181eef363a6caa3403bbd387da8a'.
6. CreateGlossaryTerm: $.Id expected '<shaped-not-asserted>' got 'f468a53fe3479899006bc1151631a39b'.
7. GetAsset: $.AssetForms, $.Attachments, $.Forms.catalog::Resource, $.GlossaryTerms, $.IterableForms, $.MetadataForms, $.Type all missing; $.Forms.primaryForm.Content expected '{"owner":"analytics-team","rowCount":1000}' got '{"owner": "analytics-team", "rowCount": 1000}' (re-serialized, not echoed).
8. GetAsset: $.AssetForms, $.Attachments, $.Forms.catalog::Resource, $.IterableForms, $.MetadataForms, $.Type all missing.
9. PutAssetType: $.Id expected the submitted asset-type name, got 'edc32b9f8a8e49e04a4af41cae372a44'.
10. GetAssetType: $.Id expected 'glue_000000000000::<assetTypeName>', got 'edc32b9f8a8e49e04a4af41cae372a44'.
(Names and digests above are that run's values.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Re-replay: glue-glossary-and-term-lifecycle#task#2, #task#5, #task#6, glue-glossary-term-lifecycle-asset-association#task#6, glue-glossary-term-crud-lifecycle#setup#1, glue-glossary-term-crud-lifecycle#task#2, glue-asset-crud-with-forms#task#4, glue-glossary-term-lifecycle-asset-association#task#7, glue-asset-type-lifecycle-put-get-delete#task#2, glue-asset-type-lifecycle-put-get-delete#task#3.
3. Each fixture creates the metadata-catalog resource then reads it back; compare the response members and identifier shapes with the AWS behavior quoted per cause.
