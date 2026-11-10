Issue-Source: trace-sweep-divergence
Issue-Trace: comprehendmedical-detect-phi-note
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehendmedical-detect-phi-note comprehendmedical-infer-icd10cm-conditions comprehendmedical-infer-rxnorm-medications comprehendmedical-infer-snomedct-concepts
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Attributes[*].RelationshipScore
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Attributes[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*].Code
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*].Description
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].Traits[*].Score
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferRxNorm|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|InferRxNorm|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].RelationshipScore
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Category
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].EndOffset
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*].Code
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*].Description
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].SNOMEDCTConcepts[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Text
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Traits[*]
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Traits[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Type
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.SNOMEDCTDetails.VersionDate
Issue-Commit: 4ee504be713ad5d2fe853da15ad6096ca014344c

# ComprehendMedical synchronous inference answers diverge from recorded AWS analyses

> _Note: this issue was identified against the code base at commit `4ee504be713ad5d2fe853da15ad6096ca014344c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectPHI, InferICD10CM, InferRxNorm, InferSNOMEDCT

## Description
The four synchronous text-inference operations of ComprehendMedical now answer HTTP 200 (they are no longer the 501 scaffolds that issue 20261022T000000Z-stub-text-inference-service-com-amazonaws-comprehendmedical-comprehendmedical-20181030.md describes), but the lexicon/rule engine that replaced the stub does not reproduce the analysis AWS returns for the same clinical text: every confidence score is a locally synthesized number, the ontology concept lists are short, mis-ranked and partly mis-coded, whole entities are not reported at all, and the version metadata is a local constant. Replay of the corpus puts all four operations at a 200-vs-200 body mismatch. This issue has FOUR causes; all four must be closed.

NOTE on values: the floats, codes and version strings quoted below are the recorded examples, not the requirement. The requirement is that for a given input text the emulator report the entity set, the attribute set, the per-span concept list (its membership, its ordering and its cardinality) and the version metadata that AWS reports for that text; a single recording's numbers merely fill that template.

Cause 1 of 4: DetectPHI reports confidence scores no model produced (fixture comprehendmedical-detect-phi-note, step task#1).
AWS: Smithy DetectPHI -> DetectPHIResponse { Entities: EntityList (required), ModelVersion: String (required) }; Entity.Score is a Float documented as "The level of confidence that Amazon Comprehend Medical has in the accuracy of the detection". The recording shows PHI detection over a clinical note returning per-entity scores that are the model's own output for that text (e.g. 0.999757707118988, 0.999953031539917, 0.999990701675415 for the unambiguous NAME/AGE/ID spans and lower values such as 0.9676140546798706 where the span is weaker) — i.e. a confidence that tracks how determinate each span is. Recorded behavior only for the exact float of any one span.
Emulator: the entity set, spans, categories and ModelVersion all match, but every Score is replaced by a value drawn from the engine's own weight table (0.9359999895095825, 0.9813600182533264, 0.975600004196167, 0.9575999975204468, 0.975600004196167, 0.9868896007537842), so the six entities of this note diverge on Score alone and the ordering of confidence across spans is wrong (the engine scores a weak span HIGHER than the determinate ones).
Why: a client that thresholds on Score — the documented purpose of the field, and the normal way PHI redaction pipelines decide what to mask or route for human review — keeps or drops the wrong spans, because the numbers it reads are table constants rather than detection confidence and they do not even rank the spans in the same order AWS does.
Fix: the DetectPHI handler's scoring step (the confidence weights the ops module shares with the ontology-linking passes), not its entity recognition — the spans and types already agree.
Traces: comprehendmedical-detect-phi-note#task#1.

Cause 2 of 4: InferICD10CM mis-ranks and mis-codes the ICD-10-CM concept list and reports a local ModelVersion (fixture comprehendmedical-infer-icd10cm-conditions, step task#1).
AWS: Smithy InferICD10CM -> InferICD10CMResponse { Entities: ICD10CMEntityList (required), ModelVersion: String }; ICD10CMEntity carries Score, Traits, Attributes (each with Score and RelationshipScore) and ICD10CMConcepts: ICD10CMConceptList, documented as "The ICD-10-CM concepts that the entity could refer to, along with a score indicating the likelihood of the match" — a ranked candidate list (the recording carries five candidates per medical-condition entity, descending by Score, with the code whose long description best fits the mention ranked first; e.g. for a hypertensive-heart/kidney mention the recorded 4th and 5th candidates are I13.0 "Hypertensive heart and chronic kidney disease with heart failure and stage 1 through stage 4 chronic kidney disease, or unspecified chronic kidney disease" and I12.0).
Emulator: the candidate lists are ranked and populated differently — at the same positions it reports I11.9 "Hypertensive heart disease without heart failure" and I15.9 instead of I13.0 and I12.0 — every entity/attribute/trait/concept Score differs (e.g. entity 0.877546489238739 -> 0.972000002861023; concept 0.6960551738739014 -> 0.501677393913269, with the tail of the list collapsing to evenly divided values 0.2508.../0.1254.../0.0627.../0.0313... that are the first score halved repeatedly rather than per-candidate likelihoods), one recorded entity ($.Entities[2]) is not reported at all, and $.ModelVersion is the crate constant MODEL_VERSION = "1.1.0" rather than the version the recording reports.
Why: a coding client reads ICD10CMConcepts[0] as the suggested code and the rest as the review list; a different first/4th/5th candidate is a different clinical code, a missing entity is a condition silently not coded, and a halving sequence is not a likelihood distribution so any confidence cutoff behaves differently against the emulator. ModelVersion is the field AWS documents for tracking which model coded a batch, so a fixed "1.1.0" makes every batch look like it came from a model AWS never ran.
Fix: the InferICD10CM handler's lexicon/linking tables and ranking (the shared ontology-linking pass), its confidence weighting, and the MODEL_VERSION constant in that op module.
Traces: comprehendmedical-infer-icd10cm-conditions#task#1.

Cause 3 of 4: InferRxNorm does not report the recorded medication entities or their RxNorm concepts (fixture comprehendmedical-infer-rxnorm-medications, step task#1).
AWS: Smithy InferRxNorm -> InferRxNormResponse { Entities: RxNormEntityList (required), ModelVersion: String }; each medication entity carries Score, Attributes (DOSAGE / ROUTE_OR_MODE / FREQUENCY, each with Score and RelationshipScore) and RxNormConcepts, whose Code is "RxNorm concept ID, also known as the RxCUI" with its Description and match Score. The recording returns, for a metformin mention, five RxNorm candidates (861007 "metformin hydrochloride 500 mg oral tablet", 860975, 6809 "metformin", 1807917, 861819) ranked by Score, with Id values continuing the response-wide monotonic numbering (entity Id 13, attribute Ids 14-16).
Emulator: the whole recorded entity object at $.Entities[0] (and likewise $.Entities[1]) is absent as recorded — the emulator emits an entity for the same span and type but with NO RxNormConcepts list at all, different Scores/RelationshipScores (0.821601390838623 -> 0.8399999737739563; attribute 0.8617406487464905 -> 0.9660000205039978 / 0.8999999761581421 / 0.9359999895095825) and Ids restarted at 1 instead of continuing the response numbering; $.ModelVersion again differs.
Why: InferRxNorm exists to turn a medication mention into an RxCUI. With RxNormConcepts missing, a client gets a medication entity it cannot code at all — a null dereference or an empty coding result rather than a normalized drug — and the restarted Id numbering breaks the attribute-to-entity joins that AWS's monotonic per-response ids are documented to support.
Fix: the InferRxNorm handler's RxNorm linking table (populate the ranked concept list for every medication entity), its id allocation across the response, and its confidence weighting.
Traces: comprehendmedical-infer-rxnorm-medications#task#1.

Cause 4 of 4: InferSNOMEDCT truncates concept lists to one candidate, mis-codes them, drops entities and reports a local SNOMED CT revision (fixture comprehendmedical-infer-snomedct-concepts, step task#1).
AWS: Smithy InferSNOMEDCT -> InferSNOMEDCTResponse { Entities: SNOMEDCTEntityList (required), ModelVersion: String, SNOMEDCTDetails, Characters }; SNOMEDCTEntity and SNOMEDCTAttribute each carry SNOMEDCTConcepts ("the SNOMED concepts that the entity could refer to, along with a score indicating the likelihood of the match"), and SNOMEDCTDetails carries Edition, Language and VersionDate — "The version date of the SNOMED-CT ontology used". The recording returns five ranked candidates per span, on ATTRIBUTES as well as entities (e.g. an attribute's list continues 44054006 "Diabetes mellitus type 2 (disorder)", 258199000, 410657003, 405751000 after its first candidate), and codes each mention to the concept its own written form names (422014003 "Disorder due to type 2 diabetes mellitus (disorder)", 472969004 "History of diabetes mellitus type 2 (situation)").
Emulator: attribute concept lists are truncated to a single candidate (recorded entries 2-5 are simply absent), entity candidates at the same ranks carry different codes and descriptions (237599002 "Insulin treated type 2 diabetes mellitus (disorder)" and 422034002 "Diabetic retinopathy associated with type 2 diabetes mellitus (disorder)" where the recording has 422014003 and 472969004), several recorded entities are missing entirely ($.Entities[4], $.Entities[5]) and one ($.Entities[3]) is reported with a different Text, Type, Category, EndOffset, traits and attributes, two attributes come back with SNOMEDCTConcepts absent, and the response metadata is the module's constants ModelVersion "1.1.0" and SNOMEDCTDetails.VersionDate "2021-09-01" (DETAILS_VERSION_DATE) instead of the revision the recording reports.
Why: the candidate list is the whole product of this operation; a one-element list removes the review candidates a terminology-mapping client is built around, a different code is a different clinical concept, and a missing entity is a concept never mapped. SNOMEDCTDetails.VersionDate is how a client records which ontology revision produced a mapping, so a hard-coded date mislabels every stored mapping's provenance.
Fix: the InferSNOMEDCT handler's SNOMED CT lexicon and linking pass (emit the full ranked candidate list for entities AND attributes, code each span from its own written form, recognize the recorded entity set), its confidence weighting, and the MODEL_VERSION / DETAILS_VERSION_DATE constants.
Traces: comprehendmedical-infer-snomedct-concepts#task#1.

Related open issues (neither covers these replay keys): 20261022T000000Z-stub-text-inference-...md still describes these six operations as 501 stubs and warns against exactly the hand-written-table-plus-derived-score approach now serving them; 20261029T172356Z-comprehendmedical-asynchronous-job-error-contract-diverges-from-aws.md owns the DescribePHIDetectionJob validation-message divergence and is excluded here.

## Evidence
Replay of trace set simparity-wire-aws-v1 (28 steps, 8 passed, 20 mismatched, score 28.6%) reported four net-new comprehendmedical clusters, all verdict=mismatch with expected_status 200 and actual_status 200, none owned and none stub-blocked:

1) comprehendmedical-detect-phi-note#task#1 — DetectPHI. $.Entities[0..5].Score all value_mismatch, e.g. expected 0.999757707118988 got 0.9359999895095825; expected 0.999953031539917 got 0.9813600182533264; expected 0.999990701675415 got 0.975600004196167; expected 0.981614887714386 got 0.9575999975204468; expected 0.9972621202468872 got 0.975600004196167; expected 0.9676140546798706 got 0.9868896007537842. Entity spans, types and ModelVersion agree.

2) comprehendmedical-infer-icd10cm-conditions#task#1 — InferICD10CM. Diverging keys include $.Entities[0].Score (expected 0.877546489238739, got 0.972000002861023), $.Entities[0].Attributes[0].Score / .RelationshipScore, $.Entities[0].Traits[0].Score, $.Entities[0].ICD10CMConcepts[0..4].Score (expected 0.6960551738739014/0.2902306020259857/0.26375195384025574/0.2445349246263504/0.2381153106689453, got 0.501677393913269/0.2508386969566345/0.12541934847831726/0.06270967423915863/0.031354837119579315), $.Entities[1].ICD10CMConcepts[3].Code (expected 'I13.0', got 'I11.9') and .Description (expected 'Hypertensive heart and chronic kidney disease with heart failure and stage 1 through stage 4 chronic kidney disease, or unspecified chronic kidney disease', got 'Hypertensive heart disease without heart failure'), $.Entities[1].ICD10CMConcepts[4].Code (expected 'I12.0', got 'I15.9'), $.Entities[2] (missing) and $.ModelVersion.

3) comprehendmedical-infer-rxnorm-medications#task#1 — InferRxNorm. $.Entities[0] missing: expected the metformin GENERIC_NAME entity with Id 13, Score 0.821601390838623, attributes Id 14-16 ('500 mg' DOSAGE, 'orally' ROUTE_OR_MODE, 'twice daily' FREQUENCY) each Score 0.8617406487464905, and RxNormConcepts [861007 'metformin hydrochloride 500 mg oral tablet' 0.3430459499359131, 860975, 6809 'metformin', 1807917, 861819]; got an entity with Id 1, Score 0.8399999737739563, attribute Ids 2-4 with Scores 0.9660000205039978/0.8999999761581421/0.9359999895095825 and NO RxNormConcepts member. Same for $.Entities[1]; $.ModelVersion also diverges.

4) comprehendmedical-infer-snomedct-concepts#task#1 — InferSNOMEDCT. $.Entities[0].Attributes[0].SNOMEDCTConcepts[1..4] missing (expected 44054006 'Diabetes mellitus type 2 (disorder)' 0.04262993857264519, 258199000 'Type 3 (qualifier value)', 410657003 'Type (attribute)', 405751000 'Diabetes type (observable entity)'; got None); $.Entities[0].SNOMEDCTConcepts[2].Code expected '422014003' 'Disorder due to type 2 diabetes mellitus (disorder)' got '237599002' 'Insulin treated type 2 diabetes mellitus (disorder)'; [3].Code expected '472969004' 'History of diabetes mellitus type 2 (situation)' got '422034002' 'Diabetic retinopathy associated with type 2 diabetes mellitus (disorder)'; $.Entities[2].Attributes[0..1].SNOMEDCTConcepts diverge, $.Entities[3].{Text,Type,Category,EndOffset,Traits[0],Attributes[0..2],SNOMEDCTConcepts[0..4]} diverge, $.Entities[4] and $.Entities[5] missing, plus $.ModelVersion and $.SNOMEDCTDetails.VersionDate.

Emulator-side source of the diverging metadata: services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/infer_snomedct.rs line 133 `const MODEL_VERSION: &str = "1.1.0";` and line 140 `const DETAILS_VERSION_DATE: &str = "2021-09-01";`, serialized at lines 2778-2782.

Smithy citations: DetectPHI.Entities/ModelVersion required, Entity.Score Float "level of confidence ... in the accuracy of the detection"; ICD10CMEntity.ICD10CMConcepts -> ICD10CMConceptList of ICD10CMConcept{Code,Description,Score}; RxNormConcept.Code "RxNorm concept ID, also known as the RxCUI"; SNOMEDCTEntity.SNOMEDCTConcepts and SNOMEDCTAttribute concepts; SNOMEDCTDetails{Edition,Language,VersionDate} "The version date of the SNOMED-CT ontology used"; Entity.Id "monotonically increasing id unique within this response".

## Steps to Reproduce
1. Boot the emulator from this checkout and point a comprehendmedical client at it.
2. DetectPHI: replay fixture comprehendmedical-detect-phi-note step task#1 (aws comprehendmedical detect-phi --text <the fixture's clinical note>). The response is 200 with the right entities, but every Entities[*].Score comes from the engine's weight table rather than from detection confidence, and the relative ranking of the spans differs from the recording.
3. InferICD10CM: replay comprehendmedical-infer-icd10cm-conditions step task#1. Compare Entities[*].ICD10CMConcepts: the candidate codes/descriptions at ranks 4-5 differ, the tail Scores are the head Score halved repeatedly, one recorded entity is missing, and ModelVersion is "1.1.0".
4. InferRxNorm: replay comprehendmedical-infer-rxnorm-medications step task#1. Entities[0] and Entities[1] come back with no RxNormConcepts member at all and with Ids restarted at 1 instead of continuing the response-wide numbering.
5. InferSNOMEDCT: replay comprehendmedical-infer-snomedct-concepts step task#1. Attribute SNOMEDCTConcepts lists hold a single candidate instead of the recorded five, entity candidate codes differ, Entities[3] is reported with a different span/type/category, Entities[4] and Entities[5] are absent, and SNOMEDCTDetails.VersionDate is "2021-09-01".
6. Re-replay all four fixtures after the fix: each must match the recorded body (entity set, attributes, full ranked concept lists and version metadata), not only the first one.
