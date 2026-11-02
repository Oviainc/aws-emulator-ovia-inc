Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__sns.publish-fails-while-kms-master-key-is-disabled
Issue-Key: kms__sns.publish-fails-while-kms-master-key-is-disabled|run#18|Timeout|time.sleep(2)
Issue-Commit: 6a9be084645386b6802b5798f23439463b3480cb

# kms__sns: Publish to an SSE-KMS topic keeps succeeding after the topic's CMK is disabled — SNS never consults the key's live state

> _Note: this issue was identified against the code base at commit `6a9be084645386b6802b5798f23439463b3480cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
ONE net-new root cause, grounded in the one failing check of this suite's `kms__sns` service that the replay report marks neither ALREADY FILED nor OWNED.

Related OPEN issues that do NOT carry this check's current replay key (named for context only, they do not cover this failure): 20261029T175759Z-kms-sns-publish-to-an-sse-kms-topic.md (same surface; its recorded Issue-Key is `kms__sns.publish-fails-while-kms-master-key-is-disabled|run#18|Timeout|time.sleep(20)`, while the check now polls on a 2 s sleep, so the report no longer attributes this failure to it), 20261027T030148Z-sns-6-net-new-behavioral-root-causes-no.md (its Cause 4 is the sibling key-STATE divergence on the `sns` service) and 20261025T025317Z-sns-authorizes-nothing-on-the-data-plane-topic.md (which is about the key POLICY, not the key's lifecycle state).

--- Cause 1 of 1: Publish to a topic whose KmsMasterKeyId names a DISABLED customer CMK is accepted instead of refused KMSDisabled ---

AWS: Smithy (com.amazonaws.sns#Publish) declares `KMSDisabledException` with awsQueryError code `KMSDisabled` and httpError 400 — documented as "The request was rejected because the specified customer master key (CMK) isn't enabled" — precisely for this input. Recorded behavior: a topic created with Attributes `KmsMasterKeyId` = a customer CMK (supplied here as an alias) envelopes every message under that key, so SNS must resolve the key on the publish path; once the key is disabled, Publish is refused HTTP 400 with Error.Code exactly `KMSDisabled` (not a generic InvalidParameter/AuthorizationError), every subsequent Publish is refused the same way while the key stays disabled, and EnableKey restores Publish. The refusal is state-driven, never a one-off: the fixed part of the contract is the status (400) and the code (`KMSDisabled`); the key identity in the message is a value taken from the topic's own KmsMasterKeyId, never a literal from one run.

Emulator: every Publish after DisableKey still answers 200 with a MessageId. KmsMasterKeyId is stored as an opaque topic attribute at CreateTopic and no KMS call is made per publish, so the refusal the check polls for never appears; the check's bounded poll (which exits as soon as any non-success code is seen) therefore runs until the harness kills it — "Error: Check timed out after 60.0s". The positive control (Publish while the key is Enabled) succeeds, so the topic itself is usable and the timeout is caused solely by the refusal never arriving. Calibration note for the fixer: real AWS caches a topic's data key for roughly 300 s after DisableKey, so the check budgets 420 s; the 60 s harness cap cannot cover that latency on its own — confirm the fix with a direct DisableKey + Publish probe rather than only the timed replay.

Why: disabling a topic's CMK is the standard break-glass control for stopping an encrypted topic from accepting traffic. A client told the publish succeeded believes its message was enveloped under a key that could not have encrypted it, a security runbook that proves revocation by a failing Publish silently passes, and the `KMSDisabled` code that callers branch on (retry after re-enabling, versus a permanent failure) is unreachable.

Fix: the SNS Publish handler's encryption/key-resolution path (and the same path used by PublishBatch) — resolve the topic's KmsMasterKeyId, aliases included, and request the data key from KMS per publish, mapping a disabled key onto the already-modelled KMSDisabledException channel.

Traces: kms__sns.publish-fails-while-kms-master-key-is-disabled (mismatch, 60 s timeout inside the post-DisableKey Publish poll). Handler component: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs.

## Evidence
Failing check (suite service heading `### kms__sns`): `kms__sns.publish-fails-while-kms-master-key-is-disabled` — "Error: Check timed out after 60.0s".

Expected: after kms:DisableKey on the CMK named by the topic's KmsMasterKeyId, Publish → HTTP 400, Error.Code `KMSDisabled` (Smithy: Publish declares KMSDisabledException, awsQueryError code `KMSDisabled`, httpError 400); a second Publish while the key is still disabled is refused identically; after kms:EnableKey Publish succeeds again with a MessageId.

Actual: Publish keeps returning HTTP 200 with a MessageId in the disabled state. The check names a success `"<published>"` precisely because "A Publish that SUCCEEDS while the key is disabled is the over-permissive failure this check exists to catch"; its poll only breaks on a non-success code, so the harness killed it at 60 s with every post-DisableKey attempt accepted. The pre-DisableKey control Publish returned a MessageId, proving the CMK-encrypted topic was usable before the single thing under test was broken.

## Steps to Reproduce
1. kms:CreateKey; kms:CreateAlias pointing at it.
2. sns:CreateTopic with Attributes={"KmsMasterKeyId": "<that alias>"}.
3. sns:Publish(TopicArn=<topic>, Message="control-before-disable") — succeeds with a MessageId (positive control).
4. kms:DisableKey(KeyId=<key>); kms:DescribeKey confirms KeyState="Disabled".
5. sns:Publish(TopicArn=<topic>, Message="probe") repeatedly — EXPECT HTTP 400 with Error.Code `KMSDisabled`; OBSERVE HTTP 200 with a MessageId on every attempt.
6. kms:EnableKey(KeyId=<key>); sns:Publish succeeds again (recovery leg).
Replay: kms__sns.publish-fails-while-kms-master-key-is-disabled.
