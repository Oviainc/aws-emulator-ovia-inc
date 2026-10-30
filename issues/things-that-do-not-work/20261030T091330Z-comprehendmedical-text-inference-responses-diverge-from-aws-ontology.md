Issue-Source: trace-sweep-divergence
Issue-Trace: comprehendmedical-detect-phi-note
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehendmedical-detect-phi-note comprehendmedical-infer-icd10cm-conditions comprehendmedical-infer-rxnorm-medications comprehendmedical-infer-snomedct-concepts
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.Entities[*].Score
Issue-Key: comprehendmedical|DetectPHI|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*]
Issue-Key: comprehendmedical|InferICD10CM|2xx->2xx|$.Entities[*].ICD10CMConcepts[*]
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
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Traits[*].Score
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.Entities[*].Type
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.ModelVersion
Issue-Key: comprehendmedical|InferSNOMEDCT|2xx->2xx|$.SNOMEDCTDetails.VersionDate
Issue-Commit: 97b52288e8c9303330ae8e79df94bfb75ff84489

# ComprehendMedical text-inference responses diverge from AWS ontology linking

> _Note: this issue was identified against the code base at commit `97b52288e8c9303330ae8e79df94bfb75ff84489`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectPHI

## Description
The four synchronous text-inference operations (DetectPHI, InferICD10CM, InferRxNorm, InferSNOMEDCT) all answer 200 with a response body built from a small canned table instead of from the recorded Comprehend Medical inference contract. In every case the status is right and the envelope is right, but the entity set, the ontology-concept lists, the confidence ladder and ModelVersion are wrong, so a client reads a well-formed but clinically wrong answer. There are FOUR causes in this bundle; all four must be closed.

Cause 1 of 4 - DetectPHI returns a placeholder ModelVersion and canned entity confidences. Fixture comprehendmedical-detect-phi-note, step task#1.
AWS: Smithy: DetectPHI output DetectPHIResponse with Entities (EntityList, @required) and ModelVersion (String, @required, @length min 1); ModelVersion is documented as the version of the model used for the analysis, i.e. the service's real PHI model version, and Entity.Score is documented as "the level of confidence that Amazon Comprehend Medical has in the accuracy of the detection", i.e. a per-entity value derived from the detection. Recorded behavior: ModelVersion is the live PHI model version string (the recording shows "1.1.0") and each of the six detected entities carries its own distinct Score.
Emulator: ModelVersion is "0.1.0" - a scaffold placeholder, not a Comprehend Medical model version - and all six entity Scores come from a fixed ladder of reused constants (0.9359999895095825, 0.975600004196167 three times, 0.9575999975204468, 0.9848160147666931) rather than being per-entity.
Why: a client that pins or logs ModelVersion for auditability (required for regulated PHI workflows) records a version that no Comprehend Medical model ever had, and a client that thresholds on Score (the normal way to gate PHI redaction) sees the same handful of values for every entity, so its threshold logic is untestable against the emulator.
Fix: the DetectPHI handler in the comprehendmedical service crate - the response builder that fills ModelVersion and the per-entity Score, not the serializer (the shape is already correct).
Traces: comprehendmedical-detect-phi-note#task#1.

