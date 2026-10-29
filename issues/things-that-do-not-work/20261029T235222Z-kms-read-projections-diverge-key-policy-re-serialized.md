Issue-Source: trace-sweep-divergence
Issue-Trace: kms-key-policy-management
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-key-policy-management kms-key-policy-special-characters-round-trip kms-key-rotation-management kms-asymmetric-sign-verify
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|GetKeyPolicy|2xx->2xx|$.Policy
Issue-Key: kms|ListKeyRotations|2xx->2xx|$.Rotations[*].KeyId
Issue-Key: kms|GetKeyLastUsage|2xx->2xx|$.KeyLastUsage.CloudTrailEventId
Issue-Key: kms|GetKeyLastUsage|2xx->2xx|$.KeyLastUsage.KmsRequestId
Issue-Key: kms|GetKeyLastUsage|2xx->2xx|$.KeyLastUsage.Operation
Issue-Key: kms|GetKeyLastUsage|2xx->2xx|$.KeyLastUsage.Timestamp
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# KMS read projections diverge: key policy re-serialized compactly, rotation KeyId not an ARN, last-usage fabricated

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetKeyPolicy

## Description
Three KMS read paths answer 200 with a body whose VALUES are shaped differently from the recording: a policy document loses AWS's canonical formatting, a key identifier is reported as a bare id where AWS reports a full ARN, and a usage-tracking record is invented where AWS reports none yet. 3 causes.

Cause 1 of 3: GetKeyPolicy re-serializes the key policy compactly.
AWS: GetKeyPolicy returns the key policy document as KMS's own canonical pretty-printed JSON, not as the caller's bytes and not as compact JSON. The recorded document uses two-space indentation, ' : ' (space-colon-space) between a member name and its value, '[ { ... } ]' with a space inside the array/object brackets, and a trailing newline before the closing brace — the formatting AWS's serializer produces. The document's CONTENT (Version, Id, Statement with Sid/Effect/Principal/Action/Resource) is whatever policy is attached to the key, including the account-root default policy on a key created without one; only the formatting is the requirement here.
Smithy: GetKeyPolicyResponse.Policy is PolicyType, length 1..131072, pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$ — the pattern explicitly admits tab, LF and CR, i.e. the modeled value is a multi-line document. The model does not state the layout, so the recording is the requirement.
Emulator: answers 200 with Policy serialized compactly — no newlines, no indentation, '":"' with no spaces — so the string differs from the recording byte for byte even though it parses to the same object.
Why: callers round-trip this document (GetKeyPolicy -> edit -> PutKeyPolicy), diff it against a committed baseline, or hash it to detect drift; a compact re-serialization makes an unchanged policy look changed on every read, and breaks any text-level assertion or signature over the document. It also means the emulator's GetKeyPolicy output is not stable against AWS for infrastructure tools that compare policy text.
Fix: the GetKeyPolicy handler's response serializer — emit the policy document in KMS's canonical pretty-printed form rather than the compact form (and apply the same rendering wherever a stored policy is read back).
Traces: kms-key-policy-management#task#3 (4 affected steps: kms-key-policy-management#task#3, #task#5, #task#7 and kms-key-policy-special-characters-round-trip#task#3 — so it reproduces on a freshly created key, after a PutKeyPolicy, and on a document containing special characters).

Cause 2 of 3: ListKeyRotations reports Rotations[].KeyId as a bare key id instead of the key ARN.
AWS: each RotationsListEntry.KeyId is the fully qualified key ARN, arn:aws:kms:<region>:<account>:key/<key-id> — the region and account come from the key being listed, the uuid is the key's id.
Smithy: RotationsListEntry.KeyId targets KeyIdType (length 1..2048) and is documented only as 'Unique identifier of the key', so the model permits either form and the recording fixes it as the ARN.
Emulator: answers 200 with KeyId set to the bare uuid, with no arn:aws:kms:<region>:<account>:key/ prefix.
Why: a client that feeds a listed rotation's KeyId straight into a cross-region or cross-account call, or that joins it against ARNs returned by DescribeKey/ListKeys, gets an identifier that does not match and that is ambiguous outside the request's own region/account.
Fix: the ListKeyRotations handler's entry projection — render the key's ARN, as the other KMS read paths do.
Traces: kms-key-rotation-management#task#8.

