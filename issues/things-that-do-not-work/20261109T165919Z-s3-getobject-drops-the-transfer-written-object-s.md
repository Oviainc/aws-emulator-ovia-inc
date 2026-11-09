Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-sftp-connector-sends-file-to-own-server
Issue-Step: task#20
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-sftp-connector-sends-file-to-own-server
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-meta-user-agent
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-meta-user-agent-id
Issue-Related: 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md
Issue-Commit: 67d35df6c1acce919d84587dd79dfbb9f77fdc15

# S3 GetObject drops the Transfer-written object's user metadata and its CRC64NVME full-object checksum header

> _Note: this issue was identified against the code base at commit `67d35df6c1acce919d84587dd79dfbb9f77fdc15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One root cause with two header families, on the S3 GetObject response for an object delivered into a bucket by a Transfer Family SFTP transfer: the object's header-bound metadata is not served back. Status matches (200) and the body is served.

Cause 1 of 1: GetObject returns neither the stored x-amz-meta-* user metadata nor the object's full-object checksum header.
AWS: Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json) GetObjectOutput.Metadata targets com.amazonaws.s3#Metadata and is bound @httpPrefixHeaders("x-amz-meta-"), so every user-metadata entry stored with the object is echoed back on GetObject (and HeadObject) as x-amz-meta-<key>: <value>; GetObject also carries @httpChecksum with requestValidationModeMember ChecksumMode and responseAlgorithms [CRC64NVME, CRC32, CRC32C, SHA256, SHA1, SHA512, MD5, XXHASH64, XXHASH3, XXHASH128], with GetObjectOutput.ChecksumCRC64NVME bound to the x-amz-checksum-crc64nvme header — returned for the algorithm the object was stored with, CRC64NVME being the default full-object algorithm for an upload that names none. For the specific metadata keys the S3 model says nothing, so: recorded behavior only — a file that Transfer Family writes into S3 on a client's behalf is stored with the provenance metadata x-amz-meta-user-agent, whose value is the fixed text 'AWSTransfer', and x-amz-meta-user-agent-id, whose value is the variable pair <transfer user name>@<serverId> taken from the session that delivered the file (the recorded 'sim2real-probe-tu-bca28893@s-00000000000000001' is this run's generated user name and server id, not part of the contract); the checksum value is the object's own base64 CRC64NVME over its bytes (the recorded '9a3dyGwAQ9w=' is this object's content checksum, not a constant).
Emulator: all three headers are absent from the 200 response — x-amz-meta-user-agent, x-amz-meta-user-agent-id and x-amz-checksum-crc64nvme all come back None.
Why: a client reading back a file that arrived over SFTP has no way to learn it was written by Transfer Family or which user/server session delivered it, which is the documented provenance path for auditing an inbound transfer; and a client that verifies integrity on download gets no checksum header at all, so it cannot distinguish an object stored without a checksum from a header the server dropped, and GET->PUT round trips silently strip metadata. Note both ends must be closed: if the Transfer delivery path never stores the metadata, the omission is in that writer rather than in the GetObject serializer.
Fix: the S3 GetObject/HeadObject response-header serializer (the @httpPrefixHeaders projection of the stored object's metadata map, and the checksum-header emission for the object's stored algorithm), the S3 object-write path's default full-object checksum algorithm (CRC64NVME when the caller names none), and the Transfer Family S3 delivery writer that must stamp user-agent/user-agent-id metadata on objects it writes.
Traces: transfer-sftp-connector-sends-file-to-own-server#task#20.

Related OPEN issue that does NOT carry this wire key: 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md. It describes the same serializer family (user metadata, default Content-Type, CRC64NVME) but is bound to Issue-Trace databrew-recipe-job-writes-transformed-csv#task#12 and carries only that fixture's keys ($headers.x-amz-meta-job_name, $headers.x-amz-meta-job_run_id, $headers.content-type, $headers.x-amz-checksum-crc64nvme), so a fixer closing it would never re-replay this binding and this object's Transfer-written metadata would stay unexercised. Re-replay transfer-sftp-connector-sends-file-to-own-server#task#20 even if that issue lands first.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 304/345 steps passed, 41 mismatched, score 88.1%, emulator_up true. This cluster is size 1, verdict 'mismatch', with EMPTY owned_by and EMPTY stub_path.

Fixture transfer-sftp-connector-sends-file-to-own-server, step task#20, operation GetObject, service s3, expected_status 200, actual_status 200 (status matches; the response headers diverge).
detail:
  $headers.x-amz-checksum-crc64nvme: missing (expected '9a3dyGwAQ9w=', got None)
  $headers.x-amz-meta-user-agent: missing (expected 'AWSTransfer', got None)
  $headers.x-amz-meta-user-agent-id: missing (expected 'sim2real-probe-tu-bca28893@s-00000000000000001', got None)
divergence_key: service=s3|GetObject|status=200->200|keys=$headers.x-amz-checksum-crc64nvme,$headers.x-amz-meta-user-agent,$headers.x-amz-meta-user-agent-id
Fixed vs variable: 'AWSTransfer' is fixed text; the user-agent-id value is the template <transfer user name>@<serverId> filled from the delivering session (the recorded user name and server id are this run's generated values); the checksum value is computed over the object's bytes.

Model evidence: smithy_query query=operation operation=GetObject on services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json — GetObjectOutput.Metadata @httpPrefixHeaders 'x-amz-meta-', GetObjectOutput.ChecksumCRC64NVME -> x-amz-checksum-crc64nvme, GetObjectRequest.ChecksumMode -> x-amz-checksum-mode, httpChecksum.responseAlgorithms leading with CRC64NVME.
Related-but-not-covering open issue: 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md (same serializer family, different fixture binding and keys).

## Steps to Reproduce
1. git_clone, build and boot the emulator from this checkout, then replay simparity-wire-aws-v1 and re-run the binding in Issue-Trace/Issue-Step: transfer-sftp-connector-sends-file-to-own-server#task#20.
2. By hand (region us-west-2): stand up an S3-domain Transfer Family SFTP server with a SERVICE_MANAGED user whose HomeDirectory is a bucket path, create an SFTP connector pointing at that server with a working UserSecretId and the server's host key trusted, then transfer start-file-transfer --connector-id c-... --send-file-paths /<bucket>/<file> --remote-directory-path /<bucket> and poll transfer list-file-transfer-results until the entry reaches COMPLETED.
3. aws s3api get-object --bucket <bucket> --key <delivered key> --checksum-mode ENABLED out.bin and inspect the 200 response headers: x-amz-meta-user-agent must be 'AWSTransfer', x-amz-meta-user-agent-id must be '<transfer user name>@<serverId>' for the session that delivered the file, and x-amz-checksum-crc64nvme must carry the object's base64 CRC64NVME checksum. Observe all three absent.
4. Equivalently, independent of Transfer: PUT an object with two x-amz-meta-<key> headers and no x-amz-sdk-checksum-algorithm, then GET it with x-amz-checksum-mode: ENABLED — the stored metadata and the default-algorithm checksum header must both be served back.
