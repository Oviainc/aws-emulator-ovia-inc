Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-generic-publish-validation-envelopes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-generic-publish-validation-envelopes codeartifact-package-origin-configuration-and-version-delete codeartifact-external-connection-lifecycle codeartifact-repository-error-paths codeartifact-resource-not-found-envelopes codeartifact-package-group-hierarchy-and-association
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|PublishPackageVersion|4xx->2xx|$.message
Issue-Key: codeartifact|PublishPackageVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: codeartifact|PublishPackageVersion|4xx->2xx|status
Issue-Key: codeartifact|PublishPackageVersion|4xx->4xx|$.message
Issue-Key: codeartifact|PublishPackageVersion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: codeartifact|PublishPackageVersion|4xx->4xx|status
Issue-Key: codeartifact|PublishPackageVersion|4xx->4xx|$.resourceId
Issue-Key: codeartifact|AssociateExternalConnection|4xx->4xx|$.message
Issue-Key: codeartifact|AssociateExternalConnection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: codeartifact|AssociateExternalConnection|4xx->4xx|status
Issue-Key: codeartifact|DescribePackageVersion|4xx->4xx|$.message
Issue-Key: codeartifact|DescribePackageVersion|4xx->4xx|$.resourceId
Issue-Key: codeartifact|DescribePackageVersion|4xx->4xx|$.resourceType
Issue-Key: codeartifact|ListTagsForResource|4xx->4xx|$.message
Issue-Key: codeartifact|ListTagsForResource|4xx->4xx|$.resourceId
Issue-Key: codeartifact|CreatePackageGroup|4xx->4xx|$.message
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CodeArtifact error envelopes carry the wrong status, error type and resource fields

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishPackageVersion

## Description
Eight causes in the CodeArtifact refusal paths: wrong HTTP status, wrong error shape, or wrong structured members in the error body. All eight must be closed before submitting.