Cause 3 of 3: GetKeyLastUsage fabricates a last-usage record that AWS has not yet produced.
AWS: KeyLastUsage is populated from KMS's own usage tracking, which starts at TrackingStartDate and lags the request. Immediately after a key is used, the recorded response carries a KeyLastUsage object with NONE of Operation, Timestamp, CloudTrailEventId or KmsRequestId present — AWS reports no last-usage detail until tracking has produced one.
Smithy: GetKeyLastUsageResponse models KeyId, KeyLastUsage (KeyLastUsageData), TrackingStartDate and KeyCreationDate; the members are optional, so an empty KeyLastUsage is a legal body and the recording is what fixes this behavior.
Emulator: synthesizes all four members straight from the preceding in-process call — Operation set to the last operation performed on the key ('Verify' in the recording), Timestamp set to the current clock, and CloudTrailEventId and KmsRequestId set to freshly minted uuids ($.KeyLastUsage.Operation: extra, got 'Verify'; $.KeyLastUsage.Timestamp: extra; $.KeyLastUsage.CloudTrailEventId: extra; $.KeyLastUsage.KmsRequestId: extra).
Why: a client testing the usage-tracking lag (does my key show usage yet? has tracking started?) is told usage is already recorded when AWS would report nothing, so any wait/poll loop on this surface terminates immediately and any assertion that an unused window is empty fails against AWS. The uuids and timestamp are per-run values and are NOT the requirement — the requirement is that the members are absent.
Fix: the GetKeyLastUsage handler's state projection — do not derive KeyLastUsage detail from the in-process call log; leave the members unset until a tracked usage record exists.
Traces: kms-asymmetric-sign-verify#task#6.

## Evidence
Replay of simparity-wire-aws-v1, 3 mismatch clusters, all expected_status 200 / actual_status 200:
- GetKeyPolicy, kms-key-policy-management#task#3 (4 affected steps): $.Policy value_mismatch — expected the pretty-printed document '{\n  "Version" : "2012-10-17",\n  "Id" : "key-default-1",\n  "Statement" : [ {\n    "Sid" : "Enable IAM User Permissions",\n    "Effect" : "Allow",\n    "Principal" : {\n      "AWS" : "arn:aws:iam::000000000000:root"\n    },\n    "Action" : "kms:*",\n    "Resource" : "*"\n  } ]\n}', got the compact form '{"Version":"2012-10-17","Id":"key-default-1","Statement":[{"Sid":"Enable IAM User Permissions","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"kms:*","Resource":"*"}]}'. Same object, different serialization.
- ListKeyRotations, kms-key-rotation-management#task#8: $.Rotations[0].KeyId value_mismatch — expected 'arn:aws:kms:us-west-2:000000000000:key/2c37cf0f-c677-4468-8e1c-ae929d14b1e0', got '2c37cf0f-c677-4468-8e1c-ae929d14b1e0' (ARN vs bare uuid).
- GetKeyLastUsage, kms-asymmetric-sign-verify#task#6: four members present that the recording does not carry — $.KeyLastUsage.Operation extra (got 'Verify'), $.KeyLastUsage.Timestamp extra, $.KeyLastUsage.CloudTrailEventId extra, $.KeyLastUsage.KmsRequestId extra. The recorded response's KeyLastUsage has none of them.
Smithy cross-check: GetKeyPolicyResponse.Policy's pattern admits \u0009/\u000A/\u000D so a multi-line document is modeled; RotationsListEntry.KeyId is KeyIdType (either form legal, ARN in the recording); GetKeyLastUsageResponse's KeyLastUsage members are all optional.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1.
3. kms-key-policy-management#task#3: CreateKey then GetKeyPolicy — compare the Policy string byte for byte with the recorded document; the emulator's is compact where AWS's is pretty-printed with ' : ' separators and two-space indent. Reproduces again at #task#5 and #task#7 (after PutKeyPolicy) and at kms-key-policy-special-characters-round-trip#task#3.
4. kms-key-rotation-management#task#8: ListKeyRotations on a key with rotation history — Rotations[0].KeyId is a bare uuid instead of arn:aws:kms:<region>:<account>:key/<uuid>.
5. kms-asymmetric-sign-verify#task#6: Sign/Verify on a key, then GetKeyLastUsage — the emulator fills KeyLastUsage.Operation/Timestamp/CloudTrailEventId/KmsRequestId from the call it just served, where the recorded response leaves all four unset.
