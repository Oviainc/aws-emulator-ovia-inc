Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.publish-asset-sha256-mismatch-rejected
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.publish-asset-sha256-mismatch-rejected|run#18|AssertionError|assert refusal == expected, (
Issue-Commit: 11b7a3ace789222a3c07035f7536e49e419e0113

# REOPENS: PublishPackageVersion with a mismatched assetSHA256 is still refused 403 SignatureDoesNotMatch instead of 400 ValidationException naming both digests

> _Note: this issue was identified against the code base at commit `11b7a3ace789222a3c07035f7536e49e419e0113`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishPackageVersion

## Description
REOPENS 20261031T213622Z-codeartifact-data-plane-refusals-diverge-digest-mismatch-answered.md: its fix left the call refused. That issue is marked fixed and carries this exact replay key (codeartifact.publish-asset-sha256-mismatch-rejected) as its Cause 1, but replaying the check against this checkout still gets the same 403 SignatureDoesNotMatch refusal, so the digest-integrity contract of PublishPackageVersion remains unreachable. This is not described as a new defect.

One cause (Cause 1 of 1): a PublishPackageVersion whose declared assetSHA256 disagrees with the uploaded bytes is refused as an AUTHENTICATION failure before the operation is dispatched, instead of as the operation's modeled application-layer validation failure.

AWS: Smithy PublishPackageVersion (POST /v1/package/version/publish, success 200) binds assetSHA256 -> SHA256 to the HTTP header x-amz-content-sha256, required, @length(min 64, max 64), @pattern(^[0-9a-f]+$); assetContent -> Asset is the required streaming @httpPayload. Its modeled errors are AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400 — SignatureDoesNotMatch is not modeled at all. A declared digest that disagrees with the body is therefore an APPLICATION-layer refusal: the request authenticates, the service hashes the received bytes, compares, stores nothing, and refuses with ValidationException / HTTP 400 whose message is "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '<declared>', Actual hash '<computed>'". Fixed text (exactly, with no trailing period): "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '", then "', Actual hash '", then "'". The two hex digests are the ONLY variable parts: <declared> is the assetSHA256 the request sent on x-amz-content-sha256, <computed> is the SHA-256 the service computed over the uploaded bytes. The digests quoted in the evidence below are one run's values filling that template, not part of the requirement.

Emulator: HTTP 403 with error code SignatureDoesNotMatch and message 'The request signature does not match' — an error class this operation does not model, on a status the operation only uses for AccessDeniedException. No content-hash comparison is reported and the computed digest is never named.

Why: a client relying on AWS's behavior cannot reach the integrity branch at all. SDKs and tools classify SignatureDoesNotMatch as a credential or clock problem and re-sign, refresh credentials, or alarm on an authorization failure; a corrupted upload is therefore diagnosed as a signing failure, and the 400/ValidationException branch a publisher needs in order to recompute its digest and retry the UPLOAD is never taken. Because the refusal never names the computed digest, a caller also loses the only evidence that the service hashed the payload rather than trusting the header.

Fix: the refusal is decided by the generic SigV4 payload-hash verification in the request pipeline, which re-derives the body hash and enforces it even for an operation whose model binds an input member to x-amz-content-sha256 alongside a streaming payload; that verification must treat a member-bound 64-character lowercase-hex x-amz-content-sha256 as the precomputed signed payload hash rather than re-deriving it, so the request authenticates. The digest comparison and the modeled ValidationException with the sentence above then belong in the PublishPackageVersion handler's validation (services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/publish_package_version.rs). Note the earlier fixed issue reached the same conclusion and the behavior did not change, so verify end to end with the check rather than by inspecting the handler alone. The open issue 20261101T084500Z-codeartifact-publishpackageversion-digest-mismatch-refused-as-signaturedoesnotmatch.md describes this same symptom, but under a different replay key (codeartifact.codeartifact-publish-with-mismatched-asset-sha256-rejected-and-version-absent); it does not carry this check's key.

Traces: codeartifact.publish-asset-sha256-mismatch-rejected.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator booted from this checkout: 83/95 passed (87.4%), 8 mismatched, 4 errored, all on service codeartifact. Eleven of the twelve failures are marked ALREADY FILED / OWNED; this one is not.

[mismatch] codeartifact.publish-asset-sha256-mismatch-rejected
  File "check_codeartifact_publish_asset_sha256_mismatch_rejected.py", line 73, in run
  assert refusal == expected, (
  AssertionError: a publish with a mismatched assetSHA256 returned ('SignatureDoesNotMatch', 'The request signature does not match'), expected ('ValidationException', "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash 'e082bb3059bd78f769564587b8fee9fe31739c1be0b8f396c73beeddefd908bc', Actual hash '06ce2f7126c9297b17c3f7d527cbe2a37452179f0d86625beb9d6f27903fabab'"): the refusal names the declared digest and the digest it COMPUTED from the bytes, which is what proves the service hashed the payload rather than trusting the header

Expected vs actual:
  expected: HTTP 400, code ValidationException, message "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '<declared>', Actual hash '<computed>'"
  actual:   HTTP 403, code SignatureDoesNotMatch, message 'The request signature does not match'

Request: publish_package_version(domain=D, repository=R, format='generic', namespace='sim2real', package='sim2real-integrity', packageVersion='2.0.0', assetName='payload.bin', assetContent=b'the-bytes-that-are-actually-sent\n', assetSHA256=sha256(b'the-bytes-whose-digest-was-declared\n').hexdigest()). The two digests in the message above are that run's values filling the template; only the fixed text and the rule for the variable parts (declared = the request's x-amz-content-sha256, computed = the SHA-256 of the uploaded bytes) are the requirement.

The same call carrying the TRUE digest is served by the handler (the check's positive control), so this is not an unimplemented-operation cause: only the mismatched-digest request is refused, and it is refused before the operation runs.

Ownership: 20261031T213622Z-codeartifact-data-plane-refusals-diverge-digest-mismatch-answered.md is [fixed] and lists Issue-Key codeartifact.publish-asset-sha256-mismatch-rejected|run#18|AssertionError|assert refusal == expected, ( — the same key that still fails here, which is why this is filed as a reopen.

## Steps to Reproduce
Boot the emulator from this checkout and replay codeartifact.publish-asset-sha256-mismatch-rejected. By hand, with a codeartifact client pointed at the emulator:
1. create_domain(domain=D); create_repository(domain=D, repository=R).
2. payload = b'the-bytes-that-are-actually-sent\n'; other = b'the-bytes-whose-digest-was-declared\n'.
3. publish_package_version(domain=D, repository=R, format='generic', namespace='sim2real', package=P, packageVersion='1.0.0', assetName='payload.bin', assetContent=payload, assetSHA256=sha256(payload).hexdigest()) -> 200 (setup).
4. publish_package_version(domain=D, repository=R, format='generic', namespace='sim2real', package=P, packageVersion='2.0.0', assetName='payload.bin', assetContent=payload, assetSHA256=sha256(other).hexdigest()) -> expect ValidationException / HTTP 400 with "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '<sha256(other)>', Actual hash '<sha256(payload)>'"; observe HTTP 403 SignatureDoesNotMatch / 'The request signature does not match'.
5. list_package_versions(domain=D, repository=R, format='generic', namespace='sim2real', package=P) -> only '1.0.0': the rejected publish must store nothing.
6. Repeat step 4 with the TRUE digest -> 200 with status 'Published', and get_package_version_asset returns the payload bytes. That control shows the refusal in step 4 is the payload-hash check in the auth layer, not the handler.