Cause 2 of 4 - InferICD10CM emits an entity type the operation does not detect and truncates/mislinks ICD10CMConcepts. Fixture comprehendmedical-infer-icd10cm-conditions, step task#1.
AWS: Smithy: InferICD10CM output Entities targets ICD10CMEntityList; ICD10CMEntity.Category targets ICD10CMEntityCategory whose only value is MEDICAL_CONDITION, and ICD10CMEntity.Type targets ICD10CMEntityType with values DX_NAME and TIME_EXPRESSION, documented as "InferICD10CM detects entities of the type DX_NAME and TIME_EXPRESSION". ICD10CMConcepts is a list of ICD10CMConcept {Code, Description, Score} documented as "the ICD-10-CM concepts that the entity could refer to, along with a score indicating the likelihood of the match". Recorded behavior: each MEDICAL_CONDITION/DX_NAME entity carries five candidate concepts ranked by descending Score, and every Code/Description pair belongs to the concept family of that entity's own text (e.g. the hypertension entity links to I11.x/I12.x hypertensive codes).
Emulator: the first returned entity is {Id: 1, Text: '65-year', Category: 'MEDICAL_CONDITION', Type: 'TIME_EXPRESSION', Score: 0.966, Attributes: [], Traits: []} where the recording has the diabetes-mellitus DX_NAME entity with its QUALITY attribute and five ICD-10-CM concepts, so the entity list is both populated differently and shifted; the concept list is truncated to three entries (ICD10CMConcepts[3] and [4] are absent); and the surviving concepts are linked to the wrong entity - the second entity gets Code 'E11.8' / 'Type 2 diabetes mellitus with unspecified complications' and 'E11.65' where AWS returns 'I11.0' / 'Hypertensive heart disease with heart failure' and 'I12.9'. Concept Scores again come from the canned ladder (0.972000002861023, 0.6998400092124939).
Why: a client resolving free-text conditions to billable ICD-10-CM codes receives codes for a different disease than the text it sent, with a high confidence attached, and a client that pages the five candidates for clinician review only ever sees three.
Fix: the InferICD10CM handler's ontology-linking step in the comprehendmedical service crate - entity extraction (which entities and which Type are emitted for a given span) and the ICD-10-CM candidate lookup/ranking that attaches concepts to an entity.
Traces: comprehendmedical-infer-icd10cm-conditions#task#1.

