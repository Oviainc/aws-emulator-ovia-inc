Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-policy-invalid-document-rejected kafka.cluster-policy-sequential-overwrite-latest-wins kafka.cluster-policy-version-mismatch-rejected kafka.cluster-policy-concurrent-optimistic-lock kafka.cluster-policy-json-unicode-round-trip kafka.cluster-policy-large-document-round-trip
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-policy-invalid-document-rejected|_error_code#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: kafka.cluster-policy-sequential-overwrite-latest-wins|_error_code#14|AssertionError|raise AssertionError(f"expected a ClientError, call unexpectedly succeeded with {result!r}")
Issue-Key: kafka.cluster-policy-version-mismatch-rejected|run#10|AssertionError|assert message == "One or more of the parameters are not valid.", message
Issue-Key: kafka.cluster-policy-concurrent-optimistic-lock|run#15|AssertionError|assert all(m == "The version of the cluster policy isn\u2019t current. Check the current version and try again." for m in error_messages), error_messages
Issue-Key: kafka.cluster-policy-json-unicode-round-trip|run#11|AssertionError|assert resp["Policy"] == expected, (
Issue-Key: kafka.cluster-policy-large-document-round-trip|run#9|AssertionError|assert fetched["Policy"] == expected, (
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# Cluster-policy surface: PutClusterPolicy validates nothing and ignores the optimistic lock, and GetClusterPolicy echoes the submitted document instead of MSK's canonical form (4 causes)

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutClusterPolicy

## Description
FOUR distinct root causes on the MSK cluster-policy surface. Each lives in a different part of the PutClusterPolicy/GetClusterPolicy path and must be closed separately; all four must be closed before this issue is submitted.

Cause 1 of 4: PutClusterPolicy accepts a policy document that is not a valid resource policy.
AWS: PutClusterPolicy (PUT /v1/clusters/{ClusterArn}/policy, 200) models BadRequestException/400 (Smithy: errors BadRequestException/400, ForbiddenException/403, InternalServerErrorException/500; input Policy required, CurrentVersion optional). MSK parses and validates the document: a statement missing the required 'Effect' field is refused with BadRequestException and the message 'The cluster policy is not valid. Missing required field Effect' — fixed text naming the missing field; the offending document is not echoed. After the refusal no policy is attached, so GetClusterPolicy answers NotFoundException with 'The requested resource doesn\u2019t exist.'
Emulator: HTTP 200 with a minted CurrentVersion; the invalid document is stored and GetClusterPolicy returns it.
Why: a caller that mis-renders a policy is told it is live. Access decisions that depend on the policy then differ between emulator and AWS, and policy-linting tests that rely on the service's own validation pass vacuously.
Fix: PutClusterPolicy's handler must validate the policy document (JSON shape plus required statement fields) before storing it, and raise the BadRequestException above.
Traces: kafka.cluster-policy-invalid-document-rejected.

Cause 2 of 4: PutClusterPolicy omitting CurrentVersion against an EXISTING policy is accepted and overwrites it.
AWS: once a policy exists, the update is version-gated: PutClusterPolicy without CurrentVersion is refused with BadRequestException and the fixed message 'Current cluster policy version needed for Update'; the stored policy and its CurrentVersion are left untouched. Supplying the prior version replaces the document and advances the version.
Emulator: HTTP 200 with a new CurrentVersion ({'CurrentVersion': '<new version>'}); the second document replaces the first with no version supplied.
Why: the optimistic lock is the only protection against two writers clobbering each other's policy. A client that relies on the refusal to detect a lost update silently overwrites a policy it never read, and a compare-and-set loop never sees the conflict.
Fix: PutClusterPolicy's handler must require CurrentVersion whenever a policy already exists for the cluster, and refuse the version-less call before mutating state.
Traces: kafka.cluster-policy-sequential-overwrite-latest-wins.

Cause 3 of 4: the version-mismatch refusal carries the emulator's own sentence and leaks the stored version.
AWS: PutClusterPolicy with a CurrentVersion that is not the stored one is refused with BadRequestException and the fixed message 'The version of the cluster policy isn\u2019t current. Check the current version and try again.' The whole sentence is fixed text: neither the supplied nor the stored version appears in it.
Emulator: BadRequestException with "The cluster policy's current version is <stored version>, not <supplied version>." — status and code right, message is emulator prose that additionally interpolates two values AWS keeps out of the payload, so it varies run to run where AWS's is byte-identical.
Why: clients and suites that classify or assert on MSK's conflict sentence stop matching; retry logic keyed on that text never fires; and a golden-file assertion on the message can never be stable because the emulator embeds generated version ids.
Fix: the version-mismatch refusal raised by PutClusterPolicy (and the same constant used on the concurrent path) must emit AWS's fixed sentence from the service's message constants.
Traces: kafka.cluster-policy-version-mismatch-rejected, kafka.cluster-policy-concurrent-optimistic-lock (four concurrent losers all carried the emulator sentence).

