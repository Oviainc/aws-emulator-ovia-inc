Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-invoice-unit-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-invoice-unit-errors
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|CreateInvoiceUnit|4xx->4xx|$.fieldList[*].__type
Issue-Key: invoicing|CreateInvoiceUnit|4xx->4xx|$.fieldList[*].message
Issue-Key: invoicing|CreateInvoiceUnit|4xx->4xx|$.message
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# Invoicing ValidationException payload: nested fieldList entries omit __type and messages echo the rejected value with an anchored pattern

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvoiceUnit

## Description
The emulator rejects a malformed InvoiceReceiver on CreateInvoiceUnit with the right HTTP status (400) and the right error shape (ValidationException), but it constructs the ValidationException *payload* differently from AWS in two ways on the same response. Both are in one cause because both are produced by the same error-rendering path for this service.

Cause 1 of 1 (two facets, both observed on fixture invoicing-invoice-unit-errors step task#1 - a fixer must close BOTH before submitting):

Facet (a) - nested ValidationExceptionField entries carry no __type discriminator.
AWS: Invoicing is awsJson1_0 (Smithy: service Invoicing 2024-12-01, protocols [awsJson1_0]; ValidationException @error client, httpError 400, member fieldList -> ValidationExceptionFieldList of ValidationExceptionField{name,message}). The recorded AWS response emits a __type member on each fieldList element, carrying the backend shape id of the field structure (recorded example: "com.amazonaws.awsinsightsindexservice.v0#ValidationExceptionField"). The Smithy model does not mandate a __type on nested structures, so this is recorded behavior only: what is required is that each fieldList element carries the __type discriminator member at all; the exact namespace string is a recorded example and an environment-specific backend detail, not a requirement.
Emulator: the fieldList element is serialized with no __type member at all ($.fieldList[0].__type: missing, expected a shape-id string, got None).
Why: a client that keys off the JSON discriminator to decide how to decode nested error detail (hand-rolled decoders, generic error-introspection and error-logging middleware, protocol conformance harnesses) sees a differently-shaped error document than AWS produces, and nested field detail silently decodes to an untyped object.
Fix: the invoicing error serializer for ValidationException - the awsJson1_0 structure writer used for nested error members - must write the __type discriminator on ValidationExceptionField elements the way it already does for the top-level error.

Facet (b) - the validation message echoes the rejected value and the anchored regex.
AWS: the message is built from the request, so its rule is known. Smithy: CreateInvoiceUnit.InvoiceReceiver is @required, targets AccountIdString with @pattern "^\\d{12}$"; a violation is ValidationException (httpError 400). AWS renders the pattern violation as "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern-without-anchors>" - i.e. the submitted value is NOT reproduced in the text, and the pattern is reported with the ^ and $ anchors stripped (recorded example: "1 validation error detected: Value at 'invoiceReceiver' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}"). The same two rules apply to the per-field fieldList[].message. The status (400) and the error shape/code (ValidationException) are correct and are NOT part of this finding.
Emulator: it inlines the raw submitted value and the anchored model pattern in both texts - got "1 validation error detected: Value 'not-an-account-id' at 'invoiceReceiver' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$" at $.message, and the same construction at $.fieldList[0].message.
Why: a client relying on AWS's message rule gets two things wrong. It reproduces caller-supplied input back into an error string AWS deliberately omits, so anything that surfaces or persists the message (user-facing error banners, logs, audit records) leaks the rejected value in the emulator but not in production - the opposite of the real redaction behavior. And it reports the internal anchored regex rather than the anchor-free pattern AWS publishes, so tests, docs fixtures and clients that match or display the advertised constraint diverge against the emulator.
Fix: the CreateInvoiceUnit input validation / ValidationException message builder for this service - the component that formats constraint violations - must omit the offending value from the text and strip the leading ^ / trailing $ from the pattern before interpolating it, for both the top-level message and each fieldList entry.

Related but NOT covering this: open issues "ACM ValidationException messages echo the input value and an anchored pattern" (20261022T122353Z-acm-...) and "SageMaker error fidelity: ValidationException messages carry regex anchors ..." (20261022T120642Z-sagemaker-...) report the same class of fault on other services. They carry no invoicing replay key, so they do not cover this step; if the message formatting is shared code, fixing it there may resolve facet (b) here, but facet (a) is invoicing's error serializer.

Traces: invoicing-invoice-unit-errors#task#1 (CreateInvoiceUnit, verdict mismatch, expected 400 / actual 400, diverging keys $.fieldList[0].__type, $.fieldList[0].message, $.message).

## Evidence
Fixture invoicing-invoice-unit-errors, step task#1, operation CreateInvoiceUnit, verdict mismatch. Expected status 400, actual status 400 (status agrees; payload does not).

1) $.fieldList[0].__type
   expected: "com.amazonaws.awsinsightsindexservice.v0#ValidationExceptionField" (a shape-id discriminator; the namespace itself is an environment-specific example)
   actual:   missing (None)

2) $.fieldList[0].message
   expected: "Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}"
   actual:   "Value 'not-an-account-id' at 'invoiceReceiver' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$"

3) $.message
   expected: "1 validation error detected: Value at 'invoiceReceiver' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}"
   actual:   "1 validation error detected: Value 'not-an-account-id' at 'invoiceReceiver' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$"

Model grounding (services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json): service Invoicing, apiVersion 2024-12-01, protocol awsJson1_0; CreateInvoiceUnit.InvoiceReceiver @required -> AccountIdString @pattern "^\\d{12}$"; CreateInvoiceUnit errors include ValidationException (client, httpError 400); ValidationException members {message, resourceName, reason, fieldList}; ValidationExceptionField members {name @required, message @required}.

Deterministic replay context: trace set simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, state_reset_failures 0 (so this is not contaminated by a previous fixture's state), divergence cluster affected_count 1, stub_path empty, owned_by empty - i.e. net-new and not blocked by an unimplemented operation.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1); the divergence reproduces at fixture invoicing-invoice-unit-errors, step task#1.
3. Equivalently, by hand: send an awsJson1_0 CreateInvoiceUnit request to the invoicing endpoint with a syntactically invalid InvoiceReceiver, e.g. target X-Amz-Target: Invoicing.CreateInvoiceUnit with body {"Name":"<valid name>","InvoiceReceiver":"not-an-account-id","Rule":{...}}.
4. Observe the 400 ValidationException body. Assert, against AWS's rule: (a) each fieldList element carries a __type discriminator member - the emulator omits it entirely; (b) neither $.message nor $.fieldList[0].message reproduces the submitted value, and both report the pattern with the ^/$ anchors stripped (\\d{12}) - the emulator inlines 'not-an-account-id' and emits ^\\d{12}$.
5. After a fix, re-replay invoicing-invoice-unit-errors#task#1; the step must pass on all three JSON paths.
