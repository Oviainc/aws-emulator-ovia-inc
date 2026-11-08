Issue-Source: trace-sweep-divergence
Issue-Trace: glue-asset-crud-with-forms
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-asset-crud-with-forms glue-attachment-lifecycle glue-glossary-term-lifecycle-asset-association
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetAsset|2xx->2xx|$.Forms.catalog::Resource
Issue-Related: 20261029T201859Z-glue-metadata-catalog-reads-omit-modeled-members-and.md
Issue-Commit: 7a00204bd93c8ed9dd88a2629fdcaac9e0c865d8

# Glue GetAsset reads omit the asset type and every form collection

> _Note: this issue was identified against the code base at commit `7a00204bd93c8ed9dd88a2629fdcaac9e0c865d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAsset

## Description
GetAsset answers 200 with an asset envelope that carries only the asset's scalar identity; every member that describes what the asset actually IS — its type and all four of its form/attachment collections — is absent from the response body. 2 causes, both in the GetAsset read projection.

Cause 1 of 2: GetAsset on an asset that was created with forms and attachments returns none of the form collections or the asset type.
  AWS: Smithy: GetAsset -> GetAssetOutput, which models Id (@required), Name, Description, CreatedAt, UpdatedAt, AssetTypeId (@required), GlossaryTerms (GlossaryTermIdList), Forms (AssetFormMap, "The forms on the asset, keyed by form name"), Attachments (AssetFormMap, "Additional attachments on the asset for more context, keyed by attachment name") and IterableForms (IterableFormMap, "The iterable forms available on the asset, keyed by form name"). The recording additionally shows AWS serialising the asset type under the member name `Type` and returning the collections `AssetForms` and `MetadataForms`, none of which the on-disk model declares — recorded behavior only for those three. On the wire the recorded 200 body for an asset put with one catalog form carries: `Type` (the asset-type id from the PutAsset request), `Forms` keyed by the qualified form name (e.g. a `catalog::Resource` entry whose value is an object with `Content` = the form JSON document supplied at write time and `FormTypeId` = the form type's id), `AssetForms` as a list with one entry per form held on the asset, `Attachments` as a map (empty `{}` when none were attached), `IterableForms` as a map (empty `{}` when none exist), `MetadataForms` as a list (empty `[]` when none exist) and `GlossaryTerms` as a list (empty `[]` when no term is associated). The empty-collection members are present-and-empty, not omitted.
  Emulator: status matches (200), but `Type`, `Forms`, `AssetForms`, `Attachments`, `IterableForms`, `MetadataForms` and `GlossaryTerms` are all reported absent (replay detail: `$.Type: missing (expected '<asset type id>', got None)`, `$.Forms.catalog::Resource: missing (expected {'Content': ..., 'FormTypeId': 'Resource'}, got None)`, `$.AssetForms: missing (expected [{}, {}], got None)`, `$.Attachments: missing (expected {}, got None)`, `$.IterableForms: missing (expected {}, got None)`, `$.MetadataForms: missing (expected [], got None)`, `$.GlossaryTerms: missing (expected [], got None)`).
  Why: a client cannot discover an asset's type or read back any form content it wrote. Every form-driven workflow breaks: a consumer that reads `Forms[<formName>].Content` to recover the metadata document gets a KeyError/nil instead of the document it just stored, and a consumer that iterates `AssetForms`/`MetadataForms`/`Attachments` sees an absent member where AWS guarantees at least an empty collection, so "no forms" and "field not served" are indistinguishable. The asset type is required output, so SDKs that model it non-optional fail to deserialise the response at all.
  Fix: the GetAsset handler's output projection — it must read the forms, attachments, iterable forms, metadata forms, glossary-term ids and asset-type id out of the stored asset record and serialise them, emitting present-but-empty collections when the asset holds none, rather than leaving the members unset.
  Traces: glue-asset-crud-with-forms#task#4 (representative), glue-attachment-lifecycle#task#6, glue-attachment-lifecycle#task#8.

Cause 2 of 2: GetAsset on an asset reached through the glossary-term association lifecycle shows the same dropped members, so the omission is not specific to the create-with-forms path.
  AWS: same modeled surface as Cause 1 (Smithy: GetAssetOutput members Forms/Attachments/IterableForms, plus the recorded-only `Type`, `AssetForms`, `MetadataForms`). After an asset type is created and an asset is put against it, the 200 body names that asset type in `Type` and serialises the asset's catalog form under `Forms` with its `Content` and `FormTypeId`.
  Emulator: 200 with `$.Type`, `$.Forms.catalog::Resource`, `$.AssetForms`, `$.Attachments`, `$.IterableForms` and `$.MetadataForms` all absent.
  Why: as Cause 1 — the asset-type binding and the form payload are unreadable, so a glossary/asset workflow that asserts on the asset's type or form content after association cannot proceed.
  Fix: same component as Cause 1 (the GetAsset output projection); the fix must cover the read path reached from every asset-write entry point, not just the one exercised by the CRUD fixture.
  Traces: glue-glossary-term-lifecycle-asset-association#task#7.

No open issue owns these steps (the replay reports them unowned). The nearest open issues — "Glue: 5 net-new root causes — DeleteFormType ignores asset-type references..." (20261030T010615Z) and "Glue metadata-catalog reads omit modeled members and mint non-AWS identifiers" (20261029T201859Z) — concern form-type deletion and the Hive catalog reads respectively, not the GetAsset projection, so neither covers this.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture glue-asset-crud-with-forms, step task#4, operation GetAsset, verdict mismatch, expected 200 / actual 200:
  $.Type: missing (expected 'assettype_610dd45f', got None)
  $.Forms.catalog::Resource: missing (expected {'Content': '{"name":"Customer Table","description":"Raw customer records"}', 'FormTypeId': 'Resource'}, got None)
  $.AssetForms: missing (expected [{}, {}], got None)
  $.Attachments: missing (expected {}, got None)
  $.IterableForms: missing (expected {}, got None)
  $.MetadataForms: missing (expected [], got None)
  $.GlossaryTerms: missing (expected [], got None)
  cluster also covers glue-attachment-lifecycle#task#6 and glue-attachment-lifecycle#task#8 (affected_count 3).
  (The quoted asset-type id, form name and form JSON are this recording's values filling the template; the requirement is that the members are present and carry the request's own values.)

Cause 2 — fixture glue-glossary-term-lifecycle-asset-association, step task#7, operation GetAsset, verdict mismatch, expected 200 / actual 200:
  $.Type: missing (expected 'SimGlossaryAssetType87c983e5', got None)
  $.Forms.catalog::Resource: missing (expected {'Content': '{"name":"behavioral-fidelity-asset"}', 'FormTypeId': 'Resource'}, got None)
  $.AssetForms: missing (expected [{}, {}], got None)
  $.Attachments: missing (expected {}, got None)
  $.IterableForms: missing (expected {}, got None)
  $.MetadataForms: missing (expected [], got None)
  affected_count 1.

Model evidence: smithy_query(operation="GetAsset") on services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json — GetAssetOutput declares Forms, Attachments, IterableForms, GlossaryTerms and a @required asset-type member; Type/AssetForms/MetadataForms appear in the recording only.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it.
2. Replay fixture glue-asset-crud-with-forms and stop at step task#4 (Cause 1): the fixture creates a form type and an asset type, puts an asset carrying a catalog::Resource form (and, in glue-attachment-lifecycle, an attachment), then calls GetAsset on it.
3. Inspect the 200 body: Type, Forms, AssetForms, Attachments, IterableForms, MetadataForms and GlossaryTerms are all absent, instead of naming the asset type, the stored form (Content + FormTypeId) and present-but-empty collections for the rest.
4. Replay fixture glue-glossary-term-lifecycle-asset-association to step task#7 (Cause 2) and read the GetAsset 200 body: Type, Forms, AssetForms, Attachments, IterableForms and MetadataForms are absent there too.
5. Re-replay glue-asset-crud-with-forms#task#4, glue-attachment-lifecycle#task#6, glue-attachment-lifecycle#task#8 and glue-glossary-term-lifecycle-asset-association#task#7 to confirm a fix.
