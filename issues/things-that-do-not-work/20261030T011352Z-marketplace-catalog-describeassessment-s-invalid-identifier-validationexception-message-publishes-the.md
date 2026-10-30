Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-catalog.describe-assessment-identifier-validation-boundary
Issue-Shape-Id: com.amazonaws.marketplacecatalog#AWSMPSeymour
Issue-Key: marketplace-catalog.describe-assessment-identifier-validation-boundary|run#4|AssertionError|assert msg == (
Issue-Commit: af94c44df105452dffe8d3f21560cef9d55ed36d

# DescribeAssessment's invalid-identifier ValidationException message publishes the Smithy @pattern literal instead of the regex AWS prints

> _Note: this issue was identified against the code base at commit `af94c44df105452dffe8d3f21560cef9d55ed36d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAssessment

## Description
1 cause, on marketplace-catalog, which must be closed before submitting.

Cause 1 of 1: the fixed regex text inside the DescribeAssessment invalid-identifier ValidationException message is rendered from the emulator's own model data instead of the literal string the service publishes.

  AWS: Smithy (services/service-com-amazonaws-marketplacecatalog-awsmpseymour/smithy-model/com-amazonaws-marketplacecatalog-awsmpseymour.json): DescribeAssessment.AssessmentIdentifier is @required, @length(min 1, max 2048), @pattern "^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$"; DescribeAssessment binds ValidationException with @httpError(422) and ResourceNotFoundException with @httpError(404). The model does NOT state the refusal text, so the message itself is recorded behavior only: for an AssessmentIdentifier matching neither accepted form, AWS answers error code ValidationException (HTTP 422) with Message
    "[Requested assessment identifier '<identifier>' is invalid. It should match with (^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$).]"
    Fixed text of the template: the surrounding square brackets (the service reports a list of validation findings), the sentence "Requested assessment identifier '...' is invalid. It should match with ...." AND the regex literal itself — AWS spells it as TWO separately anchored alternatives joined by '|' ((^assessment-...$)|(^arn:...$)) with a bare '::' in the ARN branch's account-id position. The ONLY variable part is the quoted identifier, echoed verbatim from the request's AssessmentIdentifier.

  Emulator: the status (422) and the error code (ValidationException) are correct, but the regex slot is interpolated straight from the member's Smithy @pattern, producing
    "[Requested assessment identifier '<identifier>' is invalid. It should match with ^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$.]"
    i.e. ONE anchored group containing both alternatives instead of two anchored alternatives, and ':[0-9]*:' where AWS's published text has '::'.

  Why: the identifier-validation text is a published contract of this refusal. A client (or a conformance test) that asserts on the ValidationException message sees a different sentence tail; worse, a client that parses the advertised regex to pre-validate identifiers locally gets a pattern that is not the one the service publishes, and the emulator's spelling additionally advertises an account-id segment ([0-9]*) in the ARN branch that AWS's message does not advertise at all. Anything keying on the published syntax therefore pre-validates against the wrong grammar.

  Fix: the DescribeAssessment identifier-validation path in the marketplace-catalog crate — the refusal's message template must carry AWS's regex string as FIXED text rather than interpolating the model's @pattern (or the crate's pattern constant) into the message, leaving only the request's identifier as the variable substitution. The surrounding handler behavior is already right and must stay: a well-formed-but-unknown 'assessment-<alnum>' identifier continues to reach the store lookup and be refused 404 ResourceNotFoundException with "Requested assessment '<identifier>' does not exist."

  Traces: behavioral check marketplace-catalog.describe-assessment-identifier-validation-boundary (group error-semantics).

Related open issues, neither of which covers this: 20261029T201559Z-marketplace-catalog-request-validation-refusals-diverge-constraint-violations-answer-400.md describes the same message divergence from the WIRE corpus and carries only wire fixture/step keys (marketplace-catalog-describe-assessment-identifier-validation-boundary#task#1), not this behavioral check's replay key; 20261029T104213Z-marketplace-catalog-describeassessment-validation.md is about the unenforced @length(1..2048) on the same member (an over-long identifier answering 404), a different cause. Closing either does not clear this check.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 24/25 passed, 1 mismatched. The single failure is marketplace-catalog.describe-assessment-identifier-validation-boundary; it was not marked ALREADY FILED and not marked OWNED by any open issue.

Check step: mp.describe_assessment(Catalog="AWSMarketplace", AssessmentIdentifier="not-a-valid-identifier") in us-east-1.

The error CODE assertion passed (code == "ValidationException"), so status/code are correct. The MESSAGE assertion failed:

  expected (AWS): "[Requested assessment identifier 'not-a-valid-identifier' is invalid. It should match with (^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$).]"
  actual (emulator): "[Requested assessment identifier 'not-a-valid-identifier' is invalid. It should match with ^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$.]"

raw assertion output: AssertionError: [Requested assessment identifier 'not-a-valid-identifier' is invalid. It should match with ^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$.] (the assert's message argument is the ACTUAL emulator message), at check_marketplace_catalog_describe_assessment_identifier_validation_boundary.py line 33.

Message-part breakdown: the quoted 'not-a-valid-identifier' is the request's AssessmentIdentifier (variable); every other character of the sentence, including the whole regex literal and the enclosing square brackets, is fixed text and is where the two strings diverge. The divergence is purely in the fixed part — the emulator emits one anchored group rather than two anchored alternatives and ':[0-9]*:' rather than '::'.

Model evidence (smithy_query query="operation", operation="DescribeAssessment", model_path=services/service-com-amazonaws-marketplacecatalog-awsmpseymour/smithy-model/com-amazonaws-marketplacecatalog-awsmpseymour.json): AssessmentIdentifier -> {required: true, length 1..2048, pattern "^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$"}; errors include ValidationException (client, httpError 422) and ResourceNotFoundException (client, httpError 404); http POST /DescribeAssessment. The emulator's emitted regex is byte-for-byte this @pattern value, which is the direct evidence that the message interpolates the model constraint instead of the service's published literal.

## Steps to Reproduce
1. Boot the emulator from the checkout and create a marketplace-catalog client against region us-east-1 (the Marketplace Catalog API is served only there).
2. Call DescribeAssessment with Catalog="AWSMarketplace" and AssessmentIdentifier="not-a-valid-identifier" (any value matching neither the 'assessment-<alnum>' short form nor the full Assessment ARN form) — POST /DescribeAssessment.
3. Observe HTTP 422 with Error.Code "ValidationException" (correct) and Error.Message "[Requested assessment identifier 'not-a-valid-identifier' is invalid. It should match with ^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$.]".
4. Expected Message: "[Requested assessment identifier 'not-a-valid-identifier' is invalid. It should match with (^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$).]" — two separately anchored alternatives joined by '|', with '::' in the ARN branch, and no [0-9]* account-id segment.
5. Regression guard for the neighbouring behavior the check also asserts: call DescribeAssessment with Catalog="AWSMarketplace" and a well-formed but unknown identifier ("assessment-" followed by alphanumerics); it must still answer HTTP 404 ResourceNotFoundException with Message "Requested assessment '<identifier>' does not exist." (no square brackets on this one).
6. Re-replay behavioral check marketplace-catalog.describe-assessment-identifier-validation-boundary to confirm.
