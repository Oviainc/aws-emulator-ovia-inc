Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-eescfn-findinmap-and-mappings-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-eescfn-findinmap-and-mappings-mistakes cloudformation-eescfn-sub-mapping-attribute-reference cloudformation-eescfn-validate-expansion-by-s3-url template-body-size-limit template-count-limits cloudformation-eescfn-stage-parameter-declaration-mistakes
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ValidateTemplate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ValidateTemplate|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|ValidateTemplate|4xx->2xx|status
Issue-Commit: 1a915a323f43835e1873a1493fb7da2b1abdc334

# CloudFormation ValidateTemplate template-error checks: wrong Fn::FindInMap wording and missing parameter-type rule

> _Note: this issue was identified against the code base at commit `1a915a323f43835e1873a1493fb7da2b1abdc334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidateTemplate

## Description
ValidateTemplate's template-format checks diverge from AWS in two ways: one check refuses with text AWS does not use, and one check is missing entirely so an invalid template validates. This bundle carries 2 distinct causes. Open issue 20261103T171935Z-cloudformation-template-validation-and-gettemplatesummary-diverge-on-limits.md covers other ValidateTemplate/GetTemplateSummary checks (template-source refusals, size and count limits, intrinsic evaluation during ValidateTemplate) and owns NEITHER of these replay keys (owned_by is empty for both clusters), so both must be closed here.

Cause 1 of 2 — Fn::FindInMap with the wrong number of parameters.
AWS: 400 with Error.Type 'Sender', Error.Code 'ValidationError' and Message 'Template error: every Fn::FindInMap object requires three parameters, the map name, map key and the attribute for return value'. The whole message is fixed text for this error class — it has no variable parts taken from the request. Smithy: ValidateTemplate models no operation errors, so this is the service-common ValidationError (recorded behavior supplies the wording).
Emulator: the same 400 ValidationError, but with Message 'Template error: every Fn::FindInMap object requires three parameters, the map name, the top-level key, and the second-level key' — a rewritten tail naming top-level/second-level keys instead of 'map key and the attribute for return value'.
Why: template linters and CI gates match CloudFormation's template-error text to classify authoring mistakes and to render guidance; rewritten fixed text is not recognised, so the failure is reported as an unknown validation error. The cluster shows the same wrong text served by ValidateTemplate across nine recorded steps.
Fix: the Fn::FindInMap arity check's message construction in the template validator used by ValidateTemplate (and the shared template-error message table, since the same text is served to CreateStack/UpdateStack paths).
Traces: cloudformation-eescfn-findinmap-and-mappings-mistakes#task#1 (cluster also covers cloudformation-eescfn-findinmap-and-mappings-mistakes#task#2 and #task#3, cloudformation-eescfn-sub-mapping-attribute-reference#task#1 and #task#2, cloudformation-eescfn-validate-expansion-by-s3-url#task#6, template-body-size-limit#task#3, template-count-limits#task#2 and #task#6).

Cause 2 of 2 — a Parameters entry whose Type is not a recognised CloudFormation parameter type.
AWS: 400 with Error.Type 'Sender', Error.Code 'ValidationError' and Message 'Template format error: Unrecognized parameter type: <declaredType>' — fixed text 'Template format error: Unrecognized parameter type: ' followed by the parameter type string exactly as the template declared it (the recorded case declares the lower-case 'string', which AWS does not accept; the accepted types are String, Number, List<Number>, CommaDelimitedList and the AWS::* / SSM parameter types).
Emulator: 200 with a ValidateTemplateResult — the parameter-type check does not exist, so the template is reported valid.
Why: ValidateTemplate is the pre-flight gate; a template with an unrecognised parameter type passes validation here and then fails at CreateStack, so the authoring error surfaces only at deploy time (and a caller that gates deployment on ValidateTemplate deploys a template AWS will never accept).
Fix: the Parameters section validation in the template parser used by ValidateTemplate — enumerate the allowed parameter types and emit the ValidationError above.
Traces: cloudformation-eescfn-stage-parameter-declaration-mistakes#task#1 (cluster also covers cloudformation-eescfn-stage-parameter-declaration-mistakes#task#2 and template-count-limits#task#5).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 2 divergence clusters on service=cloudformation, operation=ValidateTemplate, both stub_path='' and owned_by=[].
Cause 1 (9 affected steps) — expected 400 / got 400, $.ErrorResponse.Error.Message: value_mismatch; expected 'Template error: every Fn::FindInMap object requires three parameters, the map name, map key and the attribute for return value', got 'Template error: every Fn::FindInMap object requires three parameters, the map name, the top-level key, and the second-level key'. Entire message is fixed text; no part of it comes from the request.
Cause 2 (3 affected steps) — status: expected 400, got 200; $.ErrorResponse missing, expected {'Error': {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Template format error: Unrecognized parameter type: string'}}. Fixed text 'Template format error: Unrecognized parameter type: '; the trailing token is the Type value the request's Parameters section declared ('string' in the recording).
Smithy (services/service-com-amazonaws-cloudformation-cloudformation): ValidateTemplate input TemplateBody/TemplateURL, output Parameters/Description/Capabilities/CapabilitiesReason/DeclaredTransforms, errors: [] — both refusals are the service-common Sender/ValidationError.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Cause 1: fixture cloudformation-eescfn-findinmap-and-mappings-mistakes, step task#1 — ValidateTemplate on a template whose Fn::FindInMap is given the wrong number of parameters; compare ErrorResponse.Error.Message byte-for-byte with the recorded AWS message.
3. Cause 2: fixture cloudformation-eescfn-stage-parameter-declaration-mistakes, step task#1 — ValidateTemplate on a template whose Parameters entry declares Type 'string'; expect 400 Sender/ValidationError 'Template format error: Unrecognized parameter type: string', observe 200 with a ValidateTemplateResult.
4. Re-replay the other fixture#step bindings in the clusters (cloudformation-eescfn-sub-mapping-attribute-reference, cloudformation-eescfn-validate-expansion-by-s3-url, template-body-size-limit, template-count-limits) to confirm both checks across every recorded step.
