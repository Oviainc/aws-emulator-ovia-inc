Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3.bucket-policy-max-keys-condition-admits-a-capped-listing-and-refuses-a-larger-one iam__s3.get-object-version-id-denied-without-s3-get-object-version iam__s3.list-object-versions-denied-with-only-list-bucket-grant iam__s3.policy-named-user-put-object-denied-outside-granted-actions iam__s3.unnamed-user-get-object-denied-while-policy-named-user-reads iam__s3.versioned-delete-without-s3-delete-object-version-refused-while-a-plain-delete-stacks-a-marker iam__s3.identity-explicit-deny-overrides-bucket-policy-grant-denied ecs__events__iam__s3.ebecs-rule-derived-task-role-put-object-denied-while-granted-twin-writes ecs__iam__s3.task-role-get-bucket-wildcard-reads-bucket-config-but-is-denied-get-bucket-encryption iam__s3.missing-key-no-such-key-with-list-bucket-access-denied-without iam__s3.bucket-policy-group-principal-rejected-while-group-policy-grants-member iam__s3.bucket-policy-principal-list-with-one-nonexistent-user-rejected-whole iam__s3.block-public-policy-refuses-a-wildcard-source-vpc-bucket-grant-but-admits-it-as-an-identity-policy iam__s3.simulate-principal-policy-resource-policy-secure-transport-deny-overrides-the-named-grant
Issue-Key: iam__s3.bucket-policy-max-keys-condition-admits-a-capped-listing-and-refuses-a-larger-one|run#14|AssertionError|assert over == want, f"MaxKeys=100 above a cap of 10 must be refused {want!r}; got {over!r}"
Issue-Key: iam__s3.get-object-version-id-denied-without-s3-get-object-version|run#14|AssertionError|assert old == want, (
Issue-Key: iam__s3.list-object-versions-denied-with-only-list-bucket-grant|run#13|AssertionError|assert (code, message) == ("AccessDenied", expected_message), (
Issue-Key: iam__s3.policy-named-user-put-object-denied-outside-granted-actions|run#9|AssertionError|assert put == "AccessDenied", (
Issue-Key: iam__s3.unnamed-user-get-object-denied-while-policy-named-user-reads|run#16|AssertionError|assert first == "AccessDenied", (
Issue-Key: iam__s3.versioned-delete-without-s3-delete-object-version-refused-while-a-plain-delete-stacks-a-marker|run#14|AssertionError|assert refused == want, f"a delete naming a VersionId must be refused {want!r}; got {refused!r}"
Issue-Key: iam__s3.identity-explicit-deny-overrides-bucket-policy-grant-denied|_poll#9|Timeout|time.sleep(interval_s)
Issue-Key: ecs__events__iam__s3.ebecs-rule-derived-task-role-put-object-denied-while-granted-twin-writes|_refusal#29|AssertionError|raise AssertionError("the call was expected to be refused, but it succeeded")
Issue-Key: ecs__iam__s3.task-role-get-bucket-wildcard-reads-bucket-config-but-is-denied-get-bucket-encryption|_refusal#23|AssertionError|raise AssertionError("the call was expected to be refused, but it succeeded")
Issue-Key: iam__s3.missing-key-no-such-key-with-list-bucket-access-denied-without|run#16|AssertionError|assert without_list == "AccessDenied", (
Issue-Key: iam__s3.bucket-policy-group-principal-rejected-while-group-policy-grants-member|_error#14|AssertionError|raise AssertionError(f"real AWS refuses this call, but it succeeded and returned {result!r}")
Issue-Key: iam__s3.bucket-policy-principal-list-with-one-nonexistent-user-rejected-whole|_error#12|AssertionError|raise AssertionError(f"real AWS refuses this call, but it succeeded and returned {result!r}")
Issue-Key: iam__s3.block-public-policy-refuses-a-wildcard-source-vpc-bucket-grant-but-admits-it-as-an-identity-policy|_error#12|AssertionError|raise AssertionError(f"real AWS refuses this call, but it succeeded and returned {result!r}")
Issue-Key: iam__s3.simulate-principal-policy-resource-policy-secure-transport-deny-overrides-the-named-grant|_http#19|BadStatusLine|with urllib.request.urlopen(req, timeout=60) as resp:
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# s3: the data plane performs no IAM/bucket-policy authorization, PutBucketPolicy validates nothing, and one request answers with a malformed HTTP status line

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
This issue carries 6 root causes in S3. The open issues 20261029T192356Z-iam-s3-s3-getobject-is-authorized-with-neither.md, 20261027T000203Z-iam-s3-s3-getobject-performs-no-iam-authorization-a-principal.md and 20261025T000319Z-iam-s3-2-net-new-root-causes-a.md describe neighbouring symptoms but carry only other checks' replay keys; the checks below are additional and must be closed here.

