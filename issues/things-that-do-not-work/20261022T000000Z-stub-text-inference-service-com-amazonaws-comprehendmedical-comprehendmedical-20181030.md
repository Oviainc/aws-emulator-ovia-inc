Issue-Severity: HIGH
Issue-Gap-Class: stub

# ComprehendMedical's text-inference operations are still stubs

_Filed: 2026-10-22T00:00:00Z_
_Operation: service-com-amazonaws-comprehendmedical-comprehendmedical-20181030 | Class: stub_

> _Note: this issue records the scope left open when the ComprehendMedical bootstrap issue (`20261021T213813Z-bootstrap-service-com-amazonaws-comprehendmedical-comprehendmedical-20181030.md`) was resolved with the asynchronous entities-detection job slice. The code base can have drifted, so trace it back before working on it._

## Description

The asynchronous entities-detection family (`StartEntitiesDetectionV2Job`,
`DescribeEntitiesDetectionV2Job`, `ListEntitiesDetectionV2Jobs`,
`StopEntitiesDetectionV2Job`) is implemented over persisted state. The SYNCHRONOUS
text-inference operations are NOT, and each still answers HTTP 501 `NotImplemented`
with its `//! SCAFFOLDED, NOT IMPLEMENTED:` census line in place:

- `DetectEntitiesV2`, `DetectEntities` (deprecated), `DetectPHI`
- `InferICD10CM`, `InferRxNorm`, `InferSNOMEDCT`

Each one is machine-learning inference over clinical text: it reports the entities a
trained model finds, with "the level of confidence in the detection and analysis"
(`Score`), and the ICD-10-CM / RxNorm / SNOMED CT operations additionally map each
finding onto a licensed terminology's concept codes. This emulator has no such model
and no terminology, so there is nothing to compute the answer from.

A previous attempt at `DetectEntitiesV2` used a hand-written ~60-term table plus a
digest-derived `Score` and was rejected, correctly: a term list answers only the inputs
whoever wrote it enumerated, and a hash rescaled into `[0.90, 1.00)` is a number shaped
like a model probability that no model produced. **Do not re-introduce either.** The 501
is the honest answer until a real, maintained inference component is wired into the
workspace manifest and actually runs the inference in-process.

Resolving this issue therefore means ONE of:

1. Wiring in a maintained crates.io inference component (and, for the `Infer*`
   operations, a real terminology) that computes the answer for ANY clinical text, and
   implementing the operations on top of it; or
2. Confirming that no such component can run inside this emulator, and closing this
   issue as not-a-real-issue with the 501 left in place — the scaffold's documented
   answer for work the emulator cannot perform.

## Evidence

- `services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/detect_entities_v2.rs`
  (and the five siblings listed above) carry the `//! SCAFFOLDED, NOT IMPLEMENTED:`
  marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))`.
- `services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/tests/all/entities_detection_v2.rs::the_text_inference_operations_answer_not_implemented`
  pins that 501 (status 501, `__type: NotImplemented`, no `Entities` list), so an
  implementation that starts answering 200 without real inference behind it fails that
  test rather than passing silently.
- A bad request still gets a 400, not the 501: the same test module's refusal table
  covers empty text (`InvalidRequestException`) and text past the 20000-character
  limit (`TextSizeLimitExceededException`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `comprehendmedical` client at it.
2. `aws comprehendmedical detect-entities-v2 --text 'Patient reports a headache.'`
   -> HTTP 501 `NotImplemented` ("operation DetectEntitiesV2 is scaffolded but not
   implemented in this emulator").
3. The same for `detect-entities`, `detect-phi`, `infer-icd10cm`, `infer-rx-norm` and
   `infer-snomedct`.
4. The asynchronous job operations, by contrast, work:
   `aws comprehendmedical start-entities-detection-v2-job …` then
   `describe-entities-detection-v2-job --job-id <JobId>` reports the stored job.

## Not in scope

The remaining asynchronous job families (ICD10CM / RxNorm / SNOMEDCT / PHI start,
describe, list and stop) are a separate gap: they can reuse `crate::state::AsyncJob`
and the entities-detection handlers as written, and need no inference component.