Cause 3 of 4 - InferRxNorm returns a different medication entity shape and RxNorm candidate set than the recording. Fixture comprehendmedical-infer-rxnorm-medications, step task#1.
AWS: Smithy: InferRxNorm output Entities targets RxNormEntityList; each entity carries RxNormConcepts, a list of RxNormConcept {Code (the RxCUI), Description, Score}. Recorded behavior: the MEDICATION/GENERIC_NAME entity for the drug mention carries five RxNorm candidates ranked by sharply descending Score (the recording's top candidate scores ~0.34 and the fifth ~0.006), its DOSAGE/ROUTE_OR_MODE/FREQUENCY attributes carry Ids continuing the response-wide monotonic Id sequence (13..16 in the recording), and each attribute's Score and RelationshipScore are per-attribute values.
Emulator: the entity is rebuilt with Ids restarting at 1 (entity Id 1, attributes Id 2,3,4) instead of continuing the response-wide sequence, RxNormConcepts is not the recorded candidate set, and attribute Score/RelationshipScore come from the same canned ladder (0.966/0.95, 0.9/0.79, 0.936/0.71). ModelVersion also diverges.
Why: a client that maps a prescription string to an RxCUI gets a different code set and cannot distinguish a confident match from a long-tail candidate, because the emulator's scores never fall off the way real linking scores do; a client that joins attributes to entities by the monotonic Id sequence documented on RxNormEntity.Id breaks when the ids restart per entity.
Fix: the InferRxNorm handler in the comprehendmedical service crate - the RxNorm candidate lookup/ranking and the response-wide Id allocator shared with the attribute builder.
Traces: comprehendmedical-infer-rxnorm-medications#task#1.

Cause 4 of 4 - InferSNOMEDCT returns one SNOMED-CT candidate instead of the ranked list and a wrong SNOMEDCTDetails.VersionDate. Fixture comprehendmedical-infer-snomedct-concepts, step task#1.
AWS: Smithy: InferSNOMEDCT output carries Entities (SNOMEDCTEntityList), ModelVersion, and SNOMEDCTDetails {Edition, Language, VersionDate}, documented as "the information about the revision of the SNOMED-CT ontology in the response" with VersionDate being "the version date of the SNOMED-CT ontology used". SNOMEDCTConcept is {Code, Description, Score}. Recorded behavior: every entity and every attribute carries five SNOMED-CT candidates ranked by descending Score (e.g. 44054006 'Diabetes mellitus type 2 (disorder)' ahead of 73211009 'Diabetes mellitus (disorder)' and three more), and SNOMEDCTDetails.VersionDate is the ontology revision date the service actually linked against.
Emulator: each entity's and attribute's SNOMEDCTConcepts list is truncated to a single element (SNOMEDCTConcepts[1] through [4] are absent on the entity, and the attribute carries exactly one concept), concept Scores are the canned ladder values (0.972000002861023, 0.6998400092124939) rather than a real descending ranking, entity/attribute ordering is shifted so identical spans are reported at different list indices, and SNOMEDCTDetails.VersionDate does not match the recorded ontology revision.
Why: SNOMED-CT linking is only useful as a ranked candidate list a clinician or downstream rule set disambiguates; with one candidate the emulator forces a choice the real service leaves open, and a client that keys cached mappings on SNOMEDCTDetails.VersionDate caches them under an ontology revision that was never used.
Fix: the InferSNOMEDCT handler in the comprehendmedical service crate - the SNOMED-CT candidate lookup/ranking (it must return the full ranked candidate list for entities and attributes) and the static SNOMEDCTDetails block.
Traces: comprehendmedical-infer-snomedct-concepts#task#1.

Shared root cause: all four handlers answer from a small hard-coded entity/concept table rather than from the recorded inference contract, so the defect is one of inference-response fidelity across the text-inference family. Note the exact float confidence values are model-generated, so the requirement is the structure and ranking (which entities, which Type/Category, how many candidates, descending Score, correct Code/Description for the entity's own text) plus ModelVersion and SNOMEDCTDetails - not the literal floats from any one recording.

Related open issues that do NOT cover these causes: 20261022T000000Z-stub-text-inference-service-com-amazonaws-comprehendmedical-comprehendmedical-20181030.md (filed when these ops were stubs; they now answer 200, so the remaining gap is fidelity, not absence), and 20261029T172356Z-comprehendmedical-asynchronous-job-error-contract-diverges-from-aws.md / 20261030T011657Z-comprehendmedical-startphidetectionjob-skips-dataaccessrolearn-trust-resolution-and-mints.md, which own the asynchronous job-family divergences only.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator: 28 steps, 0 passed, 28 mismatched, score 0.0%; 11 divergence clusters, all service=comprehendmedical. The four clusters below carry empty stub_path and empty owned_by (net-new).

Cause 1 - comprehendmedical-detect-phi-note#task#1, DetectPHI, expected 200 / actual 200:
  $.ModelVersion: value_mismatch (expected '1.1.0', got '0.1.0')
  $.Entities[0].Score: expected 0.999757707118988, got 0.9359999895095825
  $.Entities[1].Score: expected 0.999953031539917, got 0.975600004196167
  $.Entities[2].Score: expected 0.999990701675415, got 0.975600004196167
  $.Entities[3].Score: expected 0.981614887714386, got 0.9575999975204468
  $.Entities[4].Score: expected 0.9972621202468872, got 0.975600004196167
  $.Entities[5].Score: expected 0.9676140546798706, got 0.9848160147666931
(note the emulator repeats 0.975600004196167 for three different entities - a fixed ladder, not per-entity confidence)

Cause 2 - comprehendmedical-infer-icd10cm-conditions#task#1, InferICD10CM, expected 200 / actual 200:
  $.Entities[0]: missing - expected the MEDICAL_CONDITION/DX_NAME entity Text 'diabetes mellitus' (BeginOffset 55, EndOffset 72, Id 3, Traits [{DIAGNOSIS}], one QUALITY attribute 'type 2', five ICD10CMConcepts E11.9/E11.22/E11.65/E11/E11.0); got {'Id': 1, 'Text': '65-year', 'Category': 'MEDICAL_CONDITION', 'Type': 'TIME_EXPRESSION', 'Score': 0.9660000205039978, 'BeginOffset': 13, 'EndOffset': 20, 'Attributes': [], 'Traits': []}
  $.Entities[1].ICD10CMConcepts[1].Code: expected 'I11.0', got 'E11.8'
  $.Entities[1].ICD10CMConcepts[1].Description: expected 'Hypertensive heart disease with heart failure', got 'Type 2 diabetes mellitus with unspecified complications'
  $.Entities[1].ICD10CMConcepts[2].Code: expected 'I12.9', got 'E11.65'
  $.Entities[1].ICD10CMConcepts[0].Score: expected 0.6778631210327148, got 0.972000002861023
  $.Entities[1].ICD10CMConcepts[1].Score: expected 0.35152581334114075, got 0.6998400092124939
  also diverging: $.Entities[1].ICD10CMConcepts[3], [4] (absent), $.Entities[1].Score, $.Entities[1].Traits[0].Score, $.Entities[2], $.Entities[3], $.ModelVersion
Smithy: ICD10CMEntityCategory = [MEDICAL_CONDITION]; ICD10CMEntityType = [DX_NAME, TIME_EXPRESSION].

Cause 3 - comprehendmedical-infer-rxnorm-medications#task#1, InferRxNorm, expected 200 / actual 200:
  $.Entities[0]: missing - expected the MEDICATION/GENERIC_NAME entity Text 'metformin' with Id 13, attributes Id 14/15/16 (DOSAGE '500 mg', ROUTE_OR_MODE 'orally', FREQUENCY 'twice daily') and five RxNormConcepts 861007 (0.343), 860975 (0.027), 6809 (0.018), 1807917 (0.0073), 861819 (0.0060); got {'Id': 1, 'Text': 'metformin', ..., 'Score': 0.8399999737739563, 'Attributes': [{'Type': 'DOSAGE', 'Score': 0.966, 'RelationshipScore': 0.95, 'Id': 2, ...}, {'Type': 'ROUTE_OR_MODE', 'Score': 0.9, 'RelationshipScore': 0.79, 'Id': 3, ...}, {'Type': 'FREQUENCY', 'Score': 0.936, 'RelationshipScore': 0.71, 'Id': 4, ...}]}
  also diverging: $.Entities[1], $.ModelVersion

Cause 4 - comprehendmedical-infer-snomedct-concepts#task#1, InferSNOMEDCT, expected 200 / actual 200:
  $.Entities[0].Attributes[0]: expected the QUALITY attribute 'type 2' with five SNOMEDCTConcepts (258195006 0.667, 44054006 0.0426, 258199000 0.0184, 410657003 0.0168, 405751000 0.0147); got the same attribute with exactly ONE concept [{'Code': '258195006', 'Score': 0.9359999895095825}]
  $.Entities[0].SNOMEDCTConcepts[0].Score: expected 0.8340365886688232, got 0.972000002861023
  $.Entities[0].SNOMEDCTConcepts[1].Score: expected 0.32647690176963806, got 0.6998400092124939
  $.Entities[0].SNOMEDCTConcepts[2], [3], [4]: absent
  $.SNOMEDCTDetails.VersionDate: diverges
  also diverging: $.Entities[0..5] (list ordering shifted - several keys report the same value at a different index), $.ModelVersion
Smithy: InferSNOMEDCTResponse carries SNOMEDCTDetails {Edition, Language, VersionDate}; SNOMEDCTConcept = {Code, Description, Score}.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture comprehendmedical-detect-phi-note, step task#1 - POST DetectPHI with the fixture's clinical note Text. Observe 200 with ModelVersion "0.1.0" (recording: the real PHI model version, "1.1.0" in the recording) and six entity Scores drawn from a repeating fixed ladder.
3. Cause 2: replay fixture comprehendmedical-infer-icd10cm-conditions, step task#1 - POST InferICD10CM with the fixture's Text. Observe 200 whose first entity is a TIME_EXPRESSION span ('65-year') instead of the recorded DX_NAME condition entity, whose ICD10CMConcepts lists hold three candidates instead of five, and whose hypertension entity is linked to E11.x diabetes codes instead of I11.x/I12.x.
4. Cause 3: replay fixture comprehendmedical-infer-rxnorm-medications, step task#1 - POST InferRxNorm with the fixture's medication text. Observe 200 whose entity/attribute Ids restart at 1 instead of continuing the response-wide monotonic sequence and whose RxNormConcepts are not the recorded ranked RxCUI candidates.
5. Cause 4: replay fixture comprehendmedical-infer-snomedct-concepts, step task#1 - POST InferSNOMEDCT with the fixture's Text. Observe 200 whose entity and attribute SNOMEDCTConcepts lists each hold exactly one candidate instead of the recorded five ranked candidates, and whose SNOMEDCTDetails.VersionDate does not match the recorded ontology revision.
6. All four steps return the correct HTTP status and envelope, so the divergence is only visible by diffing the response body against the fixture.
