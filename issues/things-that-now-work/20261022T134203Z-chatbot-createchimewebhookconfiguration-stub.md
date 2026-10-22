Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chatbot.associate-concurrent-unsupported-target-fails-atomically chatbot.associate-to-unsupported-target-fails-without-partial-state chatbot.associate-wrong-resource-type-rejected chatbot.chime-webhook-create-state-enabled chatbot.chime-webhook-delete-removes-resource chatbot.chime-webhook-duplicate-configuration-name-conflict-rejected chatbot.chime-webhook-tags-at-creation-reflected chatbot.chime-webhook-update-description-persists chatbot.chime-webhook-update-invalid-webhook-url-rejected-state-preserved chatbot.chime-webhook-update-sns-topics-and-logging-level-persists chatbot.describe-chime-webhook-configurations-filters-by-arn chatbot.describe-chime-webhook-configurations-pagination-returns-all-created chatbot.disassociate-from-configuration-malformed-resource-arn-rejected chatbot.disassociate-nonexistent-association-idempotent chatbot.list-associations-maxresults-bounds-rejected
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot|CreateChimeWebhookConfiguration|NotImplemented
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Implement chatbot CreateChimeWebhookConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChimeWebhookConfiguration

## Description
`CreateChimeWebhookConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/create_chime_webhook_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

15 recorded unit(s) fail on their `CreateChimeWebhookConfiguration` call, so each one unblocks only when `CreateChimeWebhookConfiguration` works: `chatbot.associate-concurrent-unsupported-target-fails-atomically`, `chatbot.associate-to-unsupported-target-fails-without-partial-state`, `chatbot.associate-wrong-resource-type-rejected`, `chatbot.chime-webhook-create-state-enabled`, `chatbot.chime-webhook-delete-removes-resource`, `chatbot.chime-webhook-duplicate-configuration-name-conflict-rejected`, `chatbot.chime-webhook-tags-at-creation-reflected`, `chatbot.chime-webhook-update-description-persists`, `chatbot.chime-webhook-update-invalid-webhook-url-rejected-state-preserved`, `chatbot.chime-webhook-update-sns-topics-and-logging-level-persists`, `chatbot.describe-chime-webhook-configurations-filters-by-arn`, `chatbot.describe-chime-webhook-configurations-pagination-returns-all-created`, `chatbot.disassociate-from-configuration-malformed-resource-arn-rejected`, `chatbot.disassociate-nonexistent-association-idempotent`, `chatbot.list-associations-maxresults-bounds-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/create_chime_webhook_configuration.rs`. Replaying the corpus, the emulator refused the `CreateChimeWebhookConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateChimeWebhookConfiguration operation: operation CreateChimeWebhookConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the chatbot emulator
Call CreateChimeWebhookConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
