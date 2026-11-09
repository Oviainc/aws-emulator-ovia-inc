Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-custom-asset-publish-to-catalog-listing
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-custom-asset-publish-to-catalog-listing
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|CreateAsset|2xx->2xx|$.createdAt
Issue-Key: datazone|CreateAsset|2xx->2xx|$.createdBy
Issue-Key: datazone|CreateAsset|2xx->2xx|$.firstRevisionCreatedAt
Issue-Key: datazone|CreateAsset|2xx->2xx|$.firstRevisionCreatedBy
Issue-Key: datazone|CreateAsset|2xx->2xx|$.formsOutput[*].renderingConfig
Issue-Key: datazone|CreateAsset|2xx->2xx|$.readOnlyFormsOutput
Issue-Key: datazone|GetListing|2xx->2xx|$.createdAt
Issue-Key: datazone|GetListing|2xx->2xx|$.createdBy
Issue-Key: datazone|GetListing|2xx->2xx|$.item.assetListing.assetFormsMetadata
Issue-Key: datazone|GetListing|2xx->2xx|$.item.assetListing.createdAt
Issue-Key: datazone|GetListing|2xx->2xx|$.item.assetListing.glossaryTerms
Issue-Key: datazone|GetListing|2xx->2xx|$.item.assetListing.governedGlossaryTerms
Issue-Key: datazone|GetListing|2xx->2xx|$.item.assetListing.latestTimeSeriesDataPointForms
Issue-Key: datazone|GetListing|2xx->2xx|$.listingRevision
Issue-Key: datazone|GetListing|2xx->2xx|$.updatedAt
Issue-Key: datazone|GetListing|2xx->2xx|$.updatedBy
Issue-Key: datazone|DeleteFormType|2xx->4xx|status
Issue-Commit: dd81faee0e8912cda86320bb65fd8f2654901268

# datazone custom-asset publish flow: catalog responses omit modeled members and DeleteFormType conflicts

> _Note: this issue was identified against the code base at commit `dd81faee0e8912cda86320bb65fd8f2654901268`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAsset

## Description
THREE causes in the custom-asset publish-to-catalog flow: the asset-create response drops modeled members, the catalog listing read drops the published asset's form metadata and its empty lists and reports a stale revision, and the form-type delete that ends the flow is refused. Fix all three before submitting. No open issue carries these keys: the open datazone issues cover validation wording (20261029T172524Z-datazone-error-messages-diverge-from-aws-wording-in.md), domain-lifecycle CreateDomain/ListDomains (20261029T172524Z-datazone-domain-lifecycle-createdomain-skips-required-member-and.md) and denied-domain classification, none of them the asset/listing/form-type serialization or delete behavior below.

Cause 1 of 3: CreateAsset omits form rendering configuration and the read-only forms list.
AWS: Smithy CreateAsset POST /v2/domains/{domainIdentifier}/assets, success code 201; CreateAssetOutput.formsOutput is @required (FormOutputList, @length 0..10) and readOnlyFormsOutput is a FormOutputList (@length 0..10). The on-disk model's FormOutput declares only formName/typeName/typeRevision/content and no renderingConfig, so for that member this is recorded behavior only: the recording shows every formsOutput entry carrying a renderingConfig object (the recorded shape is {"collapse": <bool>}, default false) and readOnlyFormsOutput serialised as an empty array [] rather than omitted when the asset has no read-only forms.
Emulator: 201, but formsOutput[0].renderingConfig is absent from the entry and readOnlyFormsOutput is absent from the body entirely.
Why: a console/client that renders metadata forms reads renderingConfig as null and cannot decide whether a form is collapsed, and code iterating the read-only forms gets null/undefined where AWS yields a zero-length collection, so it throws instead of looping zero times.
Fix: CreateAsset's response serializer and the form-output projection in asset state — emit renderingConfig for every form output and always emit readOnlyFormsOutput, as [] when there are none.
Traces: datazone-custom-asset-publish-to-catalog-listing#task#7.