Cause 1 of 8: PublishPackageVersion accepts a generic package with no `namespace`.
AWS: namespace is optional in the model (Smithy: PublishPackageVersion.namespace -> PackageNamespace, @length 1..255, @pattern ^[^#/\s]+$) but is required by the generic format — the recorded response is HTTP 400 with x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/' and the message "'namespace' is required when 'format' is 'generic'." (the message is built from the request members, so naming the member and the condition is the requirement; the exact wording is an example). Smithy: PublishPackageVersion errors -> ValidationException httpError 400.
Emulator: answers HTTP 200 and publishes the version with no namespace.
Why: a client gets a package version stored under a namespace-less identity that AWS would never create, so every later DescribePackageVersion/DeletePackageVersions keyed on the namespace misses it. The negative test for this constraint passes vacuously.
Fix: PublishPackageVersion's input validation (format-conditional required members).
Traces: codeartifact-generic-publish-validation-envelopes#task#4.

Cause 2 of 8: a declared asset digest that disagrees with the body is refused in the auth layer as SignatureDoesNotMatch instead of as a ValidationException.
AWS: assetSHA256 is bound to the x-amz-content-sha256 header (Smithy: PublishPackageVersion.assetSHA256 -> SHA256, required, @length 64..64, @pattern ^[0-9a-f]+$) and assetContent is the streaming httpPayload. A mismatch is a modeled application error: HTTP 400, x-amzn-errortype 'ValidationException...', message naming both digests ('The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash <declared>, Actual hash <computed>' — built from the request, so naming both digests is the requirement).
Emulator: answers HTTP 403 with x-amzn-errortype 'SignatureDoesNotMatch' and 'The request signature does not match' — the request is rejected by signature verification before the handler runs.
Why: the integrity contract of this operation is unreachable: a client that uploads a corrupted asset is told its credentials/signature are wrong and re-signs instead of recomputing its digest.
Fix: the SigV4 payload-hash verification in the request pipeline must not enforce the body hash for an operation whose model binds a member to x-amz-content-sha256 with a streaming payload; the digest comparison belongs in PublishPackageVersion's validation. The open behavioral issue '20261022T171312Z-codeartifact-no-iam-authorization-on-reads-publishpackageversion-digest' (cause 2) describes the same root cause but carries no wire replay key, so this wire cause is filed here with its fixture binding; closing one should close both.
Traces: codeartifact-generic-publish-validation-envelopes#task#5 (2-fixture cluster, also codeartifact-package-origin-configuration-and-version-delete#task#6).

Cause 3 of 8: the republish ConflictException reports the wrong resourceId.
AWS: republishing an existing Published generic version is refused with HTTP 409 (Smithy: PublishPackageVersion errors -> ConflictException httpError 409) and the body's resourceId is the fully-qualified package-version identity '<namespace>/<package>/<version>'.
Emulator: returns 409 with resourceId '1.0.0' — only the version component.
Why: resourceId is a structured, machine-read member of the error shape; a client that keys retry/skip logic or logging off it cannot tell WHICH package the conflict refers to, and two conflicts on different packages become indistinguishable.
Fix: the ConflictException construction in the PublishPackageVersion handler.
Traces: codeartifact-generic-publish-validation-envelopes#task#6.

Cause 4 of 8: exceeding the external-connection limit is refused as ServiceQuotaExceededException (402) instead of ValidationException (400).
AWS: the recorded refusal for associating a second external connection is HTTP 400 with x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/' and message 'ExternalConnections for repositories are currently limited to 1' (example wording). Both shapes are modeled on this operation (Smithy: AssociateExternalConnection errors -> ValidationException 400, ServiceQuotaExceededException 402), so the classification is the requirement, not merely the text.
Emulator: answers HTTP 402 with x-amzn-errortype 'ServiceQuotaExceededException'.
Why: a client catching ValidationException (a permanent input error) instead sees a quota error, which SDK retry/backoff policies and operator runbooks treat as a transient, raise-a-limit-increase condition — so the request is retried forever rather than fixed.
Fix: the AssociateExternalConnection handler's limit branch — raise ValidationException (400).
Traces: codeartifact-external-connection-lifecycle#task#5.

Cause 5 of 8: an unknown external-connection name is refused with 400 ValidationException instead of 404.
AWS: the recorded refusal for externalConnection 'public:does-not-exist' is HTTP 404 (the recording did not assert the errortype; Smithy: AssociateExternalConnection errors -> ResourceNotFoundException httpError 404).
Emulator: answers HTTP 400 with x-amzn-errortype 'ValidationException' and "Invalid external connection 'public:does-not-exist'."
Why: 400 vs 404 is the difference between 'your request is malformed' and 'that connection does not exist'; clients branch on the status, and the name does satisfy the member's pattern (Smithy: ExternalConnectionName @length 2..100, @pattern ^[A-Za-z0-9][A-Za-z0-9._\-:]{1,99}$), so a 400 is not reachable on AWS for this input.
Fix: the external-connection name lookup in the AssociateExternalConnection handler — unknown-but-well-formed names are not-found, not invalid.
Traces: codeartifact-repository-error-paths#task#7.

Cause 6 of 8: the not-found envelope for a missing package version names the wrong resource.
AWS: ResourceNotFoundException carries message, resourceId and resourceType (Smithy: ResourceNotFoundException members message (required), resourceId, resourceType -> ResourceType enum). When the PACKAGE itself does not exist in the repository, the recorded 404 reports resourceType 'package' and resourceId the package name, with a message naming the package and its namespace.
Emulator: reports resourceType 'package-version' and resourceId the requested version string '9.9.9'.
Why: resourceType/resourceId are structured members clients branch on to distinguish 'no such package' from 'package exists, version missing' — the two drive different recovery (create the package vs publish the version). The emulator makes them indistinguishable and misreports the identity.
Fix: the DescribePackageVersion not-found path — resolve the package first and classify the 404 by which level was missing.
Traces: codeartifact-resource-not-found-envelopes#task#5.

Cause 7 of 8: ListTagsForResource's not-found envelope puts the whole ARN in resourceId.
AWS: for a resourceArn naming an absent domain, the recorded 404 sets resourceId to the DOMAIN NAME and the message is the standard domain-not-found text ('Domain not found. Domain '<name>' owned by account '<acct>' does not exist.' — built from the request, so naming the domain and the owning account is the requirement).
Emulator: sets resourceId to the full ARN and emits a generic 'Resource <arn> not found.' message.
Why: resourceId is specified as the id of the resource, not the ARN; clients that feed it back into a domain-scoped call (or match it against a name) get an unusable value.
Fix: the ARN-resolution/not-found path shared by the tagging operations — report the parsed resource id, not the input ARN.
Traces: codeartifact-resource-not-found-envelopes#task#6.

Cause 8 of 8: the invalid package-group pattern refusal does not use the modeled constraint-violation message.
AWS: HTTP 400 ValidationException whose message is the standard request-derived constraint report: "1 validation error detected: Value '<value>' at 'packageGroup' failed to satisfy constraint: Pattern must start with '/'." — it names the offending member (packageGroup) and the constraint it failed (Smithy: CreatePackageGroup.packageGroup -> PackageGroupPattern, required, @length 2..520), so the rule that produces it is known and the format is the requirement.
Emulator: returns 400 (status and errortype correct) with a free-form message that names neither the member nor the constraint: "Invalid package group pattern '<value>'. A package group pattern is a path from the domain root and must begin with '/'."
Why: clients and operators parse the 'N validation error detected ... at '<member>' failed to satisfy constraint' form to attribute a failure to a specific input member; a free-form message defeats that attribution for every multi-member request.
Fix: the input-validation layer for CreatePackageGroup — emit the standard constraint-violation message naming the member and constraint.
Traces: codeartifact-package-group-hierarchy-and-association#task#11.

## Evidence
Cause 1 — codeartifact-generic-publish-validation-envelopes#task#4, PublishPackageVersion:
  status: expected 400, got 200
  $headers.x-amzn-errortype: expected 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/', got None
  $.message: expected "'namespace' is required when 'format' is 'generic'.", got None

Cause 2 — codeartifact-generic-publish-validation-envelopes#task#5, PublishPackageVersion:
  status: expected 400, got 403
  $headers.x-amzn-errortype: expected 'ValidationException:...', got 'SignatureDoesNotMatch'
  $.message: expected "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '<declared>', Actual hash '<computed>'", got 'The request signature does not match'

Cause 3 — codeartifact-generic-publish-validation-envelopes#task#6, PublishPackageVersion, 409/409:
  $.resourceId: expected 'sim2real/sim2real-probe-pkg-fbfc55f2/1.0.0', got '1.0.0'
  ($.message also differs; the recorded text is an example)

Cause 4 — codeartifact-external-connection-lifecycle#task#5, AssociateExternalConnection:
  status: expected 400, got 402
  $headers.x-amzn-errortype: expected 'ValidationException:...', got 'ServiceQuotaExceededException'

Cause 5 — codeartifact-repository-error-paths#task#7, AssociateExternalConnection:
  status: expected 404, got 400
  $headers.x-amzn-errortype: expected <not asserted by the recording>, got 'ValidationException'

Cause 6 — codeartifact-resource-not-found-envelopes#task#5, DescribePackageVersion, 404/404:
  $.resourceType: expected 'package', got 'package-version'
  $.resourceId: expected 'sim2real-probe-pkg-fbfc55f2', got '9.9.9'

Cause 7 — codeartifact-resource-not-found-envelopes#task#6, ListTagsForResource, 404/404:
  $.resourceId: expected 'sim2real-probe-ca-absentdom15-fbfc55f2', got 'arn:aws:codeartifact:us-west-2:000000000000:domain/sim2real-probe-ca-absentdom15-fbfc55f2'

Cause 8 — codeartifact-package-group-hierarchy-and-association#task#11, CreatePackageGroup, 400/400:
  $.message: expected "1 validation error detected: Value 'sim2real-probe-not-a-pattern' at 'packageGroup' failed to satisfy constraint: Pattern must start with '/'.", got "Invalid package group pattern 'sim2real-probe-not-a-pattern'. A package group pattern is a path from the domain root and must begin with '/'."

## Steps to Reproduce
1. Replay codeartifact-generic-publish-validation-envelopes step task#4: POST /v1/package/version/publish with format=generic and no namespace -> 200 instead of 400 ValidationException.
2. Step task#5 of the same fixture: publish with an assetSHA256 that does not match the body -> 403 SignatureDoesNotMatch instead of 400 ValidationException.
3. Step task#6: republish an existing Published version -> 409 whose resourceId is '1.0.0' instead of '<namespace>/<package>/<version>'.
4. Replay codeartifact-external-connection-lifecycle step task#5: associate a second external connection -> 402 ServiceQuotaExceededException instead of 400 ValidationException.
5. Replay codeartifact-repository-error-paths step task#7: associate externalConnection 'public:does-not-exist' -> 400 instead of 404.
6. Replay codeartifact-resource-not-found-envelopes step task#5 (DescribePackageVersion for a version of a package that was never published) and step task#6 (ListTagsForResource on an absent domain's ARN) -> wrong resourceType/resourceId.
7. Replay codeartifact-package-group-hierarchy-and-association step task#11: CreatePackageGroup with packageGroup 'sim2real-probe-not-a-pattern' -> non-standard validation message.
