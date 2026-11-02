Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-webhook-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-webhook-lifecycle codepipeline-webhook-put-list-deregister-delete-lifecycle codepipeline-webhook-error-paths
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|PutWebhook|2xx->4xx|$.webhook
Issue-Key: codepipeline|PutWebhook|2xx->4xx|status
Issue-Key: codepipeline|ListWebhooks|2xx->2xx|$.webhooks[*]
Issue-Key: codepipeline|PutWebhook|4xx->4xx|$.message
Issue-Key: codepipeline|DeregisterWebhookWithThirdParty|4xx->4xx|$.__type
Issue-Key: codepipeline|DeregisterWebhookWithThirdParty|4xx->4xx|$.message
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodePipeline webhook lifecycle: PutWebhook rejects a valid GITHUB_HMAC webhook, so no webhook is ever stored

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutWebhook

## Description
The webhook surface rejects valid input, therefore serves an empty webhook list, and misclassifies one refusal.

Cause 1 of 4 — PutWebhook refuses a well-formed GITHUB_HMAC webhook. AWS: 200 with output member webhook -> ListWebhookItem (Smithy: PutWebhook input webhook -> WebhookDefinition with required name/targetPipeline/targetAction/filters/authentication/authenticationConfiguration; for authentication=GITHUB_HMAC the model documents that "only the SecretToken property must be set"). The recorded request supplies authentication='GITHUB_HMAC' with authenticationConfiguration={'SecretToken': ...}, exactly what the model requires, and AWS answers 200 with the webhook echoed (its arn and url generated, the secret token redacted as '****' per the model's note). Emulator: 400 and no webhook member — it rejects the request it should accept. Why: no webhook can be created at all, so every downstream webhook flow (list, register/deregister with third party, delete) is unreachable. Fix: PutWebhook's authenticationConfiguration validation — accept SecretToken for GITHUB_HMAC — in the webhook handler's validation, before the store write. Traces: codepipeline-webhook-lifecycle#task#1 (3 affected steps, incl. codepipeline-webhook-put-list-deregister-delete-lifecycle#task#2).

Cause 2 of 4 — ListWebhooks returns no items. AWS: 200 with webhooks[0] being the webhook just created, carrying arn, url, the definition (with SecretToken redacted) and its tags. Emulator: 200 with the item absent. Why: a client enumerating webhooks after creating one sees none. Fix: webhook state/ListWebhooks projection (follows from Cause 1 but is a distinct read-path divergence; the item must be present with definition, tags, arn and url). Traces: codepipeline-webhook-lifecycle#task#2 (2 affected steps).

Cause 3 of 4 — PutWebhook's invalid-auth-config message. AWS: 400 with message "Optional['authenticationConfig' should contain only one property: 'SecretToken']" — fixed text naming the single permitted property for the authentication type. Emulator: 'authentication GITHUB_HMAC requires authenticationConfiguration.SecretToken'. Fix: the message emitted by PutWebhook's authenticationConfiguration validation. Traces: codepipeline-webhook-error-paths#task#1 (2 affected steps).

Cause 4 of 4 — DeregisterWebhookWithThirdParty misclassifies a webhook whose target pipeline is gone. AWS: 400 ValidationException with message "Webhook <webhookName> references a pipeline <pipelineName> which does not exist" — fixed text with the webhook name and its targetPipeline from state (Smithy: DeregisterWebhookWithThirdParty errors = ValidationException, WebhookNotFoundException). Emulator: 400 WebhookNotFoundException with no message member. Why: the webhook DOES exist; a client branching on WebhookNotFoundException concludes it was already removed and skips cleanup, and gets no text explaining the real cause. Fix: DeregisterWebhookWithThirdParty's resolution order — resolve the webhook first, then raise ValidationException for its dangling targetPipeline. Traces: codepipeline-webhook-put-list-deregister-delete-lifecycle#task#4.

## Evidence
codepipeline-webhook-lifecycle#task#1 (PutWebhook): 'status: expected 200, got 400', "$.webhook: missing (expected {'arn': ..., 'definition': {'authentication': 'GITHUB_HMAC', 'authenticationConfiguration': {'SecretToken': '****'}, 'filters': [{'jsonPath': '$.ref', 'matchEquals': 'refs/heads/main'}], ...}, 'tags': [...], 'url': ...}, got None)" (3 affected steps).
codepipeline-webhook-lifecycle#task#2 (ListWebhooks, 200->200): '$.webhooks[0]: missing (expected the created webhook item, got None)' (2 affected steps).
codepipeline-webhook-error-paths#task#1 (PutWebhook, 400->400): expected "Optional['authenticationConfig' should contain only one property: 'SecretToken']" / got 'authentication GITHUB_HMAC requires authenticationConfiguration.SecretToken' (2 affected steps).
codepipeline-webhook-put-list-deregister-delete-lifecycle#task#4 (DeregisterWebhookWithThirdParty, 400->400): '$.__type: expected ValidationException, got WebhookNotFoundException', '$.message: missing (expected "Webhook <name> references a pipeline <pipeline> which does not exist")'.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus.
2. Create a target pipeline, then PutWebhook with authentication='GITHUB_HMAC' and authenticationConfiguration={'SecretToken': '<token>'} -> expect 200 with the webhook echoed (SecretToken redacted), observe 400 with no webhook.
3. ListWebhooks -> expect the created item, observe an empty list.
4. PutWebhook with a GITHUB_HMAC authenticationConfiguration that is not exactly {'SecretToken': ...} -> compare the refusal message.
5. Delete the target pipeline of an existing webhook and call DeregisterWebhookWithThirdParty -> expect ValidationException with the dangling-pipeline message, observe WebhookNotFoundException with no message.
6. Re-replay codepipeline-webhook-lifecycle#task#1/#task#2, codepipeline-webhook-error-paths#task#1 and codepipeline-webhook-put-list-deregister-delete-lifecycle#task#4.
