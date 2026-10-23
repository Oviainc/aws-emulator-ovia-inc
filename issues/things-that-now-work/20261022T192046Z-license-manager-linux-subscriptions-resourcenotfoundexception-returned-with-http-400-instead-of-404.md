Issue-Source: trace-sweep-divergence
Issue-Trace: license-manager-linux-subscriptions-resource-tagging-unregistered-arn-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: license-manager-linux-subscriptions-resource-tagging-unregistered-arn-not-found
Issue-Shape-Id: com.amazonaws.licensemanagerlinuxsubscriptions#LicenseManagerLinuxSubscriptions
Issue-Key: license-manager-linux-subscriptions|TagResource|4xx->4xx|status
Issue-Key: license-manager-linux-subscriptions|ListTagsForResource|4xx->4xx|status
Issue-Key: license-manager-linux-subscriptions|UntagResource|4xx->4xx|status
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# ResourceNotFoundException returned with HTTP 400 instead of 404 on tagging operations

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
All three resource-tagging operations of license-manager-linux-subscriptions signal ResourceNotFoundException with the wrong HTTP status when the resourceArn names a subscription provider that is not registered. The error body is correct in every case (the replay flagged only the `status` key as diverging), so this is a single shared defect in how this exception is mapped onto an HTTP status, not three independent handler bugs.

Cause 1 of 3: TagResource (fixture license-manager-linux-subscriptions-resource-tagging-unregistered-arn-not-found, step task#1).
AWS: TagResource is `PUT /tags/{resourceArn}` and models ResourceNotFoundException (Smithy: TagResource errors = InternalServerException, ResourceNotFoundException, ValidationException; ResourceNotFoundException is a client error whose doc is "Unable to find the requested Amazon Web Services resource."). The model carries no @httpError trait on that shape, so the model's 400 is a Smithy default (httpErrorDefaulted: true) and says nothing about the wire status; recorded behavior only: the recording responds 404 for an unregistered ARN.
Emulator: responds 400 with the same error body.
Why: a client that branches on the HTTP status — raw HTTP/REST callers, retry and alarm classification, anything treating 404 as "absent" and 4xx-other as "malformed request" — reads an unregistered ARN as a bad request instead of a missing resource, so create-if-absent and tolerate-missing flows take the wrong branch.
Fix: the status mapping used when serializing ResourceNotFoundException for this service (the tagging handlers' error serializer), which must emit 404 rather than falling through to the generic client-error 400.
Traces: license-manager-linux-subscriptions-resource-tagging-unregistered-arn-not-found#task#1.

Cause 2 of 3: ListTagsForResource (same fixture, step task#2).
AWS: `GET /tags/{resourceArn}`, Smithy: ListTagsForResource errors = InternalServerException, ResourceNotFoundException, ValidationException; httpError defaulted as above. Recorded behavior only: 404 for an unregistered ARN.
Emulator: responds 400.
Why: same as cause 1 — a read of tags for a resource that does not exist is reported as a malformed request.
Fix: same error-status mapping, applied to this operation's path.
Traces: license-manager-linux-subscriptions-resource-tagging-unregistered-arn-not-found#task#2.

Cause 3 of 3: UntagResource (same fixture, step task#3).
AWS: `DELETE /tags/{resourceArn}`, Smithy: UntagResource errors = InternalServerException, ResourceNotFoundException; httpError defaulted as above. Recorded behavior only: 404 for an unregistered ARN.
Emulator: responds 400.
Why: same as cause 1; idempotent untag-if-present logic keyed on 404 breaks.
Fix: same error-status mapping, applied to this operation's path.
Traces: license-manager-linux-subscriptions-resource-tagging-unregistered-arn-not-found#task#3.

There are 3 causes in this issue; all three must be closed before submitting. No existing open issue covers this service.

## Evidence
Replay of trace set simparity-wire-aws-v1 (9 fixtures, 14 steps, 9 passed, 5 mismatched, score 64.3%; bootstrap_signal false, state_reset_failures 0, no stub_path and no owned_by on any of these clusters).

Expected vs actual, all with detail "status: expected 404, got 400" and divergence keys=status (body matched):
- TagResource: expected HTTP 404, actual HTTP 400. divergence_key service=license-manager-linux-subscriptions|TagResource|status=404->400|keys=status
- ListTagsForResource: expected HTTP 404, actual HTTP 400. divergence_key service=license-manager-linux-subscriptions|ListTagsForResource|status=404->400|keys=status
- UntagResource: expected HTTP 404, actual HTTP 400. divergence_key service=license-manager-linux-subscriptions|UntagResource|status=404->400|keys=status

The requirement being missed is the HTTP status (404) for ResourceNotFoundException; the error code and message already agree with the recording, so only the status mapping is at fault. Smithy references used: operations TagResource / ListTagsForResource / UntagResource (http bindings PUT|GET|DELETE /tags/{resourceArn}) and shape ResourceNotFoundException (errorKind client, httpError 400 defaulted, i.e. unmodelled).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay fixture license-manager-linux-subscriptions-resource-tagging-unregistered-arn-not-found.
3. Step task#1 issues TagResource (PUT /tags/{resourceArn}) against a subscription-provider ARN that was never registered; step task#2 issues ListTagsForResource (GET /tags/{resourceArn}) and step task#3 issues UntagResource (DELETE /tags/{resourceArn}) against the same unregistered ARN.
4. Observe each response: the emulator returns HTTP 400 while the recording returns HTTP 404, with the ResourceNotFoundException body otherwise matching.
5. After fixing, re-replay all three steps (task#1, task#2 and task#3 of that fixture) — each must return 404.
