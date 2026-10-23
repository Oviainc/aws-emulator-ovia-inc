Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-authorization-token-and-repository-endpoint
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-authorization-token-and-repository-endpoint codeartifact-domain-error-paths codeartifact-domain-lifecycle-and-tagging codeartifact-domain-permissions-policy-workflow codeartifact-external-connection-lifecycle codeartifact-external-connection-workflow codeartifact-list-domains-pagination codeartifact-package-group-hierarchy-and-association codeartifact-package-origin-configuration-and-version-delete codeartifact-repository-lifecycle-and-conflicts codeartifact-repository-permissions-policy codeartifact-tag-resource-error-paths codeartifact-write-ops-on-missing-resources codeartifact-domain-lifecycle codeartifact-domain-repository-lifecycle codeartifact-domain-tag-lifecycle codeartifact-get-authorization-token-workflow codeartifact-list-repositories-prefix-and-pagination codeartifact-package-group-error-paths codeartifact-package-group-lifecycle-and-hierarchy codeartifact-repository-error-paths codeartifact-repository-tag-lifecycle
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|DeleteDomain|2xx->2xx|$.domain.status
Issue-Key: codeartifact|DeleteDomain|2xx->2xx|$.domain.s3BucketArn
Issue-Key: codeartifact|DeleteDomain|2xx->2xx|$.domain.encryptionKey
Issue-Key: codeartifact|DeleteDomain|2xx->4xx|$
Issue-Key: codeartifact|DeleteDomain|2xx->4xx|status
Issue-Key: codeartifact|DeleteRepository|2xx->4xx|$
Issue-Key: codeartifact|DeleteRepository|2xx->4xx|status
Issue-Key: codeartifact|DeleteDomainPermissionsPolicy|2xx->4xx|$.policy
Issue-Key: codeartifact|DeleteDomainPermissionsPolicy|2xx->4xx|status
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CodeArtifact delete operations return stale state and 404 instead of AWS's 200

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDomain

## Description
Four causes in the CodeArtifact delete paths. All four are answered requests (no unimplemented operation involved), and all four must be closed before this issue is submitted.

Cause 1 of 4: DeleteDomain echoes the domain with status 'Active' instead of 'Deleted'.
AWS: DeleteDomain returns HTTP 200 with a `domain` DomainDescription (Smithy: DeleteDomain output domain -> DomainDescription, http DELETE /v1/domain code 200), and DomainDescription.status is a DomainStatus whose only values are 'Active' and 'Deleted' (Smithy: DomainStatus enum = [Active, Deleted]). The recorded response for a successful delete carries status 'Deleted' — the echoed description reflects the post-delete state of the resource.
Emulator: returns 200 but with $.domain.status = 'Active', i.e. the description is read out before the state transition is applied (or the transition is never recorded on the echoed copy).
Why: a client that deletes a domain and asserts on the returned status (a very common teardown/confirmation pattern, and what the CLI prints) concludes the delete did not take effect, or keeps polling a resource that is already gone. Any state machine driven off DomainStatus never observes the terminal value.
Fix: the DeleteDomain handler — apply the Deleted transition to the DomainDescription it serializes, rather than echoing the pre-delete record.
Traces: codeartifact-authorization-token-and-repository-endpoint#teardown#9 (representative; 13 fixtures in this cluster), codeartifact-domain-lifecycle#teardown#4, codeartifact-list-domains-pagination#teardown#7.

Cause 2 of 4: DeleteDomain on a domain that does not exist is refused with ResourceNotFoundException.
AWS: recorded behavior only — the recording shows HTTP 200 with an empty JSON body `{}` for a DELETE of an absent domain, i.e. the operation is idempotent. The Smithy model corroborates that a not-found refusal is not part of this operation's contract: DeleteDomain's modeled errors are AccessDeniedException (403), ConflictException (409), InternalServerException (500), ThrottlingException (429) and ValidationException (400) — ResourceNotFoundException is NOT bound to DeleteDomain.
Emulator: answers HTTP 404 with `{"__type":"ResourceNotFoundException", "message": "Domain not found. Domain '<name>' owned by account '<acct>' does not exist.", "resourceId": ..., "resourceType": "domain"}` — an error shape the operation cannot return on AWS.
Why: every idempotent cleanup path breaks. Teardown code, CloudFormation-style delete retries and 'delete if present' helpers see a hard client error instead of success, and an SDK caller cannot even catch it cleanly because the error is not in the operation's modeled error set.
Fix: the DeleteDomain handler's not-found branch — a missing domain must fall through to the 200/empty-body success response instead of raising ResourceNotFoundException.
Traces: codeartifact-domain-error-paths#task#6.

