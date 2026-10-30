Issue-Source: trace-sweep-divergence
Issue-Trace: comprehendmedical-detect-phi-note
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehendmedical-detect-phi-note comprehendmedical-infer-icd10cm-conditions comprehendmedical-infer-rxnorm-medications comprehendmedical-infer-snomedct-concepts
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.Entities[*].BeginOffset
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.Entities[*].EndOffset
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.Entities[*].Text
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.Entities[*].Type
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Attributes[*]
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].BeginOffset
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].EndOffset
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*]
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*].Code
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*].Description
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Text
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Traits[*]
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferRxNorm|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|InferRxNorm|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].BeginOffset
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].Category
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].EndOffset
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].RelationshipScore
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].RelationshipType
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].Text
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].Type
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].BeginOffset
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Category
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].EndOffset
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*].Code
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*].Description
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Text
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Traits[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Type
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.SNOMEDCTDetails.VersionDate
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# ComprehendMedical synchronous inference returns heuristic entities without ontology concepts

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectPHI

## Description
Four net-new wire divergences share one root fault: the synchronous text-inference operations now answer HTTP 200 (their per-operation stub issues are marked fixed) but the body is produced by a lexicon/pattern pass whose output does not have the SHAPE of a ComprehendMedical answer — it misses whole entities, segments entities differently, numbers entity Ids from 0, assigns a small set of fixed weights as Score, and — for the three ontology-linking operations — returns NO terminology concepts at all.

Scope note on floats: the exact confidence floats a trained model emits are not reproducible here and are NOT what this issue asks for. The requirements being missed are structural: which spans are reported, how they are segmented into entities and attributes, the Id numbering, the presence and ranking of the ontology concept lists, and the ModelVersion/VersionDate members. Where a score is cited below it is cited to show the emulator is emitting a fixed weight for every entity of a class, not a per-detection confidence.

Cause 1 of 4: DetectPHI misses entities and renumbers them.
AWS: Smithy: DetectPHI input Text (BoundedLengthString, required, @length 1..20000); output Entities (EntityList, required) and ModelVersion (required, @length min 1); errors InvalidRequestException/400, InvalidEncodingException/400, TextSizeLimitExceededException/400, TooManyRequestsException/429, InternalServerException/500, ServiceUnavailableException/503. Recorded behavior for a clinical note: six PHI entities in document order with Id values starting at 1 and increasing, each with Category PROTECTED_HEALTH_INFORMATION, a Type, byte offsets and a per-detection Score; the set includes the facility mention written as a hospital name ('Boston General Hospital'), alongside the name, the two dates, the phone number and the street address.
Emulator: five entities with Id values starting at 0; the facility mention is not detected at all, so every later entity occupies a different slot than AWS's; each Score is one of a few fixed weights (e.g. 0.8999999761581421, 0.949999988079071) rather than a per-detection confidence; ModelVersion differs from the recorded one. src/ops/detect_phi.rs computes Score from an Evidence class (Form/Context/Lexicon -> WEIGHT_PATTERN/WEIGHT_CUE/term_confidence), i.e. one constant per detection class.
Why: a de-identification client redacts the spans this operation returns. A facility name that is never reported is PHI left in the document, and Ids that start at 0 break any client that cross-references entity Ids (attributes reference them) against a recorded or AWS-produced answer.
Fix: DetectPHI's inference pass and its entity assembly (src/ops/detect_phi.rs) — entity coverage for facility/organization mentions, Id numbering from 1 in document order, and a Score that is not a per-class constant.

Cause 2 of 4: InferICD10CM returns entities with no ICD-10-CM concepts.
AWS: Smithy: InferICD10CM input Text (OntologyLinkingBoundedLengthString, required, @length 1..10000); output Entities (ICD10CMEntityList, required) and ModelVersion. Recorded behavior: each medical-condition entity carries ICD10CMConcepts — a ranked list of up to five {Code, Description, Score} (e.g. 'E11.9'/'Type 2 diabetes mellitus without complications', then 'E11.22', 'E11.65', 'E11', 'E11.0') — and the condition is segmented as the DX_NAME span 'diabetes mellitus' (offsets 55..72) with 'type 2' (48..54) reported as a QUALITY Attribute carrying its own RelationshipScore.
Emulator: ICD10CMConcepts entries are present only as nulls ($.Entities[0].ICD10CMConcepts[0].Code value_mismatch ... got None; concepts 4 and 5 missing entirely), the entity's Attributes list is empty where AWS reports the QUALITY attribute, the reported span is a different stretch of text (BeginOffset 13 / EndOffset 20 against AWS's 55 / 72), and Score is the fixed 0.949999988079071 weight.
Why: the operation's entire purpose is the code mapping. An entity list with no Codes cannot drive coding, billing or cohort selection, and a client that indexes ICD10CMConcepts[0].Code gets null instead of a code.
Fix: InferICD10CM's handler (src/ops/infer_icd10cm.rs) — it must link each detected condition to ranked ICD-10-CM concepts and emit the DX_NAME/attribute segmentation, or the operation must go back to refusing honestly rather than answering 200 with empty concept lists.