Cause 1 of 6: S3 data-plane and bucket-config requests are not authorized at all — every signed request from a principal with no grant, or with a grant that does not cover the action, succeeds.
AWS: a signed request is authorized against the caller's identity policies, the bucket policy and any session policy, per action: s3:ListBucket for ListObjectsV2, s3:GetObject for GetObject, s3:GetObjectVersion for a GET naming a VersionId, s3:ListBucketVersions for ListObjectVersions, s3:PutObject for PutObject, s3:DeleteObjectVersion for a DELETE naming a VersionId (a plain DELETE needs only s3:DeleteObject and stacks a delete marker), s3:GetEncryptionConfiguration for GetBucketEncryption (a s3:GetBucket* wildcard does NOT cover it); an identity explicit Deny overrides a bucket-policy Allow; a bucket-policy condition such as s3:max-keys caps the listing it admits. A refused request is 403 AccessDenied with 'User: <caller-arn> is not authorized to perform: <s3:Action> on resource: "<resource-arn>" because no identity-based policy allows the <s3:Action> action' (fixed text; caller ARN, action and resource ARN from the request).
Emulator: all of these succeed — 200/204 with the object bytes, the version listing, the write, the delete marker and the encryption configuration — and the explicit-deny check never converges and times out at 60 s.
Why: least-privilege is unenforceable and untestable: a caller that real AWS refuses reads and writes data here, so an emulator run cannot catch a privilege escalation and cannot exercise any AccessDenied path.
Fix: the S3 request authorizer invoked by the object and bucket-config handlers — per-action permission mapping (including the version-scoped actions and s3:GetEncryptionConfiguration), identity/resource/session policy composition with deny precedence, and bucket-policy condition keys.
Traces: iam__s3.bucket-policy-max-keys-condition-admits-a-capped-listing-and-refuses-a-larger-one, iam__s3.get-object-version-id-denied-without-s3-get-object-version, iam__s3.list-object-versions-denied-with-only-list-bucket-grant, iam__s3.policy-named-user-put-object-denied-outside-granted-actions, iam__s3.unnamed-user-get-object-denied-while-policy-named-user-reads, iam__s3.versioned-delete-without-s3-delete-object-version-refused-while-a-plain-delete-stacks-a-marker, iam__s3.identity-explicit-deny-overrides-bucket-policy-grant-denied, ecs__events__iam__s3.ebecs-rule-derived-task-role-put-object-denied-while-granted-twin-writes, ecs__iam__s3.task-role-get-bucket-wildcard-reads-bucket-config-but-is-denied-get-bucket-encryption.

Cause 2 of 6: a missing key is reported as NoSuchKey to a caller that is not allowed to list the bucket.
AWS: GetObject for a key that does not exist answers 403 AccessDenied when the caller lacks s3:ListBucket, and only 404 NoSuchKey when it holds it — existence must not leak to a caller without list permission.
Emulator: answers NoSuchKey in both cases.
Why: key existence is disclosed to an unauthorized caller, and a client that distinguishes 403 from 404 to decide whether to retry or to re-authorize branches wrongly.
Fix: the GetObject handler's not-found branch must consult s3:ListBucket on the bucket before choosing NoSuchKey over AccessDenied.
Traces: iam__s3.missing-key-no-such-key-with-list-bucket-access-denied-without.

Cause 3 of 6: PutBucketPolicy accepts an IAM group ARN as a Principal.
AWS: an IAM group cannot be a resource-policy principal, so the write is refused MalformedPolicy with 'Invalid principal in policy' (both strings fixed), and the stored policy is unchanged.
Emulator: returns 204 and stores the policy.
Why: a policy AWS rejects is accepted here, so a deployment that would fail on AWS appears to succeed, and the group grant silently does nothing.
Fix: PutBucketPolicy's principal validation.
Traces: iam__s3.bucket-policy-group-principal-rejected-while-group-policy-grants-member.

