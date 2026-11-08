Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-generic-package-publish-and-reads
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-generic-package-publish-and-reads codeartifact-generic-publish-validation-envelopes
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|GetPackageVersionReadme|4xx->2xx|$.message
Issue-Key: codeartifact|GetPackageVersionReadme|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: codeartifact|GetPackageVersionReadme|4xx->2xx|status
Issue-Key: codeartifact|PublishPackageVersion|4xx->4xx|$.message
Issue-Key: codeartifact|PublishPackageVersion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: codeartifact|PublishPackageVersion|4xx->4xx|status
Issue-Commit: d7142f0d93a1083aeee635c6d7e6fd3141132351

# CodeArtifact package-version error paths answer the wrong status instead of the modeled exception

> _Note: this issue was identified against the code base at commit `d7142f0d93a1083aeee635c6d7e6fd3141132351`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPackageVersionReadme

## Description
Two package-version operations fail to produce the modeled 4xx CodeArtifact answers on their error path: one answers 200 success where AWS answers 404, the other is refused by the signing layer with 403 where AWS answers 400. Two causes; both must be closed.

Cause 1 of 2: GetPackageVersionReadme for a version that has no readme answers 200 instead of 404 ResourceNotFoundException.
AWS: Smithy `GetPackageVersionReadme` GET /v1/package/version/readme, http 200 on success, errors include `ResourceNotFoundException` (httpError 404, required member `message`, optional `resourceId`/`resourceType`). The recorded response for a generic-format version with no readme is status 404, header `x-amzn-errortype: ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/`, body `message` = "The readme file of this package version is not found". That sentence is entirely fixed text — it has no variable parts taken from the request or state. So AWS treats "this version exists but carries no readme" as a missing RESOURCE, not as a successful read with the member omitted.
Emulator: answers 200 with an output whose `readme` member is simply absent — the handler resolves domain, repository, package and version, and then returns `GetPackageVersionReadmeOutput { .., readme: version.readme.clone() }`, which is `None` for a version published without a readme. No `message` and no `x-amzn-errortype` header are emitted.
Why: a client polling or branching on the 404 ("no readme yet") sees a 200 whose `readme` is missing and must guess; SDK callers that rely on `ResourceNotFoundException` being raised instead fall through into code that dereferences a readme that is not there, and retry/poll loops written against AWS never terminate the same way.
Fix: the GetPackageVersionReadme handler — after the version is resolved, a version whose stored readme is absent must raise the modeled `ResourceNotFoundException` (404) with AWS's fixed sentence rather than returning a 200 with the member omitted.
Traces: codeartifact-generic-package-publish-and-reads#task#11. Emulator code: services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/get_package_version_readme.rs lines 257-291. Note the open issue 20261107T222858Z-codeartifact-getpackageversionreadme-stub.md asks for this operation to be IMPLEMENTED (it since is, and answers 200 here); it does not cover this error path, and the replay marked this divergence neither stubbed nor owned.

Cause 2 of 2: PublishPackageVersion whose assetSHA256 disagrees with the body is refused 403 SignatureDoesNotMatch instead of 400 ValidationException.
AWS: Smithy `PublishPackageVersion` POST /v1/package/version/publish; `assetSHA256` is bound to the `x-amz-content-sha256` HEADER (required, length 64, pattern ^[0-9a-f]+$) and `assetContent` is the streaming httpPayload; errors include `ValidationException` (httpError 400, required `message`, optional `reason`). The recorded response is status 400, header `x-amzn-errortype: ValidationException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/`, and `message` of the form "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '<assetSHA256 as sent in x-amz-content-sha256>', Actual hash '<SHA-256 of the uploaded assetContent>'" — the two quoted hex digests are the variable parts (one from the request header, one computed from the body); everything else, including the algorithm name 'SHA-256', is fixed text.
Emulator: answers 403 with `x-amzn-errortype: SignatureDoesNotMatch` and `message` "The request signature does not match" — the SigV4 layer treats `x-amz-content-sha256` as the signed payload hash and rejects the request before the operation's own content-hash validation runs, so the modeled ValidationException is never produced.
Why: a client that uploads an asset with a stale or wrong digest gets an authentication failure instead of a validation failure: SDK retry policies treat 403 SignatureDoesNotMatch as a credential/clock problem (re-sign and retry, or surface a credentials error) rather than a non-retryable bad request, and the caller never learns which hash was expected versus calculated.
Fix: the PublishPackageVersion request path — the signing middleware must not verify this operation's `x-amz-content-sha256` header as the payload signature hash (it is a modeled input member), and the operation's validation must compute the SHA-256 of `assetContent` and raise `ValidationException` (400) with AWS's sentence naming both hashes.
Traces: codeartifact-generic-publish-validation-envelopes#task#5. Open issues 20261101T084500Z-codeartifact-publishpackageversion-digest-mismatch-refused-as-signaturedoesnotmatch.md and 20261103T235912Z-codeartifact-reopens-publishpackageversion-with-a-mismatched-assetsha256-is-still.md describe this same behavior but the replay reports this step as NOT owned by either (neither carries this fixture/step replay key), so it is filed here with the key attached.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout:

Cause 1 — fixture codeartifact-generic-package-publish-and-reads, step task#11, GetPackageVersionReadme:
  status: expected 404, got 200
  $.message: missing (expected 'The readme file of this package version is not found', got None)
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/', got None)
  divergence_key: service=codeartifact|GetPackageVersionReadme|status=404->200|keys=$.message,$headers.x-amzn-errortype,status

Cause 2 — fixture codeartifact-generic-publish-validation-envelopes, step task#5, PublishPackageVersion:
  status: expected 400, got 403
  $.message: value_mismatch (expected "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '<assetSHA256 from the request header>', Actual hash '<SHA-256 of the body>'", got 'The request signature does not match')
  $headers.x-amzn-errortype: shape_mismatch (expected 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/', got 'SignatureDoesNotMatch')
  divergence_key: service=codeartifact|PublishPackageVersion|status=400->403|keys=$.message,$headers.x-amzn-errortype,status

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: replay fixture codeartifact-generic-package-publish-and-reads step task#11 — publish a generic package version with no readme, then call GetPackageVersionReadme for it. The emulator answers 200 with no readme member; AWS answers 404 ResourceNotFoundException 'The readme file of this package version is not found'.
3. Cause 2: replay fixture codeartifact-generic-publish-validation-envelopes step task#5 — POST PublishPackageVersion with an x-amz-content-sha256 / assetSHA256 that does not match the uploaded assetContent. The emulator answers 403 SignatureDoesNotMatch; AWS answers 400 ValidationException naming the expected and actual hashes.
