Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.copy-object-to-itself-no-change-rejected s3.deep-s3-immutability-copy-metadata-directive-replace-drops-unrestated s3.deep-s3-storage-class-copy-in-place-to-deep-archive-closes-reads s3.s3-concurrent-copy-while-the-source-is-being-deleted-never-yields-a-partial-object s3.s3-itc-e2e-numbered-buckets-missing-tiering-lifecycle s3__sqs.inline-tagging-and-canned-acl-on-write-withhold-tagging-and-acl-events
Issue-Key: s3.copy-object-to-itself-no-change-rejected|run#6|AssertionError|assert refusal == _SELF_COPY_REFUSED, (
Issue-Key: s3.deep-s3-immutability-copy-metadata-directive-replace-drops-unrestated|run#13|AssertionError|assert h["ContentType"] == "binary/octet-stream", (
Issue-Key: s3.deep-s3-storage-class-copy-in-place-to-deep-archive-closes-reads|_error#17|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: s3.s3-concurrent-copy-while-the-source-is-being-deleted-never-yields-a-partial-object|run#13|AssertionError|assert code == ("NoSuchKey", "The specified key does not exist."), (
Issue-Key: s3.s3-itc-e2e-numbered-buckets-missing-tiering-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: s3__sqs.inline-tagging-and-canned-acl-on-write-withhold-tagging-and-acl-events|_wait_for_records#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 67bdaa95f371a43447a945b4dd8bda66772a6b0f

# S3 object plane: 6 net-new root causes — self-copy refusal rule not implemented, MetadataDirective=REPLACE keeps unrestated system metadata, DEEP_ARCHIVE reads not closed, a synthetic 16 MiB per-bucket cap refuses copies, CFN-declared Intelligent-Tiering never listed, and inline-tagging/ACL writes publish no creation event

