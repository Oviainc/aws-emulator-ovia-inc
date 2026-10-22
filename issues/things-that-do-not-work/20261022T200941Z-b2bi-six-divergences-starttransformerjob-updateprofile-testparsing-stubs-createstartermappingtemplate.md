Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi.start-transformer-job-inactive-transformer-rejected b2bi.get-profile-not-found b2bi.create-starter-mapping-template-with-output-sample b2bi.create-transformer-idempotent-token-concurrent-dedup b2bi.test-parsing-malformed-edi-document-error b2bi.access-denied-missing-iam-permission-rejected
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi.start-transformer-job-inactive-transformer-rejected|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: b2bi.get-profile-not-found|run#19|AssertionError|assert mutate_code == "ResourceNotFoundException", (
Issue-Key: b2bi|CreateStarterMappingTemplate|ResourceNotFoundException
Issue-Key: b2bi.create-transformer-idempotent-token-concurrent-dedup|run#14|AssertionError|assert error_codes, (
Issue-Key: b2bi.test-parsing-malformed-edi-document-error|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: b2bi.access-denied-missing-iam-permission-rejected|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# b2bi: six divergences — StartTransformerJob/UpdateProfile/TestParsing stubs, CreateStarterMappingTemplate refuses outputSampleLocation, CreateTransformer never conflicts on a shared clientToken, IAM Deny not enforced (6 causes)

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTransformerJob

## Description
Six distinct root causes in the b2bi emulator, all found by the behavioral suite (b2bi 7/42). Each cause below is independent and must be closed before this issue is done. Related open issue: 20261022T031134Z-b2bi-tagging-test-harness-and-transformer-operations-answer.md (TagResource/ListTagsForResource/TestConversion/TestMapping/CreateTransformer-as-stub) — it does NOT cover any of the six causes here except as noted in Cause 4, and the suite no longer binds its replay key to these checks because the observed failures have changed.

Cause 1 of 6: StartTransformerJob is an unimplemented scaffold, so an inactive transformer is not refused.
AWS: Smithy: StartTransformerJob input inputFile/outputLocation/transformerId required (transformerId len 1..64, pattern ^[a-zA-Z0-9_-]+$), output transformerJobId required, errors = AccessDenied(403)/Conflict(409)/InternalServer(500)/ResourceNotFound(404)/Throttling(429)/Validation(400), success 200. Recorded behavior: starting a job whose transformerId resolves to a transformer in status 'inactive' is refused with ValidationException (HTTP 400) and writes nothing under the requested output prefix; an 'active' transformer with the identical input runs and reaches status 'succeeded'. Recorded message example: "Transformer <transformerId> must be active".
Emulator: every call answers the scaffold 501 NotImplemented, so the check saw error code 'NotImplemented' instead of 'ValidationException'.
Why: a client cannot run any transformer job at all, and the status precondition that protects it from running an unconfigured transformer is never enforced — a caller cannot distinguish 'transformer not active' (400, fix the transformer) from a service fault.
Fix: implement the StartTransformerJob handler, including the transformer-status precondition (inactive -> ValidationException) and job state for GetTransformerJob.
Traces: b2bi.start-transformer-job-inactive-transformer-rejected.

Cause 2 of 6: UpdateProfile is an unimplemented scaffold, so a mutation on a deleted profile is not refused as not-found.
AWS: Smithy: UpdateProfile input profileId required (len 1..64, pattern ^[a-zA-Z0-9_-]+$), errors = AccessDenied(403)/Conflict(409)/InternalServer(500)/ResourceNotFound(404)/ServiceQuotaExceeded(402)/Throttling(429)/Validation(400), success 200 echoing the profile. Recorded behavior: after DeleteProfile, an UpdateProfile on that same profileId is refused with ResourceNotFoundException (HTTP 404) — the same resolution verdict GetProfile already gives (the GetProfile not-found beats of this check pass). Recorded message example: "Profile with profileId <id> not found."
Emulator: UpdateProfile answers the scaffold 501 NotImplemented for every call, so the check saw 'NotImplemented' where 'ResourceNotFoundException' is required.
Why: clients cannot rename or re-point any profile, and a mutation against a stale id yields a non-modeled 501 instead of the 404 that tells them the resource is gone.
Fix: implement the UpdateProfile handler against the profile store, resolving profileId and returning ResourceNotFoundException when it does not resolve.
Traces: b2bi.get-profile-not-found.

