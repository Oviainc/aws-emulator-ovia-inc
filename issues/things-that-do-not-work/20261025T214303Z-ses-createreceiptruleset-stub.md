Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption ses.clone-receipt-rule-set-copies-rules ses.concurrent-receipt-rule-set-create-exactly-one-wins ses.concurrent-update-receipt-rule-last-write-wins ses.configuration-set-update-on-nonexistent-rejected ses.create-receipt-rule-invalid-header-name-rejected ses.create-receipt-rule-invalid-s3-bucket-fails ses.create-receipt-rule-rich-actions-round-trip ses.delete-receipt-rule-missing-rule-is-noop ses.receipt-rule-create-after-nonexistent-rejected ses.receipt-rule-creation-prepends-and-set-position-reorders ses.receipt-rule-describe-update-and-position ses.receipt-rule-duplicate-name-rejected ses.receipt-rule-empty-recipients-list-vs-absent-persists ses.receipt-rule-lambda-action-requires-a-rule-scoped-invoke-permission ses.receipt-rule-lifecycle-and-position ses.receipt-rule-s3-action-kms-key-policy-is-evaluated-apart-from-the-bucket-grant ses.receipt-rule-s3-action-requires-a-live-bucket-policy-grant ses.receipt-rule-set-active-lifecycle ses.receipt-rule-set-clone-preserves-order-and-stays-independent ses.receipt-rule-set-delete-while-active-rejected ses.reorder-receipt-rule-set-duplicate-and-unknown-rule-rejected ses.reorder-receipt-rule-set-missing-rule-fails ses.set-active-receipt-rule-set-nonexistent-rejected ses.update-receipt-rule-full-replace-resets-unspecified-fields ses.update-receipt-rule-invalid-lambda-function-rejected ses.update-receipt-rule-invalid-s3-configuration-rejected ses.update-receipt-rule-invalid-sns-topic-rejected ses.update-receipt-rule-on-nonexistent-rule-rejected ses.update-receipt-rule-replaces-actions-and-preserves-position ses__sns.receipt-rule-sns-action-honours-an-explicit-topic-policy-deny
Issue-Key: ses|CreateReceiptRuleSet|NotImplemented
Issue-Related: 20261023T120410Z-ses-v1-and-v2-is-entirely-unreachable-the.md
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# Implement ses CreateReceiptRuleSet: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReceiptRuleSet

## Description
`CreateReceiptRuleSet` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_receipt_rule_set.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

34 recorded unit(s) fail on their `CreateReceiptRuleSet` call, so each one unblocks only when `CreateReceiptRuleSet` works: `s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers`, `s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule`, `s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact`, `s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption`, `ses.clone-receipt-rule-set-copies-rules`, `ses.concurrent-receipt-rule-set-create-exactly-one-wins`, `ses.concurrent-update-receipt-rule-last-write-wins`, `ses.configuration-set-update-on-nonexistent-rejected`, `ses.create-receipt-rule-invalid-header-name-rejected`, `ses.create-receipt-rule-invalid-s3-bucket-fails`, `ses.create-receipt-rule-rich-actions-round-trip`, `ses.delete-receipt-rule-missing-rule-is-noop`, `ses.receipt-rule-create-after-nonexistent-rejected`, `ses.receipt-rule-creation-prepends-and-set-position-reorders`, `ses.receipt-rule-describe-update-and-position`, `ses.receipt-rule-duplicate-name-rejected`, `ses.receipt-rule-empty-recipients-list-vs-absent-persists`, `ses.receipt-rule-lambda-action-requires-a-rule-scoped-invoke-permission`, `ses.receipt-rule-lifecycle-and-position`, `ses.receipt-rule-s3-action-kms-key-policy-is-evaluated-apart-from-the-bucket-grant` (+14 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_receipt_rule_set.rs`. Replaying the corpus, the emulator refused the `CreateReceiptRuleSet` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateReceiptRuleSet operation: operation CreateReceiptRuleSet is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ses emulator
Call CreateReceiptRuleSet and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