Cause 3 of 4: InferRxNorm returns medications with no RxNorm concepts and mislabelled attributes.
AWS: Smithy: InferRxNorm input Text (required, @length 1..10000); output Entities (RxNormEntityList, required) and ModelVersion. Recorded behavior for a medication sentence: the GENERIC_NAME entity 'metformin' (offsets 188..197) carries RxNormConcepts — ranked {Code, Description, Score} entries such as '861007'/'metformin hydrochloride 500 mg oral tablet', '860975', '6809'/'metformin' — and three attributes typed DOSAGE ('500 mg'), ROUTE_OR_MODE ('orally') and FREQUENCY ('twice daily'), with Ids continuing the document's single Id sequence (13 for the entity, 14..16 for its attributes).
Emulator: the entity has no RxNormConcepts member content at all, the '500 mg' attribute is typed STRENGTH instead of DOSAGE, Ids restart at 0/1/2/3 instead of continuing the document sequence, and Score/RelationshipScore are the fixed weights (0.84/0.9/0.95/0.79/0.71).
Why: a client resolving prescriptions to RxNorm codes gets none, and one keyed on the DOSAGE attribute type finds no dosage — the strength/dosage distinction is part of the modeled attribute vocabulary, not a synonym.
Fix: InferRxNorm's handler (src/ops/infer_rx_norm.rs) — RxNorm concept linking, the modeled attribute Type vocabulary, and one document-wide Id sequence shared by entities and attributes.

Cause 4 of 4: InferSNOMEDCT merges entities, drops attribute-level concepts and invents traits.
AWS: Smithy: InferSNOMEDCT input Text (required, @length 1..10000); output Entities (SNOMEDCTEntityList, required), ModelVersion, SNOMEDCTDetails, Characters. Recorded behavior: the condition is the DX_NAME entity 'diabetes mellitus' (55..72) with a QUALITY Attribute 'type 2' (48..54) that carries its OWN ranked SNOMEDCTConcepts; concept Descriptions are SNOMED fully specified names including the semantic tag in parentheses ('Diabetes mellitus type 2 (disorder)', 'Type 2 (qualifier value)'); Traits carry only what the model asserts (DIAGNOSIS); SNOMEDCTDetails reports the terminology VersionDate.
Emulator: one merged DX_NAME entity 'type 2 diabetes mellitus' (48..72) with an empty Attributes list, concept Descriptions without the semantic tag ('Diabetes mellitus type 2'), an extra PAST_HISTORY trait the recording does not have, a different SNOMEDCTDetails.VersionDate and a different ModelVersion; dozens of per-entity keys diverge as a result of the merge shifting every slot.
Why: the attribute-level concept list is how a client codes qualifiers separately from the condition; merging them loses that, and a Description without the semantic tag is not the SNOMED concept's fully specified name, so string joins against a terminology release fail.
Fix: InferSNOMEDCT's handler (src/ops/infer_snomedct.rs) — entity/attribute segmentation, concept linking at both levels including fully specified names, and trait assertion limited to what the pass can actually establish.

Related open issue: 'ComprehendMedical's text-inference operations are still stubs' (20261022T000000Z-stub-text-inference-service-com-amazonaws-comprehendmedical-comprehendmedical-20181030.md) describes these six operations as answering 501 and warns against a hand-written term table with a derived Score. It does NOT cover these four divergences: the replayed emulator answers 200, and what the replay shows is exactly the outcome that issue warned about — a lexicon pass with per-class constant scores and no terminology. The per-operation stub issues for DetectPHI, InferICD10CM, InferRxNorm and InferSNOMEDCT are all marked fixed, so no open issue owns these fidelity gaps; the replay confirms it (no stub_path, no owning issue on any of the four clusters).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator; 4 of the 9 divergence clusters, all service=comprehendmedical, verdict=mismatch, expected and actual status both 200, no stub_path and no owning issue.

