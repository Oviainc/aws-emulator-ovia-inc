Issue-Source: trace-sweep-divergence
Issue-Trace: groundstation-config-other-capability-types
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: groundstation-config-other-capability-types groundstation-get-minute-usage
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation|CreateConfig|4xx->2xx|$.message
Issue-Key: groundstation|CreateConfig|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: groundstation|CreateConfig|4xx->2xx|status
Issue-Key: groundstation|GetMinuteUsage|4xx->4xx|$.message
Issue-Key: groundstation|GetMinuteUsage|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# Ground Station request validation: unapproved uplink frequency accepted and wrong validation error identity

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfig

## Description
Ground Station's input-validation layer both under-rejects (an antenna-uplink config AWS refuses is created with HTTP 200) and mis-identifies the rejection it does make (a range violation is answered with the wrong error type). Two causes.

Cause 1 of 2: CreateConfig accepts an antenna-uplink config whose uplink frequency is not approved.
AWS: Smithy CreateConfig -> POST /config, errors DependencyException(531), InvalidParameterException(431, 'One or more parameters are not valid', members message + parameterName), ResourceLimitExceededException(429), ResourceNotFoundException(434). configData is a ConfigTypeData union whose antennaUplinkConfig carries the spectrum/target EIRP; the recorded response for an antenna-uplink config with a non-approved uplink frequency is HTTP 431 with x-amzn-errortype: InvalidParameterException and message 'The specified Uplink Frequency has not been approved.' The message rule here is the service's fixed refusal text for this condition (recorded behavior; the model documents the error but not the text), so treat the 431 status and the InvalidParameterException error code as the requirement and the text as the example.
Emulator: HTTP 200 — the config is created and an ARN is handed back. No `message`, no x-amzn-errortype header.
Why: a client gets an ARN for a config AWS would never have created, so the failure surfaces much later (at ReserveContact / mission-profile wiring time) instead of at create time. Tests that assert the refusal pass vacuously, and uplink-frequency approval is silently not enforced.
Fix: CreateConfig's validation for the antennaUplinkConfig branch of ConfigTypeData (reject non-approved uplink frequencies with InvalidParameterException/431 before the state write).

Cause 2 of 2: GetMinuteUsage answers an out-of-range `month` with the wrong error type and the wrong message rule.
AWS: Smithy GetMinuteUsage -> POST /minute-usage with required `month` (Month, @range min 1 max 12) and required `year` (Year, @range 2018..3000). For month=13 the recording shows HTTP 400 with x-amzn-errortype: BadRequestException and message 'Invalid request body' — i.e. Ground Station answers a malformed/out-of-range body with a BadRequestException envelope, not a constraint-detail ValidationException.
Emulator: HTTP 400 (status matches) but x-amzn-errortype: ValidationException and the generic AWS constraint text "1 validation error detected: Value '13' at 'month' failed to satisfy constraint: Member must have value less than or equal to 12".
Why: the error CODE is what SDKs and clients branch on. A client catching BadRequestException does not catch the emulator's ValidationException, so the rejection escapes its handler as an unmodeled error even though the status is right.
Fix: GetMinuteUsage's validation/error mapping — the range violation on `month` must be emitted as the service's BadRequestException envelope rather than the shared ValidationException formatter.

Traces (one representative per cause):
- Cause 1: groundstation-config-other-capability-types#task#1 (CreateConfig, expected 431, got 200)
- Cause 2: groundstation-get-minute-usage#task#2 (GetMinuteUsage, 400 with wrong errortype)

## Evidence
Cause 1 — groundstation-config-other-capability-types#task#1, CreateConfig:
  expected status 431, actual 200
  $.message: missing (expected 'The specified Uplink Frequency has not been approved.', got None)
  $headers.x-amzn-errortype: missing (expected 'InvalidParameterException', got None)
  cluster also covers groundstation-config-other-capability-types#task#6

Cause 2 — groundstation-get-minute-usage#task#2, GetMinuteUsage (400 vs 400):
  $headers.x-amzn-errortype: expected 'BadRequestException', got 'ValidationException'
  $.message: expected 'Invalid request body', got "1 validation error detected: Value '13' at 'month' failed to satisfy constraint: Member must have value less than or equal to 12"

Smithy citations: CreateConfig errors include InvalidParameterException @httpError 431 (members message, parameterName; doc 'One or more parameters are not valid'); GetMinuteUsage input month -> Month @range{min:1,max:12}, year -> Year @range{min:2018,max:3000}; GetMinuteUsage's modeled errors are DependencyException(531)/InvalidParameterException(431)/ResourceNotFoundException(434) — ValidationException is not among them.

## Steps to Reproduce
1. Replay the wire corpus: replay_wire_corpus().
2. Cause 1: fixture groundstation-config-other-capability-types, step task#1 — POST /config with configData.antennaUplinkConfig using an uplink frequency that is not approved for the account. AWS answers 431 with x-amzn-errortype: InvalidParameterException; the emulator answers 200 and returns a configArn. Same divergence at step task#6.
3. Cause 2: fixture groundstation-get-minute-usage, step task#2 — POST /minute-usage with {"month": 13, "year": <valid year>}. Both return 400, but check the x-amzn-errortype header: AWS sends BadRequestException, the emulator sends ValidationException.
