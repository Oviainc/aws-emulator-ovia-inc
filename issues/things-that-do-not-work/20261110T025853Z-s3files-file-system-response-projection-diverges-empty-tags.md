Issue-Source: trace-sweep-divergence
Issue-Trace: s3files-nfs-mount-write-lands-in-bucket
Issue-Step: task#23
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3files-nfs-mount-write-lands-in-bucket
Issue-Shape-Id: com.amazonaws.s3files#S3Files
Issue-Key: s3files|CreateFileSystem|2xx->2xx|$.creationTime
Issue-Key: s3files|CreateFileSystem|2xx->2xx|$.tags
Issue-Key: s3files|GetFileSystem|2xx->2xx|$.creationTime
Issue-Key: s3files|GetFileSystem|2xx->2xx|$.tags
Issue-Commit: 109411407d03b40f255761f7a04c49ae72d75d8c

# s3files file-system response projection diverges: empty tags omitted and creationTime carries sub-second precision

> _Note: this issue was identified against the code base at commit `109411407d03b40f255761f7a04c49ae72d75d8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFileSystem, GetFileSystem

## Description
Two net-new wire divergence clusters in the `s3files` service, both in the file-system response projection: the `tags` member is omitted entirely when a file system carries no tags where AWS serializes an empty list, and `creationTime` is serialized with sub-second precision where AWS's recorded value is a whole-number epoch-seconds timestamp. Both symptoms appear on the create path AND on the read-back path, and each path is its OWN divergence cluster with its own replay binding, so BOTH causes below must be re-replayed and closed before submitting even though one shared edit may fix both.

Cause 1 of 2: CreateFileSystem's 201 response omits `tags` when the file system has none, and emits a fractional `creationTime` (fixture s3files-nfs-mount-write-lands-in-bucket, step task#23).
AWS: Smithy (services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json) models CreateFileSystem as PUT /file-systems with success status 201, restJson1 protocol, output CreateFileSystemResponse{creationTime (Timestamp), fileSystemArn, fileSystemId, bucket, prefix (@default ""), clientToken, kmsKeyId, status (LifeCycleState), statusMessage, roleArn, ownerId, tags (TagList), name}. `creationTime` is documented "The time when the file system was created, in seconds since 1970-01-01T00:00:00Z (Unix epoch time)" and restJson1's default timestamp encoding is epoch-seconds. Recorded behavior: the response carries `tags` as an EMPTY LIST for a file system created without tags (the member is always present, never dropped), and `creationTime` as an epoch-seconds number of whole-second shape — the recorded instant itself is per-run and is NOT the requirement; the member's presence and its numeric shape are.
Emulator: the replayed step answers 201 but `$.tags` is ABSENT (expected [], got None) and `$.creationTime` fails the recorded shape check (got 1794279168.02605 — a float with microsecond fraction) where the recording's value is a whole-number epoch-seconds value. Hypothesis: `creation_time` is taken straight from `SystemTime::now()` and serialized at the clock's full resolution rather than truncated to seconds.
Why: a client that reads `tags` unconditionally (an SDK consumer that iterates the list, a tag-reconciliation loop, an IaC provider diffing desired vs actual tags) gets a null where AWS guarantees a list, which is a null-dereference or a spurious "tags removed" diff rather than "no tags". A caller or recorder that round-trips `creationTime` through a whole-second epoch representation loses or re-formats the fractional part, so equality checks against a previously-read value and any cache keyed on the timestamp break.
Fix: CreateFileSystem's output projection in services/service-com-amazonaws-s3files-s3files/src/ops/create_file_system.rs — the `tags: (!stored.tags.is_empty()).then(|| stored.tags.clone())` projection (line 306) must emit the list unconditionally, and `creation_time` (line 295, `DateTime::from(stored.creation_time)`) must be truncated to whole epoch seconds, or the state must store it truncated.
Traces: s3files-nfs-mount-write-lands-in-bucket#task#23.

Cause 2 of 2: GetFileSystem's 200 read-back shows the identical two defects, so the stored record reads back wrong too (same fixture, step task#24).
AWS: Smithy: GetFileSystem GET /file-systems/{fileSystemId}, status 200, output GetFileSystemResponse with the same member set — `creationTime` (Timestamp), `tags` (TagList) — errors ResourceNotFoundException (404), ValidationException (400), ThrottlingException (429), InternalServerException (500). Recorded behavior: the read-back carries `tags: []` for an untagged file system and the same whole-second epoch `creationTime` the create returned.
Emulator: HTTP 200 with `$.tags` missing (expected [], got None) and `$.creationTime` = 1794279168.02605, failing the recorded shape — i.e. the defect is in the projection used by BOTH operations, not only in create.
Why: the read path is what clients poll (waiters on `status`, describe-then-compare loops), so every poll re-delivers the null `tags` and the fractional timestamp; a client that compares the create response's creationTime to a later GetFileSystem's also sees a value AWS would have kept stable and whole-second.
Fix: the same two members in services/service-com-amazonaws-s3files-s3files/src/ops/get_file_system.rs (lines 173 and 184). Since both handlers hand-roll the identical projection, the durable fix is a shared file-system-to-response projection in this service's state/serializer layer. NOTE: the open issue 20261022T204614Z-s3files-six-control-plane-fidelity-defects-missing-nested.md covers other CreateFileSystem/GetFileSystem faults (clientToken dedupe, roleArn validation, not-found message) but carries NO wire replay key for these two steps and does not mention `tags` or `creationTime`, so it does not cover this; the two steps must be re-replayed here.
Traces: s3files-nfs-mount-write-lands-in-bucket#task#24.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture s3files-nfs-mount-write-lands-in-bucket.

Cause 1 — step task#23, CreateFileSystem [mismatch, status 201 -> 201]:
  $.creationTime: value_mismatch (expected <shaped-not-asserted> — the recorded whole-number epoch-seconds timestamp; got 1794279168.02605)
  $.tags: missing (expected [], got None)

Cause 2 — step task#24, GetFileSystem [mismatch, status 200 -> 200]:
  $.creationTime: value_mismatch (expected <shaped-not-asserted>, got 1794279168.02605)
  $.tags: missing (expected [], got None)

The recorded instant (and therefore the literal number above) is per-run and is not the requirement; the requirement is that `tags` is always serialized (as [] when empty) and that `creationTime` has the recorded whole-second epoch shape.

Code: services/service-com-amazonaws-s3files-s3files/src/ops/create_file_system.rs:295 (`creation_time: Some(DateTime::from(stored.creation_time))`) and :306 (`tags: (!stored.tags.is_empty()).then(|| stored.tags.clone())`); services/service-com-amazonaws-s3files-s3files/src/ops/get_file_system.rs:173 and :184 (identical two lines).

Model: smithy_query operation CreateFileSystem/GetFileSystem and shapes CreateFileSystemResponse/GetFileSystemResponse/TagList/Tag in services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json; service protocol restJson1 (epoch-seconds default timestamp encoding).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), fixture s3files-nfs-mount-write-lands-in-bucket.
2. Cause 1 — step task#23: call s3files.CreateFileSystem(bucket=<an existing bucket ARN>, roleArn=<an IAM role ARN>) with NO tags. Observe HTTP 201 whose body has no `tags` member at all and whose `creationTime` is a float with a fractional part (e.g. ...168.02605). AWS returns `tags: []` and a whole-number epoch-seconds `creationTime`.
3. Cause 2 — step task#24: call s3files.GetFileSystem(fileSystemId=<the id returned above>). Observe HTTP 200 with the same two divergences, confirming the read-back projection has them too.
4. Both steps must pass on re-replay before this issue is closed.
