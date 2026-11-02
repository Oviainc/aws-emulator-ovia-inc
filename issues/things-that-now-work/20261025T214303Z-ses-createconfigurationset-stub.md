Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.configuration-set-delete-nonexistent ses.configuration-set-duplicate-create-fails sesv2.configuration-set-sending-disabled-blocks-v2-send-email-then-restored ses.configuration-set-sending-pause-state ses.configuration-set-tracking-options-lifecycle-with-verified-domain ses.configuration-sets-v1-and-v2-pagination-and-cross-api-token-continuation ses.delivery-options-invalid-tls-policy-rejected ses.event-destination-duplicate-name-fails ses.event-destination-lifecycle ses.event-destination-on-missing-configuration-set-rejected ses.event-destination-unowned-sns-topic-fails ses.reputation-metrics-flag-isolated-per-configuration-set ses.reputation-metrics-toggle-persists-across-describe ses.send-raw-email-unknown-configuration-set-fails sesv2.tag-resource-reserved-prefix-and-key-limit-rejected ses.tracking-options-delete-on-unset-set-fails ses.tracking-options-unverified-domain-fails ses.update-configuration-set-tracking-options-invalid-custom-redirect-rejected ses.update-configuration-set-tracking-options-missing-tracking-options-rejected
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses|CreateConfigurationSet|NotImplemented
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# Implement ses CreateConfigurationSet: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSet

## Description
`CreateConfigurationSet` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_configuration_set.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

19 recorded unit(s) fail on their `CreateConfigurationSet` call, so each one unblocks only when `CreateConfigurationSet` works: `ses.configuration-set-delete-nonexistent`, `ses.configuration-set-duplicate-create-fails`, `ses.configuration-set-sending-disabled-blocks-v2-send-email-then-restored`, `ses.configuration-set-sending-pause-state`, `ses.configuration-set-tracking-options-lifecycle-with-verified-domain`, `ses.configuration-sets-v1-and-v2-pagination-and-cross-api-token-continuation`, `ses.delivery-options-invalid-tls-policy-rejected`, `ses.event-destination-duplicate-name-fails`, `ses.event-destination-lifecycle`, `ses.event-destination-on-missing-configuration-set-rejected`, `ses.event-destination-unowned-sns-topic-fails`, `ses.reputation-metrics-flag-isolated-per-configuration-set`, `ses.reputation-metrics-toggle-persists-across-describe`, `ses.send-raw-email-unknown-configuration-set-fails`, `ses.tag-resource-reserved-prefix-and-key-limit-rejected`, `ses.tracking-options-delete-on-unset-set-fails`, `ses.tracking-options-unverified-domain-fails`, `ses.update-configuration-set-tracking-options-invalid-custom-redirect-rejected`, `ses.update-configuration-set-tracking-options-missing-tracking-options-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_configuration_set.rs`. Replaying the corpus, the emulator refused the `CreateConfigurationSet` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateConfigurationSet operation: operation CreateConfigurationSet is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ses emulator
Call CreateConfigurationSet and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