Cause 2 of 3: GetListing drops the published asset's form metadata and empty lists, and reports a listing revision one behind.
AWS: Smithy GetListing GET /v2/domains/{domainIdentifier}/listings/{identifier}, 200; GetListingOutput.listingRevision is @required, and item targets ListingItem -> AssetListing, whose glossaryTerms, governedGlossaryTerms and latestTimeSeriesDataPointForms are list members the recording serialises as empty arrays rather than omitting. assetFormsMetadata is not declared on AssetListing in the on-disk model (model drift), so for that member this is recorded behavior only: the recording returns item.assetListing.assetFormsMetadata as one entry per metadata form attached to the published asset, each {formName, renderingConfig{collapse}, typeName, typeRevision} with names/revisions taken from the asset's forms (the recording's sim2real_probe_dz_form_* name and typeRevision "1" are just that run's values). The recorded listingRevision for the listing read after the asset is published again is "2" — i.e. the revision increments with each publish of the same listing.
Emulator: 200 with item.assetListing.assetFormsMetadata, item.assetListing.glossaryTerms, item.assetListing.governedGlossaryTerms and item.assetListing.latestTimeSeriesDataPointForms all absent, and listingRevision "1" where AWS reports "2". Hypothesis: the publish path reuses the existing listing without bumping its revision counter.
Why: a catalog consumer sees a published asset with no form metadata at all, so it cannot display or validate the asset's forms; the absent lists turn "no glossary terms" into null and break iteration; and a stale listingRevision makes revision-pinned reads (GetListing?listingRevision=N) and optimistic-concurrency checks address the wrong revision.
Fix: GetListing's listing projection/serializer (populate assetFormsMetadata from the published asset's forms and always emit the three list members, [] when empty) plus the publish path in listing state that must increment listingRevision on re-publish.
Traces: datazone-custom-asset-publish-to-catalog-listing#task#9.

Cause 3 of 3: DeleteFormType answers 409 ConflictException for a form type whose users are already gone.
AWS: Smithy DeleteFormType DELETE /v2/domains/{domainIdentifier}/form-types/{formTypeIdentifier}, success code 204 with an empty output; ConflictException (httpError 409, message @required, "There is a conflict while performing this action") is the error AWS uses while the form type is still in use. In the recording the asset and asset type that referenced the form type were deleted in the preceding teardown steps, so AWS deletes it and answers 204 No Content.
Emulator: 409 ConflictException, refusing the delete.
Why: teardown cannot complete, so the form type leaks and any re-run of the same flow hits a name collision on the form type; idempotent cleanup code that treats 409 as a real in-use conflict loops or aborts where AWS simply succeeds.
Fix: DeleteFormType's handler and the form-type reference bookkeeping in state — drop a form type's references when the referencing assets/asset types are deleted, and raise ConflictException only while a live reference remains.
Traces: datazone-custom-asset-publish-to-catalog-listing#teardown#14.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture datazone-custom-asset-publish-to-catalog-listing (expected -> actual):
1. CreateAsset task#7, status 201 on both sides: $.formsOutput[0].renderingConfig expected {'collapse': False}, got None (member missing); $.readOnlyFormsOutput expected [], got None (member missing).
2. GetListing task#9, status 200 on both sides: $.item.assetListing.assetFormsMetadata expected [{'formName': '<form name>', 'renderingConfig': {'collapse': False}, 'typeName': '<form type name>', 'typeRevision': '<revision>'}], got None; $.item.assetListing.glossaryTerms expected [], got None; $.item.assetListing.governedGlossaryTerms expected [], got None; $.item.assetListing.latestTimeSeriesDataPointForms expected [], got None; $.listingRevision expected '2', got '1'.
3. DeleteFormType teardown#14: status expected 204, got 409 (ConflictException).
Not filed here (per-run values, not requirements): $.createdAt/$.updatedAt/$.firstRevisionCreatedAt on task#7 and task#9 are clock times and $.createdBy/$.updatedBy/$.firstRevisionCreatedBy are account-specific principal ARNs, asserted only for shape.

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1, fixture datazone-custom-asset-publish-to-catalog-listing:
1. task#7 — CreateAsset with one metadata form (formsInput) against a custom asset type: expect 201 whose formsOutput entry includes renderingConfig and whose body includes readOnlyFormsOutput as []; the emulator omits both members.
2. task#9 — publish the asset to the catalog, then GetListing for the resulting listing: expect 200 with item.assetListing.assetFormsMetadata listing the asset's forms, with glossaryTerms/governedGlossaryTerms/latestTimeSeriesDataPointForms as [], and listingRevision '2'; the emulator omits the four members and returns listingRevision '1'.
3. teardown#14 — after deleting the asset and the asset type, DeleteFormType for the form type they used: expect 204 No Content; the emulator returns 409 ConflictException.
