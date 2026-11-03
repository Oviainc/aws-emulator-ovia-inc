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
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts[*].Code
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Attributes[*].SNOMEDCTConcepts[*].Description
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
Issue-Commit: b7089938ce6288d22e31bd61b58c939edb920b29

# ComprehendMedical detection and ontology-linking responses are synthetically generated

> _Note: this issue was identified against the code base at commit `b7089938ce6288d22e31bd61b58c939edb920b29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectPHI

## Description
FOUR causes, one shared fault. The comprehendmedical synchronous inference family (DetectPHI, InferICD10CM, InferRxNorm, InferSNOMEDCT) answers 200 with a response body that is assembled by an emulator-local keyword/offset generator instead of reproducing the modeled Comprehend Medical inference output for the submitted text. The HTTP status, the protocol framing and the entity offsets are right; what is wrong is the payload content: the entity set (count and membership), the monotonic Id numbering, the ontology concept lists (Code + Description) and the model/ontology version fields. Every cause below is the same generator seen through a different operation, so they should be fixed together, but each must be closed on its own fixture.

Note on scope: this is NOT the stub issue '20261022T000000Z-stub-text-inference-service-com-amazonaws-comprehendmedical-comprehendmedical-20181030.md' (ComprehendMedical's text-inference operations are still stubs) — these four operations are implemented and serving 200s, and the replay reports no stub_path for any of them. It is also unrelated to the asynchronous-job error-contract issues ('...asynchronous-job-error-contract-diverges-from-aws.md', '...describephidetectionjob-refuses-an-unknown-jobid-with-the.md'), which cover the Start/Describe/Stop job family.

Cause 1 of 4: DetectPHI returns fabricated confidence scores for every detected entity (fixture comprehendmedical-detect-phi-note, step task#1).
AWS: Smithy: DetectPHI -> DetectPHIResponse with required members Entities (EntityList) and ModelVersion; Entity.Score is a Float documented as 'The level of confidence that Amazon Comprehend Medical has in the accuracy of the detection'. The model does not fix numeric values, so for the concrete values this is recorded behavior only: the recording for this clinical note returns six entities whose Scores are the detector's own per-entity confidences (0.999757707118988, 0.999953031539917, 0.999990701675415, 0.981614887714386, 0.9972621202468872, 0.9676140546798706 in entity order), i.e. values that differ per entity and derive from the text span being scored.
Emulator: status 200 and the entity set, types and offsets agree, but all six Scores are drawn from a small fixed palette of emulator constants and are re-used across entities and across operations (0.9359999895095825, 0.9813600182533264, 0.975600004196167 twice, 0.9575999975204468, 0.9868896007537842). The same constants 0.9359999895095825 / 0.975600004196167 reappear in the InferICD10CM and InferSNOMEDCT responses below, which is what identifies them as generator constants rather than scores computed for this text.
Why: a client that thresholds on Score (the normal way PHI redaction pipelines decide what to mask or send to human review) gets a different decision against the emulator than against AWS, and a client that calibrates a threshold on emulator traffic ships a threshold that is meaningless in production. Because the constants repeat, a test that asserts 'the two distinct entities got distinct confidences' cannot be written at all.
Fix: the DetectPHI handler's response builder in the comprehendmedical service crate — specifically the scoring step of its detection generator, which must emit the per-span confidence of the detector it is emulating rather than a constant table shared across entities and operations.
Traces: comprehendmedical-detect-phi-note#task#1 (6 diverging JSON paths: $.Entities[0..5].Score).

Cause 2 of 4: InferICD10CM drops an entity, returns the wrong ICD-10-CM concept codes and descriptions, and the wrong ModelVersion (fixture comprehendmedical-infer-icd10cm-conditions, step task#1).
AWS: Smithy: InferICD10CM -> InferICD10CMResponse { Entities: ICD10CMEntityList (required), PaginationToken, ModelVersion }; ICD10CMConcept { Code — 'The ICD-10-CM code that identifies the concept found in the knowledge base from the Centers for Disease Control', Description — 'The long description of the ICD-10-CM code in the ontology', Score }. Code and Description are therefore ontology data: for a given Code, the Description is fixed by the CDC ICD-10-CM release and is not a per-run value. Recorded behavior for this note: three entities, each with a five-member ICD10CMConcepts list ordered by descending Score, and for the hypertension entity the 4th and 5th concepts are Code 'I13.0' / 'Hypertensive heart and chronic kidney disease with heart failure and stage 1 through stage 4 chronic kidney disease, or unspecified chronic kidney disease' and Code 'I12.0' / 'Hypertensive chronic kidney disease with stage 5 chronic kidney disease or ...'.
Emulator: status 200, but (a) $.Entities[2] is missing entirely — the emulator returns fewer entities than the recording for the same Text; (b) the ICD10CMConcepts candidate lists differ in membership: at $.Entities[1].ICD10CMConcepts[3] it returns Code 'I11.9' / 'Hypertensive heart disease without heart failure' and at [4] Code 'I15.9' instead of 'I13.0' and 'I12.0'; (c) every Score, RelationshipScore and Trait Score diverges and the concept Scores follow a visible synthetic ramp (0.501677393913269, 0.2508386969566345, 0.12541934847831726, 0.06270967423915863, 0.031354837119579315 — successive halvings) rather than ontology-match confidences; (d) $.ModelVersion diverges from the recorded model version string.
Why: ICD-10-CM linking output feeds coding and billing logic. A missing entity means a condition in the note is silently never coded; a wrong Code/Description pair means the client codes the encounter to a different diagnosis (I11.9 'without heart failure' instead of I13.0 'with heart failure and CKD' is a different billable condition); and the halving ramp makes any candidate-ranking or score-cutoff logic untestable. A wrong ModelVersion defeats clients that pin or audit the inference model version.
Fix: the InferICD10CM handler's response builder and the ICD-10-CM ontology table it draws candidate Code/Description pairs from, plus whatever reports ModelVersion, in the comprehendmedical service crate.
Traces: comprehendmedical-infer-icd10cm-conditions#task#1 (22 diverging JSON paths, including $.Entities[2] missing, $.Entities[1].ICD10CMConcepts[3].Code/.Description, $.Entities[1].ICD10CMConcepts[4].Code/.Description, $.ModelVersion).

Cause 3 of 4: InferRxNorm returns a structurally different medication entity — Id numbering restarts at 1 and the RxNorm concept list does not match the ontology (fixture comprehendmedical-infer-rxnorm-medications, step task#1).
AWS: Smithy: InferRxNorm -> InferRxNormResponse { Entities: RxNormEntityList (required), PaginationToken, ModelVersion }; RxNormConcept { Code — 'RxNorm concept ID, also known as the RxCUI', Description, Score }; Attribute.Id — 'a monotonically increasing id unique within this response'. Recorded behavior: the 'metformin' GENERIC_NAME entity carries Id 13 with attributes Id 14 (DOSAGE '500 mg'), 15 (ROUTE_OR_MODE 'orally'), 16 (FREQUENCY 'twice daily'), and a five-member RxNormConcepts list of real RxCUIs ordered by descending Score: 861007 'metformin hydrochloride 500 mg oral tablet', 860975 '24 hr metformin hydrochloride 500 mg extended release oral tablet', 6809 'metformin', 1807917, 861819.
Emulator: status 200, but $.Entities[0] is replaced wholesale and $.Entities[1] likewise: the same 'metformin' span (BeginOffset 188 / EndOffset 197, which does match) comes back as Id 1 with attributes Id 2/3/4, its Scores and RelationshipScores are generator constants (0.8399999737739563, 0.9660000205039978, 0.949999988079071, 0.8999999761581421, 0.7899999618530273, 0.9359999895095825, 0.7099999785423279), and its RxNormConcepts content does not match the recorded RxCUI candidate list. $.ModelVersion also diverges.
Why: the Id space is the only thing tying an Attribute back to the entity it modifies, so a client that merges InferRxNorm output with DetectEntitiesV2 output, or that stores Ids across calls, mis-associates dosage/route/frequency with the wrong medication. A RxNormConcepts list that is not the real RxCUI candidate set means medication reconciliation resolves to the wrong drug product, which is the single highest-consequence output of this API.
Fix: the InferRxNorm handler's response builder — its entity/attribute Id allocator (must be a monotonic counter over the whole response, matching the recorded numbering semantics) and the RxNorm concept table it ranks candidates from, in the comprehendmedical service crate.
Traces: comprehendmedical-infer-rxnorm-medications#task#1 (diverging JSON paths $.Entities[0], $.Entities[1], $.ModelVersion — whole-object replacements).

Cause 4 of 4: InferSNOMEDCT returns the wrong entity set, wrong SNOMED-CT concepts, no concepts on attributes, and wrong ontology version metadata (fixture comprehendmedical-infer-snomedct-concepts, step task#1).
AWS: Smithy: InferSNOMEDCT -> InferSNOMEDCTResponse { Entities: SNOMEDCTEntityList (required), PaginationToken, ModelVersion, SNOMEDCTDetails, Characters }; SNOMEDCTDetails { Edition — 'the US edition', Language — 'US English (en)', VersionDate — 'The version date of the SNOMED-CT ontology used' }. SNOMEDCTDetails is therefore a published constant triple for the ontology release the service runs, not a per-run value, and SNOMEDCTConcept Code/Description pairs are fixed by that SNOMED-CT release. Recorded behavior: six entities; attributes carry their own SNOMEDCTConcepts lists (e.g. the diabetes-type attribute links 258199000 'Type 3 (qualifier value)', 410657003 'Type (attribute)', 405751000 'Diabetes type (observable entity)'); entity-level lists are five members ordered by descending Score (e.g. 422014003).
Emulator: status 200, but ~70 JSON paths diverge: $.Entities[4] and $.Entities[5] are missing (four entities instead of six); $.Entities[3] is a different entity altogether (Category, Type, Text and EndOffset all differ); $.Entities[2].Attributes[0].SNOMEDCTConcepts and [1].SNOMEDCTConcepts are missing (attributes come back with no linked concepts at all); the concepts that are returned are the wrong ontology rows — at $.Entities[0].Attributes[0].SNOMEDCTConcepts[2..4] it returns 73211009 'Diabetes mellitus (disorder)', 237599002 'Insulin treated type 2 diabetes mellitus (disorder)' and 422034002 'Diabetic retinopathy associated with type 2 diabetes mellitus (disorder)' where the recording has 258199000 'Type 3 (qualifier value)', 410657003 'Type (attribute)' and 405751000 'Diabetes type (observable entity)' — i.e. the generator links the whole-entity concept instead of the attribute's own concept; the Scores again follow the synthetic halving ramp (0.48309677839279175, 0.24154838919639587, 0.12077419459819794, 0.06038709729909897, 0.030193548649549484); and $.ModelVersion plus $.SNOMEDCTDetails.VersionDate diverge from the published ontology release values.
Why: a client walking SNOMEDCTDetails to decide which ontology release to reconcile against is told the wrong release; attributes with an empty concept list make attribute-level linking look unsupported; missing entities silently drop findings; and attribute concepts that are copies of the parent entity's concepts invert the meaning of the output (the qualifier 'Type 3' becomes the disorder 'Diabetes mellitus').
Fix: the InferSNOMEDCT handler's response builder in the comprehendmedical service crate — the entity enumerator, the attribute-level concept linking path (which currently yields nothing or the parent entity's concepts), the SNOMED-CT concept table, and the constants that populate ModelVersion and SNOMEDCTDetails.Edition/Language/VersionDate.
Traces: comprehendmedical-infer-snomedct-concepts#task#1 (~70 diverging JSON paths, including $.Entities[4] and $.Entities[5] missing, $.Entities[3].Category/.Type/.Text/.EndOffset, $.Entities[2].Attributes[0].SNOMEDCTConcepts missing, $.ModelVersion, $.SNOMEDCTDetails.VersionDate).

No cause here is a per-run artifact: there is no request id, clock value, host or generated name among the diverging paths. Every diverging path is response content the emulator computes deterministically from the request Text, so each is reproducible by replaying its fixture step.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, /__admin/health 200): 28 steps, 4 passed, 24 mismatched, score 14.3%. Four net-new divergence clusters, all verdict=mismatch, all with empty stub_path and empty owned_by:

1) fixture comprehendmedical-detect-phi-note step task#1, DetectPHI, expected 200 / actual 200:
   $.Entities[0].Score: expected 0.999757707118988, got 0.9359999895095825
   $.Entities[1].Score: expected 0.999953031539917, got 0.9813600182533264
   $.Entities[2].Score: expected 0.999990701675415, got 0.975600004196167
   $.Entities[3].Score: expected 0.981614887714386, got 0.9575999975204468
   $.Entities[4].Score: expected 0.9972621202468872, got 0.975600004196167
   $.Entities[5].Score: expected 0.9676140546798706, got 0.9868896007537842

2) fixture comprehendmedical-infer-icd10cm-conditions step task#1, InferICD10CM, expected 200 / actual 200:
   $.Entities[2]: missing (recording has a third entity; emulator returns two)
   $.Entities[1].ICD10CMConcepts[3].Code: expected 'I13.0', got 'I11.9'
   $.Entities[1].ICD10CMConcepts[3].Description: expected 'Hypertensive heart and chronic kidney disease with heart failure and stage 1 through stage 4 chronic kidney disease, or unspecified chronic kidney disease', got 'Hypertensive heart disease without heart failure'
   $.Entities[1].ICD10CMConcepts[4].Code: expected 'I12.0', got 'I15.9'
   $.Entities[1].ICD10CMConcepts[4].Description: expected 'Hypertensive chronic kidney disease with stage 5 chronic kidney disease or ...', got a different ontology description
   $.Entities[0].ICD10CMConcepts[0..4].Score: expected 0.6960551738739014 / 0.2902306020259857 / 0.26375195384025574 / 0.2445349246263504 / 0.2381153106689453, got 0.501677393913269 / 0.2508386969566345 / 0.12541934847831726 / 0.06270967423915863 / 0.031354837119579315 (successive halvings)
   $.Entities[0].Score: expected 0.877546489238739, got 0.972000002861023
   $.Entities[0].Attributes[0].RelationshipScore: expected 0.6795811653137207, got 0.949999988079071
   $.ModelVersion: value_mismatch
   (22 diverging paths in the cluster's divergence_key)

3) fixture comprehendmedical-infer-rxnorm-medications step task#1, InferRxNorm, expected 200 / actual 200:
   $.Entities[0]: whole object replaced. Expected entity Id 13, Score 0.821601390838623, Text 'metformin', Type GENERIC_NAME, Attributes Id 14 ('500 mg', DOSAGE, Score 0.8617406487464905, RelationshipScore 0.8813977241516113) / Id 15 ('orally', ROUTE_OR_MODE, RelationshipScore 0.8672217726707458) / Id 16 ('twice daily', FREQUENCY), and RxNormConcepts [861007 'metformin hydrochloride 500 mg oral tablet' 0.3430459499359131, 860975 '24 hr metformin hydrochloride 500 mg extended release oral tablet' 0.027012230828404427, 6809 'metformin' 0.01818265020847321, 1807917 ... 0.007255574688315392, 861819 ... 0.005996042862534523].
   Got entity Id 1, Score 0.8399999737739563, same Text/Type/offsets (188-197), Attributes Id 2 (DOSAGE, Score 0.9660000205039978, RelationshipScore 0.949999988079071) / Id 3 (ROUTE_OR_MODE, 0.8999999761581421 / 0.7899999618530273) / Id 4 (FREQUENCY, 0.9359999895095825 / 0.7099999785423279), with a non-matching RxNormConcepts list.
   $.Entities[1]: whole object replaced. $.ModelVersion: value_mismatch.

4) fixture comprehendmedical-infer-snomedct-concepts step task#1, InferSNOMEDCT, expected 200 / actual 200:
   $.Entities[4], $.Entities[5]: missing (four entities instead of six)
   $.Entities[3].Category, .Type, .Text, .EndOffset, .Score, .Traits[0], .Attributes[0..2], .SNOMEDCTConcepts[0..4]: all diverge (a different entity)
   $.Entities[2].Attributes[0].SNOMEDCTConcepts, $.Entities[2].Attributes[1].SNOMEDCTConcepts: missing (attributes returned with no linked concepts)
   $.Entities[0].Attributes[0].SNOMEDCTConcepts[2].Code: expected '258199000', got '73211009'; .Description: expected 'Type 3 (qualifier value)', got 'Diabetes mellitus (disorder)'
   $.Entities[0].Attributes[0].SNOMEDCTConcepts[3].Code: expected '410657003', got '237599002'; .Description: expected 'Type (attribute)', got 'Insulin treated type 2 diabetes mellitus (disorder)'
   $.Entities[0].Attributes[0].SNOMEDCTConcepts[4].Code: expected '405751000', got '422034002'; .Description: expected 'Diabetes type (observable entity)', got 'Diabetic retinopathy associated with type 2 diabetes mellitus (disorder)'
   $.Entities[0].Attributes[0].SNOMEDCTConcepts[0..4].Score: expected 0.666792631149292 / 0.04262993857264519 / 0.018421370536088943 / 0.016781525686383247 / 0.014690077863633633, got 0.48309677839279175 / 0.24154838919639587 / 0.12077419459819794 / 0.06038709729909897 / 0.030193548649549484 (successive halvings)
   $.Entities[0].SNOMEDCTConcepts[2].Code: expected '422014003', got a different code
   $.ModelVersion, $.SNOMEDCTDetails.VersionDate: value_mismatch
   (~70 diverging paths in the cluster's divergence_key)

Model evidence used: Smithy model services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/smithy-model/com-amazonaws-comprehendmedical-comprehendmedical-20181030.json — DetectPHI (DetectPHIResponse.Entities required, ModelVersion required), InferICD10CM/InferRxNorm/InferSNOMEDCT (Entities required, ModelVersion optional), ICD10CMConcept.Code 'the ICD-10-CM code ... from the Centers for Disease Control' + Description 'the long description of the ICD-10-CM code in the ontology', RxNormConcept.Code 'RxNorm concept ID, also known as the RxCUI', Attribute.Id 'a monotonically increasing id unique within this response', SNOMEDCTDetails.Edition/Language/VersionDate (US edition, en, ontology version date). Numeric score values are recorded behavior only; the Code/Description pairings, the entity/attribute Id semantics and the ontology version metadata are model-stated or ontology-fixed.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; confirm /__admin/health answers 200.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus with the corpus bound). All four causes reproduce deterministically, each from its own fixture step:
   Cause 1: fixture comprehendmedical-detect-phi-note, step task#1 — POST the fixture's DetectPHI request (X-Amz-Target ComprehendMedical_20181030.DetectPHI) with the recorded clinical-note Text and compare $.Entities[*].Score against the recorded response.
   Cause 2: fixture comprehendmedical-infer-icd10cm-conditions, step task#1 — POST the recorded InferICD10CM request and compare the entity count, $.Entities[1].ICD10CMConcepts[3..4].Code/.Description and $.ModelVersion.
   Cause 3: fixture comprehendmedical-infer-rxnorm-medications, step task#1 — POST the recorded InferRxNorm request and compare $.Entities[0] (entity Id, attribute Ids, RxNormConcepts) and $.ModelVersion.
   Cause 4: fixture comprehendmedical-infer-snomedct-concepts, step task#1 — POST the recorded InferSNOMEDCT request and compare the entity count, $.Entities[2].Attributes[*].SNOMEDCTConcepts (present vs missing), $.Entities[0].Attributes[0].SNOMEDCTConcepts[2..4].Code/.Description, $.ModelVersion and $.SNOMEDCTDetails.VersionDate.
3. Each step returns HTTP 200 in both cases, so the divergence is visible only by diffing the JSON body against the recorded response. All four must be re-replayed after a fix: fixing one operation's generator does not touch the other three.
