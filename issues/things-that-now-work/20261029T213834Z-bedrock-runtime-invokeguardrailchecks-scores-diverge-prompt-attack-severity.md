Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-runtime-guardrail-checks-inline
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-runtime-guardrail-checks-inline bedrock-runtime-invoke-guardrail-checks-prompt-attack bedrock-runtime-invoke-guardrail-checks-sensitive-information-multi-message bedrockruntime-guardrail-checks-decisions
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|2xx->2xx|$.results.promptAttack.results[*].severityScore
Issue-Key: bedrock-runtime|InvokeGuardrailChecks|2xx->2xx|$.results.sensitiveInformation.results[*].confidenceScore
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# bedrock-runtime InvokeGuardrailChecks scores diverge: prompt-attack severity and category order, and PII confidenceScore is a quantized f32

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeGuardrailChecks

## Description
InvokeGuardrailChecks answers 200 with a structurally valid body, but every score member in it is wrong: the prompt-attack per-category severityScore list is emitted with scores that do not match the evaluated content (and in a different category order from the service's), and every sensitiveInformation confidenceScore is served as one constant, f32-rounded value instead of the per-detection confidence. Four net-new causes, all in the same scoring/serialization path.

Cause 1 of 4 - sensitiveInformation.confidenceScore is a single quantized constant (fixture bedrock-runtime-guardrail-checks-inline, step task#1; the same step also shows the prompt-attack defect of Cause 3).
AWS: Smithy GuardrailChecksSensitiveInformationResultEntry.confidenceScore -> Double, @required, @range min 0.0 max 1.0, documented as "The confidence score for the detection, ranging from 0.0 to 1.0. Higher values indicate greater confidence." It is per-detection: a fully-determined entity (e.g. a syntactically complete, checksummed identifier) scores at the top of the range while a weaker match scores lower, so distinct entries in one response carry distinct scores. Recorded behavior shows exactly that: this step's single entry carries confidenceScore 1.0.
Emulator: serves 0.8999999761581421 for that entry - the decimal expansion of the f32 nearest 0.9, i.e. one hard-coded 0.9 confidence held as a 32-bit float and widened to the modeled Double on serialization.
Why: a client that thresholds on confidence ("redact only >= 0.95", "review 0.9") gets a different decision than against AWS for every entity, and because the value is never exactly 0.9 an equality or rounding comparison on it fails too; an SDK that round-trips the Double re-emits the 17-digit artifact.
Fix: the InvokeGuardrailChecks handler's sensitive-information (PII) scoring, plus its serializer, which must carry confidenceScore as a 64-bit Double end to end rather than widening an f32 literal.

Cause 2 of 4 - confidenceScore is constant across entries of differing strength (fixture bedrock-runtime-invoke-guardrail-checks-sensitive-information-multi-message, step task#1; the cluster also covers bedrockruntime-guardrail-checks-decisions#task#1).
AWS: same member as Cause 1. Recorded behavior shows two detections in one response with two different confidences - the first 1.0, the second 0.8 - i.e. the score varies per detected entity and per the evidence for it.
Emulator: both entries carry the identical 0.8999999761581421, so the per-entity signal is erased; whatever entity count a request produces, every confidenceScore is the same number.
Why: a caller cannot rank or filter detections by confidence at all - a weak match is indistinguishable from a certain one - so confidence-gated redaction and human-review routing behave differently than on AWS.
Fix: same component as Cause 1 - per-entity confidence in the PII recognizer, not one default applied to the whole list.

Cause 3 of 4 - promptAttack severityScore is quantized and the per-category entries are not in the service's order (fixture bedrock-runtime-invoke-guardrail-checks-prompt-attack, step task#1).
AWS: Smithy GuardrailChecksPromptAttackResultEntry = { category (GuardrailChecksPromptAttackCategory, @required), severityScore (Double, @required, @range 0.0-1.0, "Higher values indicate greater severity") }, carried in GuardrailChecksPromptAttackResult.results. The score is per category: a category the content clearly exercises scores at the top of the range and a category it does not exercise scores 0.0. Recorded behavior for this step: results[0].severityScore 1.0 and results[1].severityScore 0.0 - a decisive hit on the first category and a clean miss on the second, in the order the service returns the categories.
Emulator: serves results[0].severityScore 0.0 and results[1].severityScore 0.9 - the decisive and the clean-miss category are transposed, and the hit is reported as a hard-coded 0.9 rather than the decisive score.
Why: a client that reads results[0] (or indexes the list positionally rather than matching on `category`) attributes the attack to the wrong category, and one that thresholds severity at > 0.9 sees no detection where AWS reports a certain one - a guardrail bypass.
Fix: the InvokeGuardrailChecks handler's prompt-attack classifier - per-category scoring, and the order in which the result entries are appended to GuardrailChecksPromptAttackResult.results.

Cause 4 of 4 - same prompt-attack defect with three categories (fixture bedrockruntime-guardrail-checks-decisions, step task#2).
AWS: as Cause 3. Recorded behavior: three entries scoring 1.0, 0.0, 1.0 - two decisive categories and one miss.
Emulator: 0.0, 0.9, 0.9 - the miss and the hits are mis-placed and every nonzero score is the same 0.9 constant, so the response reports one pattern of categories where AWS reports another.
Why: as Cause 3; with more than two categories the positional mismatch means a caller's per-category policy map is applied to the wrong categories.
Fix: as Cause 3.

Related open issues, neither of which covers these causes: 20261024T031932Z (inline-guardrail error taxonomy - error codes, not scores) and the resolved 20261026T233519Z (content-filter scores always 0.0 and a missed PHONE entity - the score paths it fixed are the content-filter and recognizer coverage, not promptAttack severity ordering or the f32 confidence).

## Evidence
Replay, all verdict=mismatch at status 200->200:
1) bedrock-runtime-guardrail-checks-inline#task#1 - $.results.sensitiveInformation.results[0].confidenceScore: expected 1.0, got 0.8999999761581421; $.results.promptAttack.results[0].severityScore: expected 0.8, got 0.9.
2) bedrock-runtime-invoke-guardrail-checks-sensitive-information-multi-message#task#1 (also affects bedrockruntime-guardrail-checks-decisions#task#1) - confidenceScore[0]: expected 1.0, got 0.8999999761581421; confidenceScore[1]: expected 0.8, got 0.8999999761581421.
3) bedrock-runtime-invoke-guardrail-checks-prompt-attack#task#1 - promptAttack.results[0].severityScore: expected 1.0, got 0.0; results[1].severityScore: expected 0.0, got 0.9.
4) bedrockruntime-guardrail-checks-decisions#task#2 - promptAttack.results[0..2].severityScore: expected 1.0, 0.0, 1.0; got 0.0, 0.9, 0.9.

Smithy (com.amazonaws.bedrockruntime): GuardrailChecksSensitiveInformationResultEntry.confidenceScore = Double @required @range(0.0,1.0); GuardrailChecksPromptAttackResultEntry.severityScore = Double @required @range(0.0,1.0); both results lists are @required members of GuardrailChecksResults. 0.8999999761581421 is the exact f64 expansion of the f32 value 0.9, which is the serialization half of the defect.

## Steps to Reproduce
1. git clone the repo and boot the emulator; 2. replay fixture bedrock-runtime-guardrail-checks-inline and read step task#1's 200 body - $.results.sensitiveInformation.results[0].confidenceScore is 0.8999999761581421, not the recorded 1.0; 3. replay bedrock-runtime-invoke-guardrail-checks-sensitive-information-multi-message step task#1 - both confidenceScores are the same 0.8999999761581421; 4. replay bedrock-runtime-invoke-guardrail-checks-prompt-attack step task#1 and bedrockruntime-guardrail-checks-decisions step task#2 - the promptAttack severityScore sequence is 0.0/0.9(/0.9) where the recording has 1.0/0.0(/1.0).
