Issue-Source: trace-sweep-divergence
Issue-Trace: s3-analytics-configuration-full-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-analytics-configuration-full-lifecycle s3-bucket-monitoring-and-analytics-configurations s3-bucket-metrics-configuration-multi-filter-listing s3-metrics-configuration-filtering s3-inventory-configuration-reporting-setup s3-intelligent-tiering-archive-tier-configuration s3-bucket-configuration-crud s3-bucket-lifecycle-config-lifecycle s3-bucket-lifecycle-notifications-replication s3-object-lifecycle-rules-management s3-object-restore-and-encryption s3-object-tagging-acl-and-torrent s3-object-tagging-and-acl-management s3-object-tagging-acl-and-versioning s3-object-versioning-and-lock s3-versioning-and-object-lock
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketAnalyticsConfiguration|2xx->2xx|status
Issue-Key: s3|PutBucketMetricsConfiguration|2xx->2xx|status
Issue-Key: s3|PutBucketInventoryConfiguration|2xx->2xx|status
Issue-Key: s3|PutBucketIntelligentTieringConfiguration|2xx->2xx|status
Issue-Key: s3|PutBucketLifecycleConfiguration|2xx->2xx|$
Issue-Key: s3|RestoreObject|2xx->2xx|$
Issue-Key: s3|RestoreObject|2xx->2xx|status
Issue-Key: s3|PutObjectTagging|2xx->2xx|$
Issue-Key: s3|PutObjectLockConfiguration|2xx->2xx|$
Issue-Key: s3|PutObjectAcl|2xx->2xx|$
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 empty-body writes answer the wrong status and emit a spurious XML body

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketAnalyticsConfiguration

## Description
Nine net-new root causes sharing one fault: the emulator's generic success path answers these write operations with HTTP 200 and an XML document named after the operation's output shape, where AWS answers either 204/202 or 200 with no body at all. A fixer must close all nine.

AWS (common): the four Put*Configuration sub-resource writes answer HTTP 204 No Content with no body (recorded behavior; Smithy models them with an empty output — output_shape Unit — and the default http code 200, so the 204 comes from the recordings). The remaining operations model only header-bound output members (PutBucketLifecycleConfigurationOutput.TransitionDefaultMinimumObjectSize -> x-amz-transition-default-minimum-object-size, PutObjectTaggingOutput.VersionId -> x-amz-version-id, PutObjectAclOutput/PutObjectLockConfigurationOutput.RequestCharged -> x-amz-request-charged, RestoreObjectOutput.RequestCharged/RestoreOutputPath -> headers), so AWS's response body is empty; RestoreObject's first accepted restore is 202 Accepted.

Emulator (common): 200 plus, for several of them, a body of the form <PutBucketLifecycleConfigurationOutput></PutBucketLifecycleConfigurationOutput> — the output shape's Smithy name serialized as an XML root, a name that exists in no AWS response.

Why (common): a client that branches on 204 vs 200 (to decide whether anything was returned), or that parses the body of a successful write, sees a shape AWS never sends; strict XML parsers and body-length assertions fail on it. For RestoreObject the 202-vs-200 distinction is the documented way to make a restore idempotent — 202 means 'I initiated it', 200 means 'already in progress/complete' — so the caller double-counts restores.

Cause 1 of 9: PutBucketAnalyticsConfiguration answers 200 instead of 204.
Fix: the PutBucketAnalyticsConfiguration handler's success status.
Traces: s3-analytics-configuration-full-lifecycle#task#2 (also s3-analytics-configuration-full-lifecycle#task#3, s3-bucket-monitoring-and-analytics-configurations#task#2).

Cause 2 of 9: PutBucketMetricsConfiguration answers 200 instead of 204.
Fix: the PutBucketMetricsConfiguration handler's success status.
Traces: s3-bucket-metrics-configuration-multi-filter-listing#setup#2 (also setup#3, setup#4, s3-bucket-monitoring-and-analytics-configurations#task#12, s3-metrics-configuration-filtering#task#2, #task#3).

Cause 3 of 9: PutBucketInventoryConfiguration answers 200 instead of 204.
Fix: the PutBucketInventoryConfiguration handler's success status.
Traces: s3-bucket-monitoring-and-analytics-configurations#task#7 (also s3-inventory-configuration-reporting-setup#task#2, #task#3).