Cause 4 of 6: PutBucketPolicy accepts a Principal list in which one entry does not exist.
AWS: the whole statement is refused if any principal in the list cannot be resolved — MalformedPolicy 'Invalid principal in policy' — and nothing is stored.
Emulator: returns 204 and stores the policy including the nonexistent principal.
Why: a typo'd or deleted principal is silently accepted, so the policy a caller believes it wrote is not the policy AWS would have stored.
Fix: PutBucketPolicy's principal resolution must validate every entry of a Principal list and reject the write as a whole.
Traces: iam__s3.bucket-policy-principal-list-with-one-nonexistent-user-rejected-whole.

Cause 5 of 6: BlockPublicPolicy does not refuse a public bucket policy.
AWS: with BlockPublicPolicy on, a grant whose only restriction is a wildcard-valued condition (StringLike aws:SourceVpc = 'vpc-*') still counts as public and PutBucketPolicy is refused 403 AccessDenied with 'User: <caller-arn> is not authorized to perform: s3:PutBucketPolicy on resource: "<bucket-arn>" because public policies are prevented by the BlockPublicPolicy setting in S3 Block Public Access.' (fixed text; caller ARN and bucket ARN from the request). A grant scoped to one fixed aws:SourceVpc value is not public and is accepted.
Emulator: returns 204 and stores the public policy.
Why: Block Public Access gives no protection — a policy AWS refuses is stored, so the emulator cannot be used to validate a public-access guardrail.
Fix: the public-policy evaluation behind PutBucketPolicy for the bucket's PublicAccessBlockConfiguration.
Traces: iam__s3.block-public-policy-refuses-a-wildcard-source-vpc-bucket-grant-but-admits-it-as-an-identity-policy.

Cause 6 of 6: one request against the S3 endpoint answers with something that is not an HTTP response.
AWS: every request produces a parseable HTTP response (a status line, headers and a body) whether it is allowed or refused.
Emulator: the client fails in http.client with 'BadStatusLine:  2' — the bytes returned on the connection do not start with an HTTP status line, so no status, code or body can be read. Recorded behavior only; the check was exercising a resource-policy aws:SecureTransport deny over the named grant. Hypothesis: the endpoint served non-HTTP bytes (or plaintext where the announced endpoint implies TLS) on that path.
Why: a client cannot parse the answer at all — this is worse than a wrong status, because every SDK surfaces it as a transport failure rather than an AWS error.
Fix: the S3 front end must always emit a well-formed HTTP response on this path.
Traces: iam__s3.simulate-principal-policy-resource-policy-secure-transport-deny-overrides-the-named-grant.

## Evidence
Cause 1: 'the bucket policy does not name this user and it holds no identity policy, so its signed GetObject must be refused with exactly AccessDenied; got OK'; 'ListObjectVersions needs s3:ListBucketVersions, which neither the identity nor the bucket policy grants; got OK' with the full Versions list; "a delete naming a VersionId must be refused ('AccessDenied', 'User: ... is not authorized to perform: s3:DeleteObjectVersion on resource: \"arn:aws:s3:::<bucket>/test-data.json\" because no identity-based policy allows the s3:DeleteObjectVersion action'); got ('OK', {... 'VersionId': ...})"; 'MaxKeys=100 above a cap of 10 must be refused (AccessDenied, ...); got (OK, [test-data.json])'; 'the call was expected to be refused, but it succeeded' for GetBucketEncryption and PutObject.
Cause 2: 'a caller WITHOUT s3:ListBucket must not learn whether the key exists: expected exactly AccessDenied, got NoSuchKey'.
Causes 3-5: 'real AWS refuses this call, but it succeeded and returned {... HTTPStatusCode: 204 ...}' for each PutBucketPolicy; the required answers are ('MalformedPolicy', 'Invalid principal in policy') and ('AccessDenied', 'User: ... because public policies are prevented by the BlockPublicPolicy setting in S3 Block Public Access.').
Cause 6: 'File "client.py", line 325, in _read_status / raise BadStatusLine(line) / http.client.BadStatusLine:  2'.

## Steps to Reproduce
1. Cause 1: create a bucket with a policy naming user A only; sign requests as user B (no identity policy) and as a user whose grant omits the action, and observe GetObject / PutObject / ListObjectsV2 with MaxKeys above an s3:max-keys cap / ListObjectVersions / a DELETE naming a VersionId / GetBucketEncryption all succeed.
2. Cause 2: GET a nonexistent key as a caller without s3:ListBucket.
3. Causes 3-5: PutBucketPolicy with an IAM group ARN principal; with a principal list holding one nonexistent user ARN; and with StringLike aws:SourceVpc='vpc-*' on a bucket with BlockPublicPolicy on.
4. Or replay the check ids in trace_ids.