Cause 3 of 4: DeleteRepository on a repository that does not exist is refused with ResourceNotFoundException.
AWS: recorded behavior only — the recording shows HTTP 200 with an empty JSON body `{}` for a DELETE of an absent repository in an existing domain (Smithy: DeleteRepository http DELETE /v1/repository code 200, output repository -> RepositoryDescription, optional).
Emulator: answers HTTP 404 ResourceNotFoundException with resourceType 'repository'.
Why: same breakage as cause 2, on the repository scope: idempotent teardown of a repository is impossible against the emulator.
Fix: the DeleteRepository handler's not-found branch.
Traces: codeartifact-repository-error-paths#task#5.

Cause 4 of 4: DeleteDomainPermissionsPolicy on a domain with no attached policy is refused with 404.
AWS: recorded behavior only — the recording shows HTTP 200 with `policy` present but carrying only `resourceArn` (the domain ARN) when there is no policy to remove (Smithy: DeleteDomainPermissionsPolicy output policy -> ResourcePolicy, whose members resourceArn/revision/document are all optional).
Emulator: answers HTTP 404 and omits `policy` entirely.
Why: a client cannot unconditionally clear a domain policy; the 'remove policy if any' step of a permissions workflow fails, and the ResourcePolicy envelope that identifies which resource was targeted is lost.
Fix: the DeleteDomainPermissionsPolicy handler — when no policy exists, return 200 with a ResourcePolicy carrying the resolved resourceArn and no revision/document.
Traces: codeartifact-domain-error-paths#task#4.

## Evidence
Cause 1 — codeartifact-authorization-token-and-repository-endpoint#teardown#9, DeleteDomain, status 200 expected / 200 actual:
  $.domain.status: expected 'Deleted', got 'Active'
  (same cluster covers 13 fixtures' teardowns; codeartifact-domain-lifecycle#teardown#4 and codeartifact-list-domains-pagination#teardown#7 show the same status divergence)

Cause 2 — codeartifact-domain-error-paths#task#6, DeleteDomain:
  status: expected 200, got 404
  body:   expected {}, got {'__type': 'ResourceNotFoundException', 'message': "Domain not found. Domain 'sim2real-probe-codeartifact-nodom-035e6f0e' owned by account '000000000000' does not exist.", 'resourceId': 'sim2real-probe-codeartifact-nodom-035e6f0e', 'resourceType': 'domain'}

Cause 3 — codeartifact-repository-error-paths#task#5, DeleteRepository:
  status: expected 200, got 404
  body:   expected {}, got {'__type': 'ResourceNotFoundException', 'message': "Repository not found. Repository 'sim2real-probe-codeartifact-caartermiss-f3d17e83' in domain 'sim2real-probe-codeartifact-caarterdom-f3d17e83' owned by account '000000000000' does not exist.", 'resourceId': 'sim2real-probe-codeartifact-caartermiss-f3d17e83', 'resourceType': 'repository'}

Cause 4 — codeartifact-domain-error-paths#task#4, DeleteDomainPermissionsPolicy:
  status: expected 200, got 404
  $.policy: expected {'resourceArn': 'arn:aws:codeartifact:us-west-2:000000000000:domain/sim2real-probe-codeartifact-domain-035e6f0e'}, got None

(The error message texts above are examples, not requirements; the requirements are the HTTP statuses, the absence/presence of the error shape, and $.domain.status reaching 'Deleted'.)

## Steps to Reproduce
1. Replay fixture codeartifact-authorization-token-and-repository-endpoint, step teardown#9: create a domain, delete it, and read $.domain.status from the DeleteDomain response -> 'Active' instead of 'Deleted'.
2. Replay codeartifact-domain-error-paths step task#6: DELETE /v1/domain?domain=<never-created-name> -> 404 ResourceNotFoundException instead of 200 {}.
3. Replay codeartifact-repository-error-paths step task#5: DELETE /v1/repository?domain=<existing>&repository=<never-created> -> 404 instead of 200 {}.
4. Replay codeartifact-domain-error-paths step task#4: on a domain with no resource policy, DELETE /v1/domain/permissions/policy -> 404 instead of 200 with policy.resourceArn.
