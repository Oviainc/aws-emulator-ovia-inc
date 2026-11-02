Issue-Source: trace-sweep-divergence
Issue-Trace: s3-cob-max-buckets-value-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-cob-max-buckets-value-mistakes s3-csbs-hand-rolled-paging-parameter-mistakes s3-ovm-traj-list-versions-parameter-mistakes s3-csvcols-response-override-params s3-txt-conditional-date-headers-and-precedence s3-cob-bucket-region-empty-value-rejected s3-lsd-delimiter-parameter-variants
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|ListBuckets|4xx->4xx|$.Error.Code
Issue-Key: s3|ListBuckets|4xx->4xx|$.Error.Message
Issue-Key: s3|ListBuckets|2xx->4xx|$.ListAllMyBucketsResult
Issue-Key: s3|ListBuckets|2xx->4xx|status
Issue-Key: s3|ListObjectsV2|4xx->4xx|$.Error.Code
Issue-Key: s3|ListObjectVersions|4xx->4xx|$.Error.ArgumentName
Issue-Key: s3|ListObjectVersions|4xx->4xx|$.Error.ArgumentValue
Issue-Key: s3|ListObjectVersions|4xx->4xx|$.Error.Code
Issue-Key: s3|ListObjectVersions|4xx->4xx|$.Error.Message
Issue-Key: s3|ListObjectVersions|2xx->4xx|$.ListVersionsResult
Issue-Key: s3|ListObjectVersions|2xx->4xx|status
Issue-Key: s3|GetObject|2xx->4xx|$
Issue-Key: s3|GetObject|2xx->4xx|$headers.accept-ranges
Issue-Key: s3|GetObject|2xx->4xx|$headers.content-encoding
Issue-Key: s3|GetObject|2xx->4xx|$headers.content-language
Issue-Key: s3|GetObject|2xx->4xx|$headers.content-type
Issue-Key: s3|GetObject|2xx->4xx|$headers.etag
Issue-Key: s3|GetObject|2xx->4xx|$headers.expires
Issue-Key: s3|GetObject|2xx->4xx|$headers.last-modified
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-checksum-crc32
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-checksum-type
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-server-side-encryption
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-version-id
Issue-Key: s3|GetObject|2xx->4xx|status
Issue-Key: s3|GetObject|3xx->4xx|$.ETag
Issue-Key: s3|GetObject|3xx->4xx|$headers.etag
Issue-Key: s3|GetObject|3xx->4xx|$headers.last-modified
Issue-Key: s3|GetObject|3xx->4xx|status
Issue-Key: s3|ListBuckets|4xx->2xx|$.Error
Issue-Key: s3|ListBuckets|4xx->2xx|status
Issue-Key: s3|ListObjectsV2|2xx->4xx|$.ListBucketResult
Issue-Key: s3|ListObjectsV2|2xx->4xx|status
Issue-Related: 20261030T053639Z-s3-refusals-use-the-wrong-error-code-and.md
Issue-Related: 20261102T111455Z-s3-refusal-envelopes-omit-aws-contextual-error-members.md
Issue-Related: 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 scalar parameter refusals leak Smithy coercion wording

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBuckets

## Description
Cause 1 of 6: a non-integer max-buckets is refused with the deserializer's own words. AWS: ListBuckets with max-buckets=1.5 answers 400 Code 'InvalidArgument', Message 'Provided max-buckets not an integer or within integer range', ArgumentName 'max-buckets', ArgumentValue the value as sent. Recorded behavior only. Emulator: 400 Code 'ValidationException', Message 'invalid i32 `1.5`: invalid digit found in string'. Why: 'ValidationException' is not an S3 error code at all, so SDK error matching on InvalidArgument fails and the message leaks Rust/Smithy internals. Fix: coerce scalar query parameters inside the ListBuckets request validator and raise S3's InvalidArgument instead of letting the generated deserializer answer. Traces: s3-cob-max-buckets-value-mistakes#task#3.

