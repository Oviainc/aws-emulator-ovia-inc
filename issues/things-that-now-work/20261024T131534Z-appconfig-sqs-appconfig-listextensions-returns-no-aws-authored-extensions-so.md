Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages
Issue-Key: appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages|_find_aws_authored_sqs_extension#12|AssertionError|assert aws_authored, (
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# AppConfig ListExtensions returns no AWS-authored extensions, so the predefined notification extensions cannot be associated

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListExtensions

## Description
One root cause, grounded in a replayed behavioral check whose service token is the multi-service token 'appconfig__sqs'; the defect itself is in the appconfig control plane (both constituent services, appconfig and sqs, are stood up, and the emulator answered this call successfully).

AWS: AppConfig ships AWS-authored (predefined) extensions that ListExtensions returns to EVERY account alongside the caller's own extensions — among them the deployment-event notification extensions for Amazon SQS, Amazon SNS and EventBridge (documented in the AppConfig user guide under the predefined notification extensions; the check discovers the extension by its AWS-authored summary rather than hardcoding a name). Those summaries are what a caller passes to CreateExtensionAssociation to get deployment lifecycle notifications. Smithy: ListExtensions GET /extensions, success 200, output Items: ExtensionSummaries (paginated NextToken/MaxResults), errors BadRequestException 400 / InternalServerException 500 — the model does not enumerate which extensions exist, so the presence of the AWS-authored set is documented/recorded behavior only. The replaying check is additionally marked DRAFT in the suite (no ground-truth sidecar), so its exact expectations beyond 'at least one AWS-authored extension is listed' are the check's own hypotheses.
Emulator: ListExtensions answers 200 with Items == [] for a fresh account — only user-created extensions are ever returned, and no AWS-authored extension exists to discover.
Why: the entire predefined-extension path is unreachable: a client cannot discover or associate the AWS-authored 'deployment events to Amazon SQS' (or SNS/EventBridge) extension, so deployment lifecycle notifications can never be wired up the way they are in production, and any code that looks the extension up by its AWS-authored summary fails before it can associate anything.
Fix: the appconfig extension store/seed — register the AWS-authored extensions (name, id, version, description, actions and parameter definitions) as account-visible defaults so ListExtensions and GetExtension return them and CreateExtensionAssociation accepts them.
Traces: appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages.

## Evidence
Behavioral replay (simparity-behavioral-ga-prod-v1), check appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages [mismatch]:

AssertionError: no AWS-authored extension is visible to this account; ListExtensions returned []

raised from the check's _find_aws_authored_sqs_extension helper, which paginates ListExtensions and selects the AWS-authored summary (it does not hardcode the name). Expected: ListExtensions 200 whose Items include at least the AWS-authored deployment-event notification extensions. Actual: 200 with an empty Items list. The check's later assertions about enqueued lifecycle messages were never reached, so nothing is claimed here about SQS delivery.

## Steps to Reproduce
With a fresh emulator state, call appconfig.list_extensions() (and paginate). The emulator returns 200 with Items == []. Against AWS the same call returns the AWS-authored predefined extensions (including the deployment-events-to-Amazon-SQS extension) even in an account that has created none of its own; GetExtension on one of those ids succeeds and CreateExtensionAssociation can reference it. Re-replay appconfig__sqs.deployment-events-to-sqs-extension-delivers-lifecycle-messages to confirm.
