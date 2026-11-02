Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-refarch-rekreup-detect-labels-s3object-mistakes
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-refarch-rekreup-detect-labels-s3object-mistakes
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|DetectLabels|4xx->4xx|$.message
Issue-Commit: 276fdc9b01f0fecfb4ec23b6285a10289d7fb0d2

# DetectLabels constraint-violation message renders MinConfidence as an integer, dropping the float formatting AWS uses

> _Note: this issue was identified against the code base at commit `276fdc9b01f0fecfb4ec23b6285a10289d7fb0d2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectLabels

## Description
Cause 1 of 1: the `Value '<x>'` interpolation in Rekognition's range-constraint validation message for DetectLabels.MinConfidence is built from an integer-ish rendering of the submitted number instead of the float rendering AWS uses, so the reported value loses its decimal part.

AWS: Smithy: DetectLabels.MinConfidence targets com.amazonaws.rekognition#Percent, type `float`, @range min 0 max 100. When MinConfidence is outside that range AWS answers HTTP 400 with error code ValidationException and the standard constraint message:
  `1 validation error detected: Value '<value>' at 'minConfidence' failed to satisfy constraint: Member must have value less than or equal to 100`
The fixed text is `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must have value less than or equal to `; the variable parts are the submitted value, the lowerCamel member path (`minConfidence`), and the modeled bound (`100`, from @range max). Because the member is modeled as a float, AWS renders the submitted value with float formatting, i.e. a whole number carries a `.0` suffix (`101.0`, not `101`). The recorded response shows exactly that.

Emulator: the status (400) and the error code match, but the message renders the value with integer formatting. The replayed step expected `... Value '101.0' at 'minConfidence' ...` and the emulator returned `... Value '101' at 'minConfidence' ...` — `$.message: value_mismatch`. The divergence is purely in the rendering of the float member's value, not in the request's own value.

Why: clients and test suites that assert on the constraint message — and anything that parses the offending value back out of the message to report it to a user — see a different string than AWS produces for the same request, so exact-match assertions on AWS's documented validation wording fail and value-extraction logic infers an integer member where the API models a float. It also makes the emulator's validation messages inconsistent for float members generally (any out-of-range Percent-typed input is affected the same way, e.g. MinConfidence, similarity thresholds), not just this one recorded number.

Fix: DetectLabels' input validation / the shared constraint-message formatter that renders `Value '<x>'` for @range violations — it must format the value using the member's modeled numeric type (float members rendered with their fractional part, so a whole float becomes `101.0`) rather than normalizing the parsed number to its shortest integer form. No other part of the response (status, code, member path, bound) needs to change.

Traces: rekognition-refarch-rekreup-detect-labels-s3object-mistakes#task#4 (the only member of the cluster).

Not covered by any open issue: the open Rekognition items concern collection/face fidelity, TagResource/ListTagsForResource persistence, PutProjectPolicy, and unimplemented-operation stubs; none of them touch DetectLabels input-validation message formatting.

## Evidence
Replay cluster divergence_key `service=rekognition|DetectLabels|status=400->400|keys=$.message`, verdict `mismatch`, expected_status 400 / actual_status 400.

Expected (recorded AWS response body, $.message):
  1 validation error detected: Value '101.0' at 'minConfidence' failed to satisfy constraint: Member must have value less than or equal to 100

Actual (emulator, $.message):
  1 validation error detected: Value '101' at 'minConfidence' failed to satisfy constraint: Member must have value less than or equal to 100

Only the interpolated value differs: `101.0` (float rendering, AWS) vs `101` (integer rendering, emulator). Model basis: Smithy DetectLabelsRequest.MinConfidence -> com.amazonaws.rekognition#Percent, type float, @range {min: 0, max: 100}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `rekognition-refarch-rekreup-detect-labels-s3object-mistakes`, step `task#4`.
2. Equivalently, call rekognition DetectLabels with a valid Image (S3Object) and a MinConfidence above the modeled maximum, e.g. MinConfidence = 101.
3. Observe HTTP 400 and read the `message` field of the error body.
4. The emulator reports `Value '101' at 'minConfidence'`; AWS reports `Value '101.0' at 'minConfidence'` because MinConfidence is a float member. Any whole-number out-of-range float (0-adjacent or >100) reproduces it.