1) comprehendmedical-detect-phi-note#task#1 DetectPHI: '$.Entities[3].Text: value_mismatch (expected 'Boston General Hospital', got '617-555-0199')', '$.Entities[4]: missing (expected {... Id: 5, Text: '617-555-0199', Type: 'PHONE_OR_FAX'}, got {Id: 4, Text: '42 Elm Street, Boston, MA', Type: 'ADDRESS'})', '$.Entities[5]: missing (expected {... Id: 6, Text: '42 Elm Street, Boston, MA'}, got None)', Score mismatches where the emulator emits 0.8999999761581421 / 0.949999988079071 for every entity, plus $.ModelVersion. Expected Ids run 1..6; emulator Ids run 0..4.
2) comprehendmedical-infer-icd10cm-conditions#task#1 InferICD10CM: '$.Entities[0].ICD10CMConcepts[0].Code: value_mismatch (expected 'E11.9', got None)', likewise Description/Score and concepts [1] 'E11.22' and [2] 'E11.65' -> None, '$.Entities[0].ICD10CMConcepts[3]: missing (expected {Code: 'E11', ...})', '$.Entities[0].ICD10CMConcepts[4]: missing (expected {Code: 'E11.0', ...})', '$.Entities[0].Attributes[0]: missing (expected {Type: 'QUALITY', Text: 'type 2', RelationshipScore: 0.679..., Id: 2})', '$.Entities[0].BeginOffset: value_mismatch (expected 55, got 13)', '$.Entities[0].EndOffset: value_mismatch (expected 72, got 20)', '$.Entities[0].Score: value_mismatch (expected 0.877..., got 0.949999988079071)'.
3) comprehendmedical-infer-rxnorm-medications#task#1 InferRxNorm: '$.Entities[0]: missing (expected {Id: 13, Text: 'metformin', RxNormConcepts: [{Code '861007' ...}, {'860975'}, {'6809'}, {'1807917'}, {'861819'}], Attributes: [DOSAGE '500 mg' Id 14, ROUTE_OR_MODE 'orally' Id 15, FREQUENCY 'twice daily' Id 16]}, got {Id: 0, Text: 'metformin', Attributes: [STRENGTH '500 mg' Id 1, ROUTE_OR_MODE 'orally' Id 2, FREQUENCY ... Id 3]})' — no RxNormConcepts in the actual entity; $.ModelVersion also diverges.
4) comprehendmedical-infer-snomedct-concepts#task#1 InferSNOMEDCT: '$.Entities[0]: missing (expected {Id: 3, Text: 'diabetes mellitus', BeginOffset 55, EndOffset 72, Attributes: [{Type 'QUALITY', Text 'type 2', SNOMEDCTConcepts: [{'258195006','Type 2 (qualifier value)'}, ...]}], SNOMEDCTConcepts: [{'44054006','Diabetes mellitus type 2 (disorder)'}, {'73211009','Diabetes mellitus (disorder)'}, ...], Traits: [DIAGNOSIS]}, got {Id: 0, Text: 'type 2 diabetes mellitus', BeginOffset 48, EndOffset 72, Attributes: [], Traits: [DIAGNOSIS, PAST_HISTORY], SNOMEDCTConcepts: [{Description: 'Diabetes mellitus type 2', Code: '44054006', ...}]})', plus $.SNOMEDCTDetails.VersionDate and $.ModelVersion.

Source: services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/detect_phi.rs:130-190 defines the lexicon/pattern vocabulary and an Evidence enum whose score() returns WEIGHT_PATTERN / WEIGHT_CUE / term_confidence(token count) — one constant per detection class, which is what the replayed Score values are.

Model grounding (services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json): DetectPHI output Entities + ModelVersion both required; InferICD10CM output Entities (ICD10CMEntityList) required; InferRxNorm output Entities (RxNormEntityList) required; InferSNOMEDCT output Entities (SNOMEDCTEntityList) required plus SNOMEDCTDetails and Characters.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS CLI/SDK comprehendmedical client at it.
2. Cause 1: `aws comprehendmedical detect-phi --text '<a clinical note naming a patient, two dates, a hospital, a phone number and a street address>'`. Expect one entity per PHI mention, Ids numbered from 1 in document order, including the hospital name; observe the hospital mention absent, Ids from 0, and the same constant Score repeated across entities.
3. Cause 2: `aws comprehendmedical infer-icd10cm --text '<note mentioning type 2 diabetes mellitus>'`. Expect each condition entity to carry a ranked ICD10CMConcepts list of {Code, Description, Score} and 'type 2' as a QUALITY attribute of the DX_NAME span; observe null concept entries and an empty Attributes list.
4. Cause 3: `aws comprehendmedical infer-rx-norm --text '<note prescribing metformin 500 mg orally twice daily>'`. Expect RxNormConcepts on the GENERIC_NAME entity, the '500 mg' attribute typed DOSAGE and Ids continuing one document-wide sequence; observe no concepts, Type STRENGTH, Ids restarting at 0.
5. Cause 4: `aws comprehendmedical infer-snomedct --text '<same diabetes note>'`. Expect DX_NAME 'diabetes mellitus' with a QUALITY attribute carrying its own SNOMEDCTConcepts and Descriptions that include the SNOMED semantic tag in parentheses; observe one merged entity, an empty Attributes list, tag-less Descriptions and an extra PAST_HISTORY trait.
6. Re-replay fixtures comprehendmedical-detect-phi-note#task#1, comprehendmedical-infer-icd10cm-conditions#task#1, comprehendmedical-infer-rxnorm-medications#task#1 and comprehendmedical-infer-snomedct-concepts#task#1 to confirm all four causes. Compare structure (entity set, segmentation, Id sequence, concept lists, ModelVersion/VersionDate presence), not literal confidence floats.
