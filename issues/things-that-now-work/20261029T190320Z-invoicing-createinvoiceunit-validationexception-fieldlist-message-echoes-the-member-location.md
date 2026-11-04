Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-invoice-unit-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-invoice-unit-errors
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|CreateInvoiceUnit|4xx->4xx|$.fieldList[*].message
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# CreateInvoiceUnit ValidationException fieldList message echoes the member location instead of the redacted-value wording

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvoiceUnit

## Description
Cause 1 of 1 - the per-field message inside CreateInvoiceUnit's ValidationException is rendered with the emulator's own phrasing.
AWS: CreateInvoiceUnit.InvoiceReceiver targets AccountIdString with pattern ^\\d{12}$ (Smithy: CreateInvoiceUnit.InvoiceReceiver required, pattern ^\\d{12}$; error ValidationException, httpError 400, with fieldList of ValidationExceptionField{name, message}). A receiver that does not match is refused HTTP 400 ValidationException with one fieldList entry whose message is "Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}". The fixed text of that message is "Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: "; the only variable part is the pattern itself, taken from the member's constraint and rendered unanchored (\\d{12}, not ^\\d{12}$). Because the rejected value is an account id, AWS does NOT echo it and does NOT name the member location inside the message - the literal token [redacted] stands in for the value, and the member is identified only by the entry's own name member.
Emulator: emits "Value at 'invoiceReceiver' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}" - it substitutes the member location for the redaction marker, so the message's fixed text differs from AWS's for this error class. Status (400), error class and the entry's name member all match; only the message text diverges.
Why: clients and SDK test suites match on the fixed text of a constraint-violation message, and support tooling treats the [redacted] marker as the signal that the value was deliberately withheld. A message carrying the member location where AWS places the redaction marker both fails that matching and implies the emulator would echo a caller value AWS suppresses - exactly the wrong expectation for an account identifier.
Fix: the invoicing ValidationException field-message renderer used by CreateInvoiceUnit's input validation - render pattern violations as the redacted-value form above and leave member identification to ValidationExceptionField.name.
Traces: invoicing-invoice-unit-errors#task#1 (CreateInvoiceUnit). Note: the related issue 20261022T124151Z-invoicing-validationexception-payload-nested-fieldlist-entries-omit-type.md is already resolved and does not cover this step - the replay reports no owner for it.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture invoicing-invoice-unit-errors step task#1, operation CreateInvoiceUnit, verdict mismatch, status 400 on both sides.

$.fieldList[0].message: value_mismatch
  expected: 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}'
  actual:   "Value at 'invoiceReceiver' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}"

Model grounding (services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json): CreateInvoiceUnit input member InvoiceReceiver -> AccountIdString, required, pattern ^\\d{12}$; CreateInvoiceUnit errors include ValidationException (httpError 400) whose fieldList is ValidationExceptionFieldList of ValidationExceptionField{name required, message required, each 0..1024}.

## Steps to Reproduce
1. Replay fixture invoicing-invoice-unit-errors against the emulator (replay_wire_corpus on trace set simparity-wire-aws-v1).
2. Step task#1: call CreateInvoiceUnit with an InvoiceReceiver that does not match ^\\d{12}$.
3. Inspect the 400 ValidationException body: fieldList[0].message reads "Value at 'invoiceReceiver' failed to satisfy constraint: ..." instead of "Value [redacted] failed to satisfy constraint: ...".