Cause 3 of 6: CreateStarterMappingTemplate refuses every request that carries outputSampleLocation instead of deriving the template from the sample.
AWS: Smithy: CreateStarterMappingTemplate input mappingType + templateDetails required, outputSampleLocation (S3Location) optional; output mappingTemplate required; errors = AccessDenied(403)/InternalServer(500)/ResourceNotFound(404)/Validation(400), success 200. Recorded behavior: when outputSampleLocation names a readable sample EDI object, AWS returns 200 and a template DERIVED from that sample — the sample's ISA/GS/ST envelope values are carried through verbatim and each transaction-set business field becomes a "PLACEHOLDER: <sample value>" marker; ResourceNotFoundException is for a sample that genuinely cannot be read.
Emulator: the handler returns ResourceNotFoundException unconditionally whenever outputSampleLocation is present, before any attempt to resolve the object — even for an object the emulator's own S3 PutObject just stored successfully in the same check.
Why: the whole sample-driven path of the operation is unusable; a client that passes a perfectly readable sample is told the sample does not exist, so it cannot tell a genuine missing-object 404 from an unimplemented path, and any template it does obtain carries none of the sample's values.
Fix: the CreateStarterMappingTemplate handler must resolve outputSampleLocation through the emulator's S3 object state and generate the template from the parsed sample, reserving ResourceNotFoundException for a bucket/key that really does not resolve (see services/service-com-amazonaws-b2bi-b2bi/src/ops/create_starter_mapping_template.rs:1043-1065, the unconditional refusal and its 'no S3 object is reachable from this service' comment).
Traces: b2bi.create-starter-mapping-template-with-output-sample.

Cause 4 of 6: CreateTransformer dedupes a shared clientToken but never returns ConflictException to a losing concurrent racer.
AWS: Smithy: CreateTransformer carries clientToken (@idempotencyToken) and models ConflictException (httpError 409) alongside ValidationException (400); success 201. Recorded behavior: N concurrent CreateTransformer calls sharing one clientToken and identical parameters yield exactly ONE transformer and the racers that lose the in-flight race are refused with ConflictException; only a REPLAY issued after the original request completed returns the original transformerId.
Emulator: all 5 concurrent racers returned success with the same transformerId and zero ConflictException was raised (check assertion: "expected some of the 5 concurrent racers sharing a clientToken to lose and receive ConflictException, but all 5 succeeded"). So the token dedup now exists (one transformerId is shared, which is progress over the stub state recorded in the related issue above) but the in-flight conflict arm is missing.
Why: a client that fans out retries on one clientToken and relies on 409 to back off never sees a conflict, so its retry/backoff path is dead code and a genuinely concurrent duplicate request is silently reported as a success it did not perform.
Fix: in the CreateTransformer handler's clientToken dedup (transformer state), distinguish in-flight from completed: an in-flight token -> ConflictException (409); a completed token -> replay the original transformerId. Partially overlaps Cause 5 of the related open issue named above, which recorded this requirement while the operation was still a stub.
Traces: b2bi.create-transformer-idempotent-token-concurrent-dedup.

Cause 5 of 6: TestParsing is an unimplemented scaffold, so a non-EDI document is not classified as a client error.
AWS: Smithy: TestParsing input inputFile/fileFormat/ediType required, output parsedFileContent required, errors = AccessDenied(403)/InternalServer(500)/ResourceNotFound(404)/Throttling(429)/Validation(400). Recorded behavior: an inputFile whose content is not a supported EDI interchange is refused with ValidationException (HTTP 400) (recorded message example: "The EDI file could not be parsed. No supported EDI standard interchange begins with thi"), while a valid X12 850 document in the same bucket parses with 200 and surfaces its BEG03 value in parsedFileContent.
Emulator: answers the scaffold 501 NotImplemented, so the check saw 'NotImplemented' instead of 'ValidationException'. The suite auto-filed a bare "implement the TestParsing scaffold stub" issue naming services/service-com-amazonaws-b2bi-b2bi/src/ops/test_parsing.rs for a different check; that issue does not state the malformed-input classification required here.
Why: a client validating a document before onboarding a trading partner gets no signal, and cannot distinguish bad input it must fix (400) from a service fault it should retry.
Fix: implement the TestParsing handler with EDI interchange detection/parsing: unparsable or non-EDI content -> ValidationException; valid X12 -> 200 with parsedFileContent.
Traces: b2bi.test-parsing-malformed-edi-document-error.

Cause 6 of 6: an explicit IAM Deny on b2bi:CreateProfile is not enforced, so AccessDeniedException is never produced.
AWS: Smithy: CreateProfile models AccessDeniedException (httpError 403) among its errors. Recorded behavior: for one assumed-role session, CreateProfile succeeds under an Allow-only inline policy; after an explicit Deny on b2bi:CreateProfile is added to that same inline policy the identical call is refused with AccessDeniedException (403) and creates nothing (confirmed by an independent ListProfiles read); once the Deny is removed the identical call succeeds again.
Emulator: the check never observed AccessDeniedException — it was killed by the harness after 60s while polling for the Deny to take effect, so no AccessDeniedException was ever returned within the polling window. Hypothesis: the emulator does not evaluate IAM identity policies when authorizing b2bi requests, so the request is served as if allowed and an administrator-imposed Deny can never take effect.
Why: this is an overly-permissive divergence — a call real AWS refuses is performed, and the resource is created. Any client or test that asserts a least-privilege boundary (or relies on a Deny as a guardrail) passes against the emulator and fails against AWS.
Fix: authorization for b2bi operations must consult the emulator's IAM identity policies for the calling (assumed-role) principal and return the modeled AccessDeniedException when an explicit Deny matches, instead of unconditionally allowing.
Traces: b2bi.access-denied-missing-iam-permission-rejected.

