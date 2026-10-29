Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-ad-connector-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-ad-connector-validation-errors pca-connector-ad-template-validation-errors
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad|CreateConnector|4xx->4xx|$.message
Issue-Key: pca-connector-ad|CreateTemplate|4xx->4xx|$.message
Issue-Key: pca-connector-ad|CreateTemplate|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: pca-connector-ad|CreateTemplate|4xx->4xx|status
Issue-Commit: c2fc8676d63317349fbaa2ea39a789b7f43b5131

# pca-connector-ad constraint rejections use the generic validation template and run after resource lookup

> _Note: this issue was identified against the code base at commit `c2fc8676d63317349fbaa2ea39a789b7f43b5131`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnector

## Description
Two net-new wire causes in how this service reports modeled-constraint violations on request-body members: the message is rendered in the wrong family of wording, and for CreateTemplate.Name the constraint is not evaluated at all, so the request reaches the handler and is refused with the wrong status and error class.

Cause 1 of 2: CreateConnector's @pattern violation on DirectoryId is reported with the cross-service "1 validation error detected" template instead of this service's bracketed JSON-schema wording.
AWS: HTTP 400, code ValidationException (Smithy: CreateConnector models ValidationException @httpError 400, Message @required; CreateConnector.DirectoryId @required @pattern `^d-[0-9a-f]{10}$`), message `[ECMA 262 regex "<pattern as the model declares it, anchors included>" does not match input string "<rejected value>"]`. Fixed text: `[ECMA 262 regex `, ` does not match input string `, `]` and the double-quoting; variables: the modeled pattern and the value taken from the request. There is NO member path and NO error-count prefix in this family. Recorded behavior only for the choice of family.
Emulator: answers 400 ValidationException (status and code correct) with `1 validation error detected: Value '<value>' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$`.
Why: a client that matches or displays the refusal text gets a message AWS never sends for this operation, and loses the ECMA-262 framing AWS uses to report body-member constraint failures.
Fix: the pca-connector-ad validation layer's message renderer for modeled @pattern/@length/enum failures on body members (it must emit the bracketed ECMA-262 form for this service, not the generic cross-service template).
Traces: fixture pca-connector-ad-connector-validation-errors, step task#1 (cluster also covers task#2 of the same fixture).

Cause 2 of 2: CreateTemplate does not enforce Name's @pattern, so a Name containing forbidden characters is never refused; the handler resolves the parent connector first and answers a 404 about an unrelated resource.
AWS: HTTP 400, header x-amzn-errortype ValidationException, message `[ECMA 262 regex "^(?!^\s+$)((?![\x5c'\x2b,;<=>#\x22])([\x20-\x7E]))+$" does not match input string "<the rejected Name>"]` — same fixed text as Cause 1, with the pattern from the model and the Name from the request. Input validation precedes the parent lookup, so AWS answers ValidationException even when ConnectorArn names a connector that does not exist. Smithy: CreateTemplate.Name -> TemplateName @required @length 1..64 @pattern `^(?!^\s+$)((?![\x5c'\x2b,;<=>#\x22])([\x20-\x7E]))+$`; CreateTemplate models both ValidationException @httpError 400 and ResourceNotFoundException @httpError 404.
Emulator: the Name violation is not detected; the request reaches the handler, which looks up the ConnectorArn and returns HTTP 404 with x-amzn-errortype ResourceNotFoundException and no `message` member at all. Status, error class and body all diverge.
Why: the client is told the connector is missing rather than which of its inputs is invalid, and against a real connector a template name AWS forbids would be persisted; the rejection-ordering contract (modeled validation before resource resolution) is inverted, so retry/repair logic keyed on ValidationException never fires.
Fix: input validation for CreateTemplate.Name in the service's constraint-compilation/validation layer, evaluated before the connector lookup in services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/create_template.rs. Hypothesis: this @pattern uses negative lookahead `(?!...)`, unsupported by the Rust `regex` crate, so the compiled constraint is dropped silently instead of enforced.
Traces: fixture pca-connector-ad-template-validation-errors, step task#1.

Related open issue (does NOT cover these wire steps): 20261029T190840Z-pca-connector-ad-diverges-on-seven-counts-validationexception.md reports the same two faults from the behavioral corpus (its Causes 1 and 5); it carries no wire replay key, so neither wire step above is gated by it.

## Evidence
Cause 1 — pca-connector-ad-connector-validation-errors#task#1 (CreateConnector, DirectoryId='not-a-directory-id'); same cluster covers …#task#2. Status: expected 400, actual 400.
  expected $.message: '[ECMA 262 regex "^d-[0-9a-f]{10}$" does not match input string "not-a-directory-id"]'
  actual   $.message: "1 validation error detected: Value 'not-a-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$"

Cause 2 — pca-connector-ad-template-validation-errors#task#1 (CreateTemplate, Name='Bad+Name;Not<Allowed>', ConnectorArn naming a nonexistent connector):
  status: expected 400, got 404
  $headers.x-amzn-errortype: expected 'ValidationException', got 'ResourceNotFoundException'
  $.message: expected '[ECMA 262 regex "^(?!^\\s+$)((?![\\x5c\'\\x2b,;<=>#\\x22])([\\x20-\\x7E]))+$" does not match input string "Bad+Name;Not<Allowed>"]', got None (the emulator's 404 body carries no message member)

## Steps to Reproduce
Boot the emulator and replay simparity-wire-aws-v1, or drive it directly (region us-west-2, client-side validation disabled):
1. CreateConnector(DirectoryId='not-a-directory-id', CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={'SecurityGroupIds': ['sg-0123456789abcdef0']}) -> 400 ValidationException whose message is the generic '1 validation error detected: ...' template; AWS answers the bracketed '[ECMA 262 regex "^d-[0-9a-f]{10}$" does not match input string "not-a-directory-id"]'. (pca-connector-ad-connector-validation-errors#task#1)
2. CreateTemplate(ConnectorArn=<well-formed ARN of a connector that does not exist>, Name='Bad+Name;Not<Allowed>', Definition={'TemplateV2': {...}}) -> 404 ResourceNotFoundException with no message member; AWS answers 400 ValidationException with the bracketed ECMA-262 message for Name, before it ever looks the connector up. (pca-connector-ad-template-validation-errors#task#1)