Cause 4 of 4: GetClusterPolicy returns the document byte-for-byte as submitted instead of MSK's canonical compact JSON.
AWS: recorded behavior only (the model types Policy as a string). GetClusterPolicy serves the policy canonicalized to COMPACT JSON — no insignificant whitespace, no newlines or indentation — while preserving every value, key order, unicode content (e.g. 'h\u00e9llo-\u4e16\u754c-\u0394\u03bf\u03ba\u03b9\u03bc\u03ae') and escape sequence ('\\t', '\\n') exactly as given, and keeping every statement intact and in order for a large multi-statement document.
Emulator: the stored text is echoed verbatim, including the pretty-printed indentation and newlines the caller submitted ('{\n\t"Version": "2012-10-17",\n\t"Statement": [...').
Why: a client that compares the retrieved policy with a canonical form it computed (a drift detector, a CloudFormation/Terraform-style diff, a hash of the policy) reports a spurious change on every read, and one that assumes compact JSON mis-measures the document against MSK's policy size limit.
Fix: the cluster-policy state/serializer must canonicalize the document on store or on read in GetClusterPolicy, rather than returning the raw request text.
Traces: kafka.cluster-policy-json-unicode-round-trip, kafka.cluster-policy-large-document-round-trip.

## Evidence
All six checks are kafka behavioral checks with verdict mismatch; none is marked ALREADY FILED or OWNED.

Cause 1 — kafka.cluster-policy-invalid-document-rejected: PutClusterPolicy with a statement missing 'Effect'.
  Expected: ('BadRequestException', 'The cluster policy is not valid. Missing required field Effect'), then GetClusterPolicy -> ('NotFoundException', 'The requested resource doesn\u2019t exist.').
  Actual: HTTP 200 {'CurrentVersion': 'KP6SU0D26SG050'} (version value is per-run).

Cause 2 — kafka.cluster-policy-sequential-overwrite-latest-wins: policy_one put, then PutClusterPolicy(policy_two) with NO CurrentVersion.
  Expected: ('BadRequestException', 'Current cluster policy version needed for Update') and the stored policy/version unchanged.
  Actual: HTTP 200 {'CurrentVersion': 'K3GRKSNNC7ZMJN'}.

Cause 3 — kafka.cluster-policy-version-mismatch-rejected and kafka.cluster-policy-concurrent-optimistic-lock.
  Expected message: 'The version of the cluster policy isn\u2019t current. Check the current version and try again.'
  Actual messages: "The cluster policy's current version is KNB4MPTT59KVY2, not K0THISVERSIONISWRONG." and, on the concurrent check, four identical "The cluster policy's current version is KG85QU40XTYQ8D, not KO6HBOFEXHQ4ZK." The version strings are this run's values; the requirement is the fixed sentence with no values in it.

Cause 4 — kafka.cluster-policy-json-unicode-round-trip:
  Expected: '{"Version":"2012-10-17","Statement":[{"Sid":"UnicodeAccess","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":"kafka:GetBootstrapBrokers","Resource":"<cluster arn>","Condition":{"StringLike":{"aws:userid":"h\u00e9llo-\u4e16\u754c-\u0394\u03bf\u03ba\u03b9\u03bc\u03ae  test\\t\\n"}}}]}' (compact).
  Actual: the same content pretty-printed with tabs and newlines as submitted.
  kafka.cluster-policy-large-document-round-trip shows the same divergence on a four-statement document (expected compact, got three-space-indented multi-line text).

Smithy cited: PutClusterPolicy PUT /v1/clusters/{ClusterArn}/policy code 200; input ClusterArn (httpLabel, required), CurrentVersion (optional, jsonName currentVersion), Policy (required, jsonName policy); errors BadRequestException (client, 400), ForbiddenException (403), InternalServerErrorException (500). Output carries only CurrentVersion.

## Steps to Reproduce
1. Boot the emulator, create a PROVISIONED cluster and wait for ACTIVE.
2. Cause 1: PutClusterPolicy with a statement that omits 'Effect'. Assert 400 BadRequestException 'The cluster policy is not valid. Missing required field Effect', then GetClusterPolicy and assert 404 NotFoundException 'The requested resource doesn\u2019t exist.' Then put a well-formed policy and assert it attaches (positive control).
3. Cause 2: with a policy attached, call PutClusterPolicy with a DIFFERENT document and no CurrentVersion. Assert 400 BadRequestException 'Current cluster policy version needed for Update', then GetClusterPolicy and assert both Policy and CurrentVersion are unchanged. Repeat with the correct CurrentVersion and assert the document is replaced and the version advances.
4. Cause 3: call PutClusterPolicy with CurrentVersion='K0THISVERSIONISWRONG'. Assert the Message is exactly 'The version of the cluster policy isn\u2019t current. Check the current version and try again.' and that neither the supplied nor the stored version appears in it. Repeat from four threads sharing one stale version and assert every loser carries that same sentence.
5. Cause 4: PutClusterPolicy with a pretty-printed document containing unicode and \\t/\\n escapes, then GetClusterPolicy. Assert the returned Policy is the compact JSON form (no indentation or newlines) with values, unicode, escapes and key order preserved; repeat with a four-statement document and assert statement order and content are intact.
6. Replay kafka.cluster-policy-invalid-document-rejected, kafka.cluster-policy-sequential-overwrite-latest-wins, kafka.cluster-policy-version-mismatch-rejected, kafka.cluster-policy-concurrent-optimistic-lock, kafka.cluster-policy-json-unicode-round-trip and kafka.cluster-policy-large-document-round-trip.
