Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-pab-ignore-public-acls-denies-anonymous-get-of-a-public-read-object s3.s3-pab-ignore-public-acls-denies-anonymous-list-of-a-public-read-bucket s3.s3-pab-restrict-public-buckets-denies-anonymous-get-through-a-public-policy s3.deep-s3-immutability-requester-pays-anonymous-with-payer-header-rejected s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes s3.s3-website-hosting-get-rejected-for-a-bucket-never-configured s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys s3.s3-sse-kms-object-is-unreadable-while-its-cmk-is-disabled-and-readable-again-after s3.s3-old-buckets-regional-view-advances-creation-date-on-a-config-write s3.s3-old-buckets-object-writes-do-not-move-creation-date s3.notification-filter-non-matching-key-not-delivered
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-pab-ignore-public-acls-denies-anonymous-get-of-a-public-read-object|_poll#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: s3.s3-pab-ignore-public-acls-denies-anonymous-list-of-a-public-read-bucket|_poll#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: s3.s3-pab-restrict-public-buckets-denies-anonymous-get-through-a-public-policy|_poll#13|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: s3.deep-s3-immutability-requester-pays-anonymous-with-payer-header-rejected|_poll#9|AssertionError|assert time.monotonic() < deadline, (
Issue-Key: s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes|_await_http#15|AssertionError|raise AssertionError(
Issue-Key: s3.s3-website-hosting-get-rejected-for-a-bucket-never-configured|_await_http#11|AssertionError|raise AssertionError(
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys|_await#10|AssertionError|assert last[0] == want, (
Issue-Key: s3.s3-sse-kms-object-is-unreadable-while-its-cmk-is-disabled-and-readable-again-after|run#22|AssertionError|assert denied is not None, (
Issue-Key: s3.s3-old-buckets-regional-view-advances-creation-date-on-a-config-write|_poll#11|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: s3.s3-old-buckets-object-writes-do-not-move-creation-date|_poll#14|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: s3.notification-filter-non-matching-key-not-delivered|_drain_records#20|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# 4 net-new causes: anonymous and website-endpoint requests are always AccessDenied, SSE-KMS ignores CMK state, the regional CreationDate never advances, and notification checks never converge

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Four root causes, all grounded in failing checks this report does NOT mark ALREADY FILED or OWNED. Several open s3 issues (e.g. 20261103T040901Z-s3-10-net-new-root-causes-bucket-policy.md, 20261103T040901Z-s3-10-more-net-new-root-causes-sse.md, 20261028T105230Z-s3-8-net-new-root-causes-in-the.md) own OTHER checks of this service and do not carry the replay keys below.