## Evidence
Suite simparity-behavioral-aws-v1, b2bi 7/42 (17%), 35 failed.

Cause 1 — [mismatch] b2bi.start-transformer-job-inactive-transformer-rejected: AssertionError: expected ValidationException, got 'NotImplemented' (start_transformer_job on a transformer whose get_transformer status == 'inactive'). Stub confirmed: services/service-com-amazonaws-b2bi-b2bi/src/ops/start_transformer_job.rs:4 SCAFFOLDED marker, :183 Scaffolded::not_implemented("StartTransformerJob").

Cause 2 — [mismatch] b2bi.get-profile-not-found: AssertionError: a mutation on a deleted profile must also be rejected as not-found, got 'NotImplemented' (update_profile(profileId=<deleted id>, name='renamed')); the preceding get_profile not-found and positive-control beats passed. Stub confirmed: services/service-com-amazonaws-b2bi-b2bi/src/ops/update_profile.rs:4 SCAFFOLDED marker, :202 Scaffolded::not_implemented("UpdateProfile").

Cause 3 — [error] b2bi.create-starter-mapping-template-with-output-sample: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the CreateStarterMappingTemplate operation: "the sample EDI file at outputSampleLocation s3://<bucket>/samples/sample-850-output.edi cannot be read, so no mapping template can be generated from it; omit outputSampleLocation to generate a template from templateDetails" — raised although the check had just stored that object with s3.put_object. Source: services/service-com-amazonaws-b2bi-b2bi/src/ops/create_starter_mapping_template.rs:1049-1065 returns ResourceNotFoundException for ANY present outputSampleLocation.

Cause 4 — [mismatch] b2bi.create-transformer-idempotent-token-concurrent-dedup: AssertionError: expected some of the 5 concurrent racers sharing a clientToken to lose and receive ConflictException, but all 5 succeeded. The earlier assertion that all successful racers share ONE transformerId passed, so dedup is present and only the 409 arm is missing.

Cause 5 — [mismatch] b2bi.test-parsing-malformed-edi-document-error: AssertionError: expected ValidationException, got 'NotImplemented' (test_parsing on an object containing "this is plainly not an X12 EDI document"; expected message example "The EDI file could not be parsed. No supported EDI standard interchange begins with thi").

Cause 6 — [mismatch] b2bi.access-denied-missing-iam-permission-rejected: "Check timed out after 60.0s". The check assumes one role, confirms CreateProfile succeeds under an Allow-only inline policy, then polls (90s budget, 5s interval) for AccessDeniedException after adding an explicit Deny on b2bi:CreateProfile; no AccessDeniedException was ever returned, so the poll ran until the harness killed the check. Every CreateProfile observed during the Deny window was a success the check had to undo.

## Steps to Reproduce
1. Boot the emulator and point botocore b2bi/s3/iam/sts clients at it.
2. Cause 1: create_transformer(name=..., inputConversion={fromFormat:'X12', formatOptions:{x12:{transactionSet:'X12_850', version:'VERSION_4010'}}}, mapping={templateLanguage:'JSONATA', template:'$'}) -> status 'inactive'; then start_transformer_job(inputFile={bucketName,key}, outputLocation={bucketName,key:'out-inactive/'}, transformerId=<inactive id>) -> expect ValidationException (400); emulator returns 501 NotImplemented.
3. Cause 2: create_profile(...); delete_profile(profileId=<id>); update_profile(profileId=<id>, name='renamed') -> expect ResourceNotFoundException (404); emulator returns 501 NotImplemented.
4. Cause 3: s3.create_bucket + s3.put_object(Key='samples/sample-850-output.edi', Body=<valid X12 850>); create_starter_mapping_template(mappingType='JSONATA', templateDetails={x12:{transactionSet:'X12_850', version:'VERSION_4010'}}, outputSampleLocation={bucketName,key}) -> expect 200 with a template whose envelope values come from the sample and whose business fields are 'PLACEHOLDER: <sample value>'; emulator returns ResourceNotFoundException.
5. Cause 4: fire 5 concurrent create_transformer calls sharing one clientToken and identical parameters -> expect exactly one success and ConflictException for the losers; emulator returns success to all 5.
6. Cause 5: s3.put_object(Key='not-edi-at-all.txt', Body=b'this is plainly not an X12 EDI document'); test_parsing(ediType={x12Details:{transactionSet:'X12_850', version:'VERSION_4010'}}, fileFormat='JSON', inputFile={bucketName,key}) -> expect ValidationException (400); emulator returns 501 NotImplemented.
7. Cause 6: iam.create_role + put_role_policy(Allow b2bi:CreateProfile/GetProfile/ListProfiles/DeleteProfile/TagResource); sts.assume_role; create_profile under the assumed session -> succeeds; put_role_policy adding {Effect:'Deny', Action:'b2bi:CreateProfile'}; repeat the identical create_profile -> expect AccessDeniedException (403) and no profile created; emulator keeps succeeding.
8. Re-replay the checks listed in trace_ids.