Cause 2 of 6: the same defect on max-keys for both object listings. AWS: 400 InvalidArgument 'Provided max-keys not an integer or within integer range' with ArgumentName 'max-keys'; for a negative value the message is 'max-keys cannot be negative'. Recorded behavior only. Emulator: 'ValidationException'/'invalid i32 `abc`: invalid digit found in string', and for -1 'Argument max-keys must be an integer between 0 and 2147483647: -1'. Fix: the ListObjectsV2 / ListObjectVersions request validators. Traces: s3-csbs-hand-rolled-paging-parameter-mistakes#task#6, s3-ovm-traj-list-versions-parameter-mistakes#task#6, s3-ovm-traj-list-versions-parameter-mistakes#task#7.

Cause 3 of 6: a response-override timestamp makes a valid GetObject fail. AWS: response-expires=not-a-date is accepted and echoed - the object is returned 200 with Expires set to the supplied text, plus accept-ranges, etag, last-modified, checksum and SSE headers. Recorded behavior only. Emulator: 400 ValidationException 'invalid timestamp `not-a-date`: ... Smithy does not support timezone offsets in RFC-3339 date times' and no object body at all. Why: a read that AWS serves is refused, so the caller loses data access, not just an error message. Fix: parse response-* overrides as opaque header text in the GetObject handler. Traces: s3-csvcols-response-override-params#task#6.

Cause 4 of 6: RFC-1123 conditional date headers are rejected. AWS: If-Modified-Since / If-Unmodified-Since carry HTTP-date values ('Sat, 01 Jan 2000 00:00:00 GMT'); the read is served 200 (or 304 with ETag and Last-Modified when the precondition says so). Recorded behavior only. Emulator: 400 ValidationException with the same RFC-3339 complaint, so neither the 200 nor the 304 branch is ever reached. Why: every conditional-GET client (browsers, caches, aws s3 sync) is broken. Fix: parse conditional headers as HTTP-date in the GetObject/HeadObject precondition path. Traces: s3-txt-conditional-date-headers-and-precedence#task#3, s3-txt-conditional-date-headers-and-precedence#task#8.

Cause 5 of 6: an empty bucket-region is accepted. AWS: ListBuckets with bucket-region= answers 400 InvalidArgument 'Argument value  is not a valid AWS Region' with ArgumentName 'bucket-region'. Recorded behavior only. Emulator: 200 with a listing. Why: a typo silently returns an unfiltered list instead of an error. Fix: the ListBuckets request validator. Traces: s3-cob-bucket-region-empty-value-rejected#task#2.

Cause 6 of 6: requests AWS serves are refused 400 after a parameter edge case. AWS: max-buckets at its accepted bound and the delimiter/encoding-type variants in these steps return 200 with the full ListAllMyBucketsResult / ListBucketResult. Recorded behavior only. Emulator: 400 with no result element. Why: valid paging and delimiter walks fail outright. Fix: the same validators, which must bound-check rather than reject. Traces: s3-cob-max-buckets-value-mistakes#task#6, s3-ovm-traj-list-versions-parameter-mistakes#task#8, s3-lsd-delimiter-parameter-variants#task#6.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-cob-max-buckets-value-mistakes#task#3 (ListBuckets, 400/400): $.Error.Code expected 'InvalidArgument' got 'ValidationException'; $.Error.Message expected 'Provided max-buckets not an integer or within integer range' got 'invalid i32 `1.5`: invalid digit found in string'; ArgumentName/ArgumentValue missing. s3-ovm-traj-list-versions-parameter-mistakes#task#7: Message expected 'max-keys cannot be negative' got 'Argument max-keys must be an integer between 0 and 2147483647: -1'. s3-txt-conditional-date-headers-and-precedence#task#3: status expected 200 got 400, body is a ValidationException about RFC-3339 timestamps, and accept-ranges/etag/last-modified/x-amz-checksum-crc32/x-amz-server-side-encryption are all missing. s3-cob-bucket-region-empty-value-rejected#task#2: status expected 400 got 200. s3-cob-max-buckets-value-mistakes#task#6: status expected 200 got 400.

## Steps to Reproduce
1. Replay s3-cob-max-buckets-value-mistakes and inspect step task#3 (max-buckets=1.5) and task#6 (valid request). 2. Replay s3-txt-conditional-date-headers-and-precedence steps task#3 and task#8 with HTTP-date conditional headers. 3. Replay s3-csvcols-response-override-params step task#6 with response-expires=not-a-date. 4. Replay s3-cob-bucket-region-empty-value-rejected step task#2 with an empty bucket-region.