Cause 1 of 4: unsigned (anonymous) requests are refused unconditionally, so public-read ACLs/policies and the website endpoint never serve anything.
AWS: a request with no Authorization header is evaluated as the anonymous principal. A public-read object ACL, or a bucket policy granting s3:GetObject to Principal "*", makes an unsigned GET return 200 with the bytes; a public-read bucket ACL makes an unsigned ListObjects return the listing. Public Access Block then changes that answer, and that is the behaviour under test: with IgnorePublicAcls=true the unsigned GET/LIST is refused 403 AccessDenied, and clearing the flag restores the 200; with RestrictPublicBuckets=true an unsigned GET admitted only by a public policy is refused 403, and clearing it restores the 200. The S3 website endpoint likewise serves unsigned GETs — 200 with the index document, the error document with its own status, and a 301 for a matching RoutingRule — and refuses writes 405.
Emulator: every unsigned request is refused 403 AccessDenied regardless of ACL, bucket policy or PAB state: the three PAB checks poll 120 s for the 200 that must follow clearing the flag and keep seeing 'AccessDenied'; the requester-pays check polls 60 s and sees ('AccessDenied', 'Access Denied'); the website-endpoint checks poll 90 s and keep getting HTTP 403 with body <Error><Code>AccessDenied</Code><Message>Access Denied</Message>... (and, on the REST endpoint, the variant <Message>Sign the request with the emulator credentials</Message>, which is the emulator's own text and names the real cause). The front door requires SigV4 credentials before any authorization is evaluated.
Why: this is a security-control inversion. Every Public-Access-Block, public-ACL, public-policy and website-hosting behaviour is unobservable: the flag that is supposed to block public access cannot be shown to block anything because nothing public ever worked, so a client validating that its buckets are not public gets a meaningless pass, and no static website or public download can be served at all.
Fix: the S3 front door's authentication/authorization split — an unsigned request must be admitted as the anonymous principal and then evaluated against the object/bucket ACL, the bucket policy and the Public Access Block configuration; plus the s3-website endpoint handler (index/error document, RoutingRules, 405 on writes).
Traces: s3.s3-pab-ignore-public-acls-denies-anonymous-get-of-a-public-read-object, .s3-pab-ignore-public-acls-denies-anonymous-list-of-a-public-read-bucket, .s3-pab-restrict-public-buckets-denies-anonymous-get-through-a-public-policy, .deep-s3-immutability-requester-pays-anonymous-with-payer-header-rejected, .website-endpoint-serves-index-and-error-documents-and-refuses-writes, .s3-website-hosting-get-rejected-for-a-bucket-never-configured, .s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys.

Cause 2 of 4: an SSE-KMS object stays readable while its CMK is disabled.
AWS: GetObject of an SSE-KMS object calls kms:Decrypt; while the CMK is Disabled the read is refused 400 KMS.DisabledException (message naming the key as disabled), and it succeeds again after EnableKey.
Emulator: "GetObject still returned the plaintext 120s after the CMK was disabled — the object is not actually encrypted with that key, so SSE-KMS is a stored header rather than cryptography."
Why: disabling or scheduling deletion of a CMK does not make the data it 'protects' inaccessible, so crypto-shredding and key-compromise response are untestable and a client's encryption boundary is fiction.
Fix: the S3 object read path's KMS integration (call Decrypt with the object's encryption context and propagate KMS's refusal).
Traces: s3.s3-sse-kms-object-is-unreadable-while-its-cmk-is-disabled-and-readable-again-after.

Cause 3 of 4: the regional view of a bucket's CreationDate never advances on a configuration write.
AWS: ListBuckets served from the bucket's own Region reports a CreationDate that advances when the bucket's configuration is written (the documented regional-view behaviour), while an object write alone does not move it — the contrast between the two is the behaviour under test.
Emulator: the date never moves. Both checks poll 90 s for the advance after a tag write and report "the us-west-2 view of <bucket>'s CreationDate moves after a tag write not met within 90.0s" / "the sibling's regional date moves after its tag write (barrier) not met within 90.0s", with the same timestamp observed throughout. Because the barrier never fires, the companion assertion (that an object write does NOT move it) is also unfalsifiable.
Why: a client that uses the regional CreationDate to detect configuration changes sees a frozen value, and cannot distinguish the regional view from the global one.
Fix: the S3 bucket metadata/ListBuckets projection — the regional CreationDate must be updated by bucket-configuration writes and left alone by object writes.
Traces: s3.s3-old-buckets-regional-view-advances-creation-date-on-a-config-write, .s3-old-buckets-object-writes-do-not-move-creation-date.

Cause 4 of 4: a notification filter check never converges.
AWS: with a prefix/suffix FilterRule on a notification configuration, a matching key is delivered to the destination within seconds and a non-matching key is not — the matching delivery is the calibration that makes the absence meaningful.
Emulator: the check never reaches its assertion and ends "Error: Check timed out after 180.0s": no delivery is ever observed, so neither the match nor the non-match can be measured. recorded behavior only.
Why: notification filtering cannot be verified at all, and a client reading a passing absence would be reading a dead pipeline.
Fix: the S3 notification dispatcher (event generation, FilterRule matching and destination delivery).
Traces: s3.notification-filter-non-matching-key-not-delivered.

## Evidence
C1: expected an unsigned GET/LIST to return 200 once the PAB flag is cleared (and the website endpoint to serve its index document); got 403 AccessDenied every time — 'an unsigned GET succeeds once IgnorePublicAcls is cleared not met within 120.0s. Last result: AccessDenied', and 'never returned 200 within 90s; last answer was HTTP 403 with body <Error><Code>AccessDenied</Code><Message>Access Denied</Message>...'; the REST variant says <Message>Sign the request with the emulator credentials</Message>.
C2: expected 400 KMS.DisabledException while the CMK is disabled; got the plaintext after 120s.
C3: expected the regional CreationDate to advance after a bucket tag write; it never moved within 90s.
C4: expected a delivered record for the matching key and none for the non-matching key; got no delivery — 'Error: Check timed out after 180.0s'.

## Steps to Reproduce
1) C1: put a public-read object and a bucket policy granting s3:GetObject to Principal '*'; GET/LIST with no Authorization header; toggle IgnorePublicAcls and RestrictPublicBuckets; configure a website with index/error documents and a RoutingRule and GET the s3-website host unsigned.
2) C2: write an SSE-KMS object, DisableKey, GetObject; EnableKey, GetObject.
3) C3: ListBuckets from the bucket's own Region, PutBucketTagging, ListBuckets again; repeat with only a PutObject.
4) C4: PutBucketNotificationConfiguration with a prefix FilterRule, PutObject a matching and a non-matching key, and watch the destination.