> _Note: this issue was identified against the code base at commit `67bdaa95f371a43447a945b4dd8bda66772a6b0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyObject

## Description
Six net-new root causes in the emulator's S3 object plane and bucket-subresource plane, each grounded in its own failing behavioral check. None of these checks is marked ALREADY FILED or OWNED in the replay report. Related open issues exist for neighbouring S3 clusters (20261025T032458Z-s3-object-plane-10-root-causes-conditional-requests.md, 20261025T032458Z-s3-control-plane-10-root-causes-generic-validationexception.md, 20261025T032458Z-s3-request-auth-and-eventing-plane-10-root.md, 20261025T032458Z-s3-remaining-divergences-8-root-causes-selectobjectcontent-routing.md) but none of them carries these replay keys.

=== Cause 1 of 6: CopyObject does not implement the self-copy-without-change refusal rule ===
AWS: CopyObject (Smithy: s3#CopyObject, PUT /{Bucket}/{Key+}, input members MetadataDirective/TaggingDirective/StorageClass/WebsiteRedirectLocation/ServerSideEncryption* all optional headers). AWS documents that a copy whose source and destination are the same key and which changes none of the object's metadata, storage class, website redirect location or encryption attributes is refused with HTTP 400 and error code InvalidRequest, and the message states that rule verbatim: "This copy request is illegal because it is trying to copy an object to itself without changing the object's metadata, storage class, website redirect location or encryption attributes." The rule that produces the message is documented and computed from the request, so the message is a requirement, not an example.
Emulator: the call is refused with code InvalidRequest but the message is the generic validation fallback "A parameter or header in your request isn't valid. For details, see the description of this API operation." — i.e. the self-copy condition is never recognised; the request falls through to generic header validation.
Why: a client that distinguishes an illegal self-copy from a malformed request (a sync/metadata-refresh tool that retries after adding MetadataDirective=REPLACE) cannot tell the two apart and will treat a legitimate, correctable self-copy as a bad request.
Fix: CopyObject's validation in the S3 service crate (the copy_object operation handler) must detect source==destination with no change among metadata, storage class, website redirect location and encryption attributes, and emit the documented InvalidRequest refusal before generic header validation.
Traces: check s3.copy-object-to-itself-no-change-rejected

=== Cause 2 of 6: CopyObject MetadataDirective=REPLACE does not reset unrestated system metadata ===
AWS: CopyObject with MetadataDirective=REPLACE replaces the destination's whole user-and-system metadata set from the request. System metadata headers the request does not restate are therefore NOT carried over from the source; they return to their defaults, so an object whose source Content-Type was application/xml comes back as the S3 default binary/octet-stream when REPLACE is used without a Content-Type header. (Smithy: CopyObject.MetadataDirective -> MetadataDirective enum, CopyObject.ContentType -> Content-Type header, both optional.)
Emulator: after a REPLACE copy that restates no Content-Type, HeadObject on the destination reports ContentType 'application/xml' — the source's value — instead of the default binary/octet-stream. Unrestated system metadata is being inherited under REPLACE, i.e. REPLACE is behaving like COPY for those members.
Why: callers use REPLACE precisely to clear stale metadata (content type, cache control, content disposition, content encoding, content language) in place. Under the emulator the clear silently does nothing, so a consumer that keys off Content-Type keeps serving the wrong type forever.
Fix: the copy_object handler's metadata-directive branch: under REPLACE, build the destination's system+user metadata ONLY from the request, defaulting absent members, rather than merging over the source object's metadata.
Traces: check s3.deep-s3-immutability-copy-metadata-directive-replace-drops-unrestated

=== Cause 3 of 6: GetObject on a DEEP_ARCHIVE object succeeds instead of being refused InvalidObjectState ===
AWS: GetObject models InvalidObjectState as a client error with HTTP 403 (Smithy: s3#GetObject errors = [InvalidObjectState (403), NoSuchKey (404)]). An object in an archival storage class (GLACIER / DEEP_ARCHIVE) is not retrievable until it is restored, so GetObject is refused 403 InvalidObjectState; copying such a source out is likewise refused InvalidObjectState. The recording's expected refusal is ('InvalidObjectState', "The operation is not valid for the object's storage class").
Emulator: after an in-place CopyObject that changes only StorageClass to DEEP_ARCHIVE (the header round-trips — the response carries x-amz-storage-class: DEEP_ARCHIVE), GetObject on the same key answers HTTP 200 and streams the body. The storage class is stored as a label only; it does not gate the data plane.
Why: archival lifecycle code is untestable and silently wrong against the emulator — a client never sees the InvalidObjectState it must handle by issuing RestoreObject, and a cost/retrieval model built on "archived objects are unreadable" is contradicted.
Fix: the S3 object read path (get_object, and the copy source resolution in copy_object) must consult the stored object's storage class and refuse with 403 InvalidObjectState for GLACIER/DEEP_ARCHIVE objects that have no active restore.
Traces: check s3.deep-s3-storage-class-copy-in-place-to-deep-archive-closes-reads

=== Cause 4 of 6: an emulator-internal per-bucket capacity cap is surfaced as an AWS error on CopyObject ===
AWS: recorded behavior only. S3 imposes no per-bucket byte quota; a CopyObject whose source is concurrently deleted may only fail with 404 NoSuchKey ("The specified key does not exist."), and otherwise succeeds whole — there is no partial object and no capacity refusal. ValidationException is not a modeled S3 error at all (s3#CopyObject errors = [ObjectNotInActiveTierError]).
Emulator: eight concurrent 10 MiB copies against one bucket are refused with ('ValidationException', 'copying "race-dest-0.bin" would leave the bucket "<bucket>" holding 20971616 bytes of objects, past the 16777216 bytes this emulator keeps per bucket'). An internal 16 MiB-per-bucket storage limit is leaking to the wire as an unmodeled error code, and it names the emulator in the message.
Why: any client working with objects of realistic size hits a refusal that no AWS SDK models and no retry policy understands; the failure is also non-deterministic with respect to request ordering, so it corrupts concurrency tests.
Fix: the S3 object store/state layer that enforces the per-bucket byte budget. Either raise/remove the budget for the object plane or, if a budget must remain, it must not be reported as an AWS service error on a request AWS would accept.
Traces: check s3.s3-concurrent-copy-while-the-source-is-being-deleted-never-yields-a-partial-object

=== Cause 5 of 6: a bucket's Intelligent-Tiering configuration declared on the bucket resource is never listed ===
AWS: ListBucketIntelligentTieringConfigurations (Smithy: GET /{Bucket}?intelligent-tiering, output member IntelligentTieringConfigurationList, xmlFlattened, no modeled errors) returns every configuration set on the bucket, including ones created as a property of the bucket at provisioning time. A flat Prefix + TagFilters pair must surface as a single Filter with an And block holding Prefix and Tags.
Emulator: for a provisioned bucket whose Intelligent-Tiering configuration declares Id 'MyIntelligentTieringConfig', Filter {And: {Prefix: 'documents/', Tags: [{department: finance}]}}, Status Enabled and Tierings [{90, ARCHIVE_ACCESS}, {180, DEEP_ARCHIVE_ACCESS}], the list answers with no configuration at all (the check reports 'bucket1 lists None').
Why: a consumer that reads back its tiering policy to verify or to compute expected transitions sees an unconfigured bucket, so the policy appears lost.
Fix: the S3 bucket-subresource state for intelligent-tiering must be populated when the configuration arrives as part of bucket provisioning, and ListBucketIntelligentTieringConfigurations must serialise it with the And-wrapped Filter when both Prefix and TagFilters are present. Note a separate ALREADY-FILED issue covers GetBucketIntelligentTieringConfiguration being an unimplemented stub; this cause is the provisioning-to-list path, not that stub.
Why this is listed here: the check id's service token is s3.
Traces: check s3.s3-itc-e2e-numbered-buckets-missing-tiering-lifecycle

=== Cause 6 of 6: a PutObject carrying inline tagging and a canned ACL publishes no ObjectCreated event ===
AWS: recorded behavior only (measured 2026-10-01, us-west-2). PutObject with x-amz-tagging and a canned x-amz-acl, and CopyObject with TaggingDirective=REPLACE plus a canned ACL, each produce exactly ONE notification — s3:ObjectCreated:Put / s3:ObjectCreated:Copy — and no s3:ObjectTagging:Put and no s3:ObjectAcl:Put, because the tags and ACL are part of the creation rather than separate sub-resource writes.
Emulator: the check never reaches its tagging/ACL assertions: it hangs to the harness cap and is reported 'Check timed out after 60.0s' while waiting on the control queue subscribed to s3:ObjectCreated:Put, which bounds the hold on the other queues. Hypothesis: the creation notification is never delivered at all, so the differential barrier never opens. The related open issues 20261025T002357Z-events-s3-sqs-s3-produces-no-aws-s3-events-on-either.md and 20261025T032458Z-s3-sqs-s3-notification-destinations-are-never-validated.md describe the same emission gap for other groups but do not carry this check's replay key.
Why: a consumer wired to ObjectCreated never learns about objects written with inline tags/ACL, and cannot observe the AWS guarantee that such a write does NOT also fire tagging/ACL events (so an emulator that later derives events from "which sub-resources changed" would over-deliver without being caught).
Fix: the S3 notification emission path for PutObject/CopyObject — a write carrying x-amz-tagging and/or a canned ACL must emit exactly one ObjectCreated:{Put,Copy} event to every matching configuration and must not emit ObjectTagging:Put or ObjectAcl:Put.
Traces: check s3__sqs.inline-tagging-and-canned-acl-on-write-withhold-tagging-and-acl-events

## Evidence
Cause 1 — s3.copy-object-to-itself-no-change-rejected:
  expected ('InvalidRequest', "This copy request is illegal because it is trying to copy an object to itself without changing the object's metadata, storage class, website redirect location or encryption attributes.")
  actual   ('InvalidRequest', "A parameter or header in your request isn't valid. For details, see the description of this API operation.")

Cause 2 — s3.deep-s3-immutability-copy-metadata-directive-replace-drops-unrestated:
  AssertionError: REPLACE without a ContentType left ContentType 'application/xml'; S3 resets it to its default 'binary/octet-stream'

Cause 3 — s3.deep-s3-storage-class-copy-in-place-to-deep-archive-closes-reads:
  expected ClientError ('InvalidObjectState', "The operation is not valid for the object's storage class") / HTTP 403
  actual   HTTP 200 with headers {'x-amz-storage-class': 'DEEP_ARCHIVE', 'content-length': '34', 'etag': '"11abd5a835173dadc057fae7a9e7178d"'} and a streamed Body

Cause 4 — s3.s3-concurrent-copy-while-the-source-is-being-deleted-never-yields-a-partial-object:
  expected the only legal failure ('NoSuchKey', 'The specified key does not exist.')
  actual   ('ValidationException', 'copying "race-dest-0.bin" would leave the bucket "sim2real-behav-bkt-920fd2bfc6dc" holding 20971616 bytes of objects, past the 16777216 bytes this emulator keeps per bucket')

Cause 5 — s3.s3-itc-e2e-numbered-buckets-missing-tiering-lifecycle:
  expected ListBucketIntelligentTieringConfigurations to surface {'Id': 'MyIntelligentTieringConfig', 'Filter': {'And': {'Prefix': 'documents/', 'Tags': [{'Key': 'department', 'Value': 'finance'}]}}, 'Status': 'Enabled', 'Tierings': [{'Days': 90, 'AccessTier': 'ARCHIVE_ACCESS'}, {'Days': 180, 'AccessTier': 'DEEP_ARCHIVE_ACCESS'}]}
  actual   'bucket1 lists None'

Cause 6 — s3__sqs.inline-tagging-and-canned-acl-on-write-withhold-tagging-and-acl-events:
  expected one ObjectCreated:Put on the control queue and one ObjectCreated:Copy on the family queue, with the tagging and ACL queues staying empty, then a standalone PutObjectTagging/PutObjectAcl reaching those same queues
  actual   'Check timed out after 60.0s' — the run never gets past the wait on the ObjectCreated control queue

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these check ids:
  s3.copy-object-to-itself-no-change-rejected
  s3.deep-s3-immutability-copy-metadata-directive-replace-drops-unrestated
  s3.deep-s3-storage-class-copy-in-place-to-deep-archive-closes-reads
  s3.s3-concurrent-copy-while-the-source-is-being-deleted-never-yields-a-partial-object
  s3.s3-itc-e2e-numbered-buckets-missing-tiering-lifecycle
  s3__sqs.inline-tagging-and-canned-acl-on-write-withhold-tagging-and-acl-events

Minimal reproductions:
1. CreateBucket; PutObject key; CopyObject with CopySource=<bucket>/<key>, Bucket=<bucket>, Key=<key> and no directive/storage-class/redirect/encryption change -> expect 400 InvalidRequest with the documented self-copy message.
2. PutObject key with ContentType application/xml and user metadata; CopyObject onto a second key with MetadataDirective=REPLACE and NO Content-Type header; HeadObject the destination -> expect ContentType binary/octet-stream.
3. PutObject key (STANDARD); GetObject -> 200; CopyObject onto the same key with StorageClass=DEEP_ARCHIVE; GetObject -> expect 403 InvalidObjectState; CopyObject out to STANDARD -> expect InvalidObjectState.
4. CreateBucket; PutObject a 10 MiB source; run 8 concurrent CopyObject calls of it to distinct destination keys while deleting the source -> expect success or 404 NoSuchKey, never ValidationException naming a per-bucket byte budget.
5. Provision a bucket whose Intelligent-Tiering configuration declares Id MyIntelligentTieringConfig with Prefix documents/ and TagFilter department=finance plus ARCHIVE_ACCESS@90 and DEEP_ARCHIVE_ACCESS@180; call ListBucketIntelligentTieringConfigurations -> expect the single And-wrapped configuration.
6. Create an ACL-enabled bucket, wire SQS queues for s3:ObjectCreated:Put, s3:ObjectCreated:Copy, s3:ObjectTagging:Put and s3:ObjectAcl:Put; PutObject with x-amz-tagging and a canned ACL, then CopyObject with TaggingDirective=REPLACE plus a canned ACL -> expect exactly one message on each creation queue and none on the tagging/ACL queues; then PutObjectTagging/PutObjectAcl on the same key -> expect those queues to receive.
