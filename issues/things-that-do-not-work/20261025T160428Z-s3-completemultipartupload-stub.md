Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.cli-s3-cp-large-file-multipart-checksum-defaults s3.conc-s3-conditional-writes-complete-multipart-if-none-match-rejected-when-key-appears-mid-upload s3.get-object-part-number-reads-single-part s3.multipart-complete-part-too-small-rejected s3.multipart-upload-and-presigned-get-round-trip s3.s3-oc-enforced-copy-and-multipart-accept-an-owner-only-canned-acl
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.cli-s3-cp-large-file-multipart-checksum-defaults|_run#13|AssertionError|assert r.returncode == 0, f"{' '.join(argv)} failed (rc={r.returncode}): {r.stderr[-600:]!r}"
Issue-Key: s3|CompleteMultipartUpload|NotImplemented
Issue-Commit: bf4b5d75768eba06386b10826e7ace22b8c67a1e

# Implement s3 CompleteMultipartUpload: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `bf4b5d75768eba06386b10826e7ace22b8c67a1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteMultipartUpload

## Description
`CompleteMultipartUpload` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3-amazons3/src/ops/complete_multipart_upload.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CompleteMultipartUpload` call, so each one unblocks only when `CompleteMultipartUpload` works: `s3.cli-s3-cp-large-file-multipart-checksum-defaults`, `s3.conc-s3-conditional-writes-complete-multipart-if-none-match-rejected-when-key-appears-mid-upload`, `s3.get-object-part-number-reads-single-part`, `s3.multipart-complete-part-too-small-rejected`, `s3.multipart-upload-and-presigned-get-round-trip`, `s3.s3-oc-enforced-copy-and-multipart-accept-an-owner-only-canned-acl`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3-amazons3/src/ops/complete_multipart_upload.rs`. Replaying the corpus, the emulator refused the `CompleteMultipartUpload` call: File "check_s3_cli_s3_cp_large_file_multipart_checksum_defaults.py", line 54, in run
    _run(["aws", "s3", "cp", str(local), f"s3://{bucket}/{sha_key}",
    ~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
          "--checksum-algorithm", "SHA256"], env)
          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "check_s3_cli_s3_cp_large_file_multipart_checksum_defaults.py", line 128, in _run
    assert r.returncode == 0, f"{' '.join(argv)} failed (rc={r.returncode}): {r.stderr[-600:]!r}"
           ^^^^^^^^^^^^^^^^^
AssertionError: aws s3 cp /tmp/mf-emulator-homes/8-8813/809534201484.tmp/sim2real-clichk-zouuxqvk/large.bin s3://sim2real-behav-clichk-8140c681103e/large-sha256.bin --checksum-algorithm SHA256 failed (rc=1): 'upload failed: ../tmp/mf-emulator-homes/8-8813/809534201484.tmp/sim2real-clichk-zouuxqvk/large.bin to s3://sim2real-behav-clichk-8140c681103e/large-sha256.bin An error occurred (NotImplemented) when calling the CompleteMultipartUpload operation: operation CompleteMultipartUpload is scaffolded but not implemented in this emulator\n'

## Steps to Reproduce
Boot the s3 emulator
Call CompleteMultipartUpload and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