Cause 4 of 9: PutBucketIntelligentTieringConfiguration answers 200 instead of 204.
Fix: the PutBucketIntelligentTieringConfiguration handler's success status.
Traces: s3-bucket-monitoring-and-analytics-configurations#task#17 (also s3-intelligent-tiering-archive-tier-configuration#task#2, #task#4, #task#6).

Cause 5 of 9: PutBucketLifecycleConfiguration returns a <PutBucketLifecycleConfigurationOutput> body.
AWS: 200 with an empty body.
Emulator: body is {'PutBucketLifecycleConfigurationOutput': ''}.
Fix: the PutBucketLifecycleConfiguration serializer, which must not emit a body for a header-only output shape.
Traces: s3-bucket-configuration-crud#task#10 (also s3-bucket-lifecycle-config-lifecycle#task#2, s3-bucket-lifecycle-notifications-replication#task#5, s3-object-lifecycle-rules-management#task#2, #task#4).

Cause 6 of 9: RestoreObject answers 200 with a <RestoreObjectOutput> body instead of 202 with no body.
Fix: the RestoreObject handler's status selection and serializer. The open behavioral issue 20261029T211510Z-s3-representation-divergences-lifecycle-transition-validator-restoreobject-202.md describes the 202 status but carries no wire replay key for this step, and does not cover the spurious body.
Traces: s3-object-restore-and-encryption#task#3.

Cause 7 of 9: PutObjectTagging returns a <PutObjectTaggingOutput> body.
Fix: the PutObjectTagging serializer.
Traces: s3-object-tagging-acl-and-torrent#task#3 (also s3-object-tagging-and-acl-management#task#3).

Cause 8 of 9: PutObjectLockConfiguration returns a <PutObjectLockConfigurationOutput> body.
Fix: the PutObjectLockConfiguration serializer.
Traces: s3-object-tagging-acl-and-versioning#task#14 (also s3-object-versioning-and-lock#task#7, s3-versioning-and-object-lock#task#3).

Cause 9 of 9: PutObjectAcl returns a <PutObjectAclOutput> body.
Fix: the PutObjectAcl serializer.
Traces: s3-object-tagging-and-acl-management#task#7

## Evidence
All nine clusters are 'mismatch'.
s3-analytics-configuration-full-lifecycle#task#2 (PutBucketAnalyticsConfiguration): status expected 204, got 200
s3-bucket-metrics-configuration-multi-filter-listing#setup#2 (PutBucketMetricsConfiguration): status expected 204, got 200
s3-bucket-monitoring-and-analytics-configurations#task#7 (PutBucketInventoryConfiguration): status expected 204, got 200
s3-bucket-monitoring-and-analytics-configurations#task#17 (PutBucketIntelligentTieringConfiguration): status expected 204, got 200
s3-bucket-configuration-crud#task#10 (PutBucketLifecycleConfiguration): $ value_mismatch (expected {}, got {'PutBucketLifecycleConfigurationOutput': ''})
s3-object-restore-and-encryption#task#3 (RestoreObject): status expected 202, got 200; $ value_mismatch (expected {}, got {'RestoreObjectOutput': ''})
s3-object-tagging-acl-and-torrent#task#3 (PutObjectTagging): $ value_mismatch (expected {}, got {'PutObjectTaggingOutput': ''})
s3-object-tagging-acl-and-versioning#task#14 (PutObjectLockConfiguration): $ value_mismatch (expected {}, got {'PutObjectLockConfigurationOutput': ''})
s3-object-tagging-and-acl-management#task#7 (PutObjectAcl): $ value_mismatch (expected {}, got {'PutObjectAclOutput': ''})

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand, on an existing bucket: PUT /<bucket>?analytics&id=…, ?metrics&id=…, ?inventory&id=… and ?intelligent-tiering&id=… and check the status is 204 with no body; PUT /<bucket>?lifecycle, PUT /<bucket>/<key>?tagging, PUT /<bucket>?object-lock and PUT /<bucket>/<key>?acl and check the response body is empty; POST /<bucket>/<key>?restore for an archived object and check the first call is 202 with no body.
