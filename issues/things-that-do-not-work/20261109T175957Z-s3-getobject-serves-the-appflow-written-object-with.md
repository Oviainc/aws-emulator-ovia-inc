Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-s3-to-s3-flow-run-transfers-records
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-s3-to-s3-flow-run-transfers-records
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: s3|GetObject|2xx->2xx|$
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-checksum-type
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-server-side-encryption
Issue-Related: 20261102T111455Z-s3-object-read-and-batch-delete-response-contracts.md
Issue-Related: 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md
Issue-Commit: db165e9afd311b39a9697deff319fb6ff63a6914

# S3 GetObject serves the AppFlow-written object with reordered JSON records, no full-object checksum and the wrong server-side-encryption report

> _Note: this issue was identified against the code base at commit `db165e9afd311b39a9697deff319fb6ff63a6914`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1 (three facets of one GetObject response): reading back the object an AppFlow S3 destination wrote returns 200, but the payload bytes and three response headers diverge from the recording.

Facet (a) — payload byte order. AWS: recorded behavior only. The destination object is JSON Lines, one record per line, and each record preserves the FIELD ORDER of the source record as read from the source object (the recorded body lists each record's members in source order, id before city before temp). Emulator: each record is re-serialized with its keys sorted alphabetically (city, id, temp), so the object's bytes differ from what AWS stores even though the record set is identical. The requirement is order preservation, not the recorded literal lines.

Facet (b) — missing full-object checksum headers. AWS: Smithy (com.amazonaws.s3#GetObject, GET /{Bucket}/{Key+}) models output ChecksumCRC64NVME -> httpHeader x-amz-checksum-crc64nvme and ChecksumType -> httpHeader x-amz-checksum-type (enum FULL_OBJECT|COMPOSITE), with httpChecksum.requestValidationModeMember = ChecksumMode (request header x-amz-checksum-mode) and responseAlgorithms including CRC64NVME. An object stored with a default full-object CRC64NVME checksum is read back with x-amz-checksum-crc64nvme set to the base64 CRC64NVME of the object's bytes and x-amz-checksum-type: FULL_OBJECT. Emulator: both headers are absent. The recorded value '+VAf27mVm4s=' is the checksum of that recording's bytes — the requirement is the correctly computed checksum of the stored object, not that literal.

Facet (c) — wrong server-side-encryption report. AWS: Smithy models output ServerSideEncryption -> httpHeader x-amz-server-side-encryption (enum, values include AES256 and aws:kms) and SSEKMSKeyId -> httpHeader x-amz-server-side-encryption-aws-kms-key-id (sensitive). The AppFlow S3 destination writes with KMS server-side encryption, so GetObject reports x-amz-server-side-encryption: aws:kms together with the KMS key id/ARN that encrypted the object. Emulator: reports x-amz-server-side-encryption: AES256 and omits x-amz-server-side-encryption-aws-kms-key-id entirely, i.e. the object's recorded encryption mode is not retained and the KMS key is never reported (the key ARN itself is environment-specific and is not the requirement — its presence and agreement with the object's encryption mode are).

Why: a client that verifies integrity on read (SDK checksum validation, or any consumer that compares x-amz-checksum-crc64nvme against its own digest) gets no checksum to validate and silently skips verification, so corruption that AWS would surface passes. A client that reconciles bytes or digests end-to-end sees a different payload than AWS would serve, because the JSON member order was not preserved. A client or compliance check that asserts KMS encryption on pipeline output reads AES256 with no key id and concludes the object is only SSE-S3 encrypted, failing a control that passes on AWS.

Fix: the s3 GetObject response serializer plus the object-metadata state it reads — the stored object must retain its encryption mode and KMS key id and its full-object checksum algorithm/value so GetObject can emit x-amz-server-side-encryption, x-amz-server-side-encryption-aws-kms-key-id, x-amz-checksum-crc64nvme and x-amz-checksum-type; and the appflow flow-run record writer that produces the destination object must serialize each record preserving the source field order (and write the object with the destination's KMS encryption) instead of emitting a key-sorted map. Related open issues that do NOT carry this step's replay key and do not cover it: 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md and 20261109T165919Z-s3-getobject-drops-the-transfer-written-object-s.md (other write paths' GetObject header loss), and 20261109T164957Z-cloudformation-kms-s3-a-bucket-created-by-a.md (AES256-instead-of-aws:kms on a CFN-created bucket).

Traces: appflow-s3-to-s3-flow-run-transfers-records#task#10 (GetObject of the flow's destination object). The sibling listing divergence on the same fixture (ListObjectsV2, task#9) is already owned by open issues and is not re-filed here.

## Evidence
Fixture appflow-s3-to-s3-flow-run-transfers-records, step task#10, GetObject, expected 200 / actual 200, response diverges:
  $: value_mismatch — expected body '{"id":"1","city":"lisbon","temp":"21"}\n{"id":"2","city":"oslo","temp":"4"}\n{"id":"3","city":"lima","temp":"18"}\n', got '{"city":"lisbon","id":"1","temp":"21"}\n{"city":"oslo","id":"2","temp":"4"}\n{"city":"lima","id":"3","temp":"18"}\n' (same records, keys alphabetized instead of source order)
  $headers.x-amz-checksum-crc64nvme: missing (expected the base64 CRC64NVME of the object bytes, '+VAf27mVm4s=' in this recording, got None)
  $headers.x-amz-checksum-type: missing (expected 'FULL_OBJECT', got None)
  $headers.x-amz-server-side-encryption: value_mismatch (expected 'aws:kms', got 'AES256')
  $headers.x-amz-server-side-encryption-aws-kms-key-id: missing (expected the encrypting KMS key id, got None)
Smithy citations: GetObject output ChecksumCRC64NVME -> x-amz-checksum-crc64nvme; ChecksumType -> x-amz-checksum-type (FULL_OBJECT|COMPOSITE); ServerSideEncryption -> x-amz-server-side-encryption; SSEKMSKeyId -> x-amz-server-side-encryption-aws-kms-key-id; httpChecksum.responseAlgorithms includes CRC64NVME, requestValidationModeMember ChecksumMode; errors NoSuchKey(404)/InvalidObjectState(403).

## Steps to Reproduce
1. Run the fixture's S3-source → S3-destination AppFlow flow so it writes its output object (fixture appflow-s3-to-s3-flow-run-transfers-records).
2. GET the destination object (GetObject on the output/<flow>/<execution> key), requesting x-amz-checksum-mode: ENABLED.
3. Compare the body bytes: each JSON-Lines record's keys come back alphabetized rather than in the source record's field order.
4. Inspect the response headers: x-amz-checksum-crc64nvme and x-amz-checksum-type are absent, and x-amz-server-side-encryption is AES256 with no x-amz-server-side-encryption-aws-kms-key-id, where AWS reports aws:kms plus the key id.
5. Replay step task#10 of fixture appflow-s3-to-s3-flow-run-transfers-records to confirm.
