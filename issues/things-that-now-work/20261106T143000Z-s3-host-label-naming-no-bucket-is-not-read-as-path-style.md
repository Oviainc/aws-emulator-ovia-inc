Issue-Severity: HIGH
Issue-Corpus: workspace-tests
Issue-Corpus-Kind: workspace
Issue-Commit: d4bbd2a22

# RESOLVED on mainline: a virtual-hosted-style host whose label named no bucket was not read as path-style, so Amazon S3's own `presigned_object_reads` suite failed

## Operation
GetObject

## Description
Filed while the workspace test suite was RED on it, and RESOLVED by mainline's own core
virtual-host addressing change before this report ever reached a fixer — recorded here so the
observation and its outcome are not lost.

What failed: `services/service-com-amazonaws-s3-amazons3/tests/all/presigned_object_reads.rs`
case "a host label naming no bucket" —

    GET /host-style-bucket/written.bin
    Host: absent-host-bucket.s3.us-east-1.amazonaws.com

had to resolve from the PATH, because `absent-host-bucket` is no bucket of the store, and
answered HTTP 404 `<Error><Code>NoSuchBucket</Code>…<BucketName>absent-host-bucket</BucketName>`
instead — an answer about an address nobody named. `src/service.rs`'s `addressed_in_host`
documents and implements the rule the suite asserts ("a host-style request naming a bucket this
emulator does NOT hold is read as path-style"), so the bucket the response named was resolved
somewhere that did not consult it. The same defect showed through
`core/router/tests/all/cloudfront_oac_s3_origin_edge.rs`, whose S3 setup addressed a bucket
path-style under the bucket's own domain.

What resolved it: mainline's core virtual-host addressing (`VirtualHostAddressing`, cited by
that router suite's own `regional_endpoint` helper, which mainline introduced for the same
reason). Verified on the merged tree:
`cargo test -p service-com-amazonaws-s3-amazons3 --test all presigned_object_reads` passes
(2 passed), and `cargo test -p aws-emulator-router --test all` is 535/535 green.

## Evidence
Before: the assertion above panicked at `presigned_object_reads.rs:385` with
"case \"a host label naming no bucket\": the read must resolve from the path, got 257 bytes
with status 404".
After (same checkout with mainline merged): both tests of that suite pass, and the CloudFront
OAC router suite passes with its setup calls addressed to the Regional endpoint.

## Steps to Reproduce
No longer reproducible: `cargo test -p service-com-amazonaws-s3-amazons3 --test all
presigned_object_reads` passes on mainline.
