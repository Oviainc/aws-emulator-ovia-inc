Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-access-point-name-validation-splits-into-two-identities
Issue-Step: teardown#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-access-point-name-validation-splits-into-two-identities s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code s3c-access-point-public-access-block-is-replaced-not-merged s3c-account-id-host-prefix-is-authorized-before-the-request s3control-access-point-lifecycle-and-policy s3control-access-point-policy-status-envelopes s3control-access-grants-grant-lifecycle-and-location-referential-integrity s3control-access-grants-location-lifecycle s3control-access-grants-instance-resource-policy-lifecycle s3control-account-public-access-block-lifecycle
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|DeleteAccessPoint|2xx->2xx|status
Issue-Key: s3control|PutAccessPointPolicy|2xx->2xx|status
Issue-Key: s3control|DeleteAccessPointPolicy|2xx->2xx|status
Issue-Key: s3control|DeleteAccessGrant|2xx->2xx|status
Issue-Key: s3control|DeleteAccessGrantsLocation|2xx->2xx|status
Issue-Key: s3control|DeleteAccessGrantsInstanceResourcePolicy|2xx->2xx|status
Issue-Key: s3control|DeletePublicAccessBlock|2xx->2xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Successful s3control deletions and policy puts answer 200 instead of 204

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAccessPoint

## Description
7 net-new root causes in the s3control crate, all the same fault: operations whose AWS answer is an empty body are served with HTTP 200 where the recorded wire answers 204 No Content. One cause per operation family because each is a separate handler's success path; the status is the only diverging field in each.

Shared AWS note: the on-disk Smithy model declares http {code: 200} for these operations (e.g. Smithy: DeleteAccessPoint http {method: DELETE, uri /v20180820/accesspoint/{Name}, code 200}; PutAccessPointPolicy output_shape Unit) — the model's success code is stale for the empty-body members of this service. The requirement here is recorded behavior only: the wire answers 204 with no body.

--- Cause 1 of 7: DeleteAccessPoint ---
AWS: recorded behavior only — deleting an existing access point answers 204 No Content with an empty body.
Emulator: answers 200 with an empty body.
Why: a client that distinguishes 204 (deleted, nothing to read) from 200 (a body is expected) will try to parse a body that is not there, and SDK/HTTP middleware that asserts the modeled success status on the wire disagrees with the real service.
Fix: the DeleteAccessPoint handler's success path in the s3control crate — return 204 for the empty-body success.
Traces: s3c-access-point-name-validation-splits-into-two-identities#teardown#9 (8 steps in cluster).

--- Cause 2 of 7: PutAccessPointPolicy ---
AWS: recorded behavior only — a successful policy put answers 204 No Content.
Emulator: answers 200.
Why: same client-side mismatch; a successful put is indistinguishable from an answer that should carry a body.
Fix: the PutAccessPointPolicy handler's success path.
Traces: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#13 (5 steps in cluster).

--- Cause 3 of 7: DeleteAccessPointPolicy ---
AWS: recorded behavior only — 204 No Content.
Emulator: answers 200.
Why: as above.
Fix: the DeleteAccessPointPolicy handler's success path.
Traces: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#teardown#17 (4 steps in cluster).

--- Cause 4 of 7: DeleteAccessGrant ---
AWS: recorded behavior only — 204 No Content.
Emulator: answers 200.
Why: as above.
Fix: the DeleteAccessGrant handler's success path.
Traces: s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#11.

--- Cause 5 of 7: DeleteAccessGrantsLocation ---
AWS: recorded behavior only — 204 No Content.
Emulator: answers 200.
Why: as above.
Fix: the DeleteAccessGrantsLocation handler's success path.
Traces: s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#13 (2 steps in cluster).

--- Cause 6 of 7: DeleteAccessGrantsInstanceResourcePolicy ---
AWS: recorded behavior only — 204 No Content.
Emulator: answers 200.
Why: as above.
Fix: the DeleteAccessGrantsInstanceResourcePolicy handler's success path.
Traces: s3control-access-grants-instance-resource-policy-lifecycle#task#5.

--- Cause 7 of 7: DeletePublicAccessBlock ---
AWS: recorded behavior only — 204 No Content.
Emulator: answers 200.
Why: as above.
Fix: the DeletePublicAccessBlock handler's success path.
Traces: s3control-account-public-access-block-lifecycle#task#4.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator. Every cluster's only detail is `status: expected 204, got 200`:
- DeleteAccessPoint: expected 204, got 200 (s3c-access-point-name-validation-splits-into-two-identities#teardown#9, 8 affected steps).
- PutAccessPointPolicy: expected 204, got 200 (s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#13, 5 affected steps).
- DeleteAccessPointPolicy: expected 204, got 200 (same fixture, teardown#17, 4 affected steps).
- DeleteAccessGrant: expected 204, got 200 (s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#11).
- DeleteAccessGrantsLocation: expected 204, got 200 (same fixture, task#13, 2 affected steps).
- DeleteAccessGrantsInstanceResourcePolicy: expected 204, got 200 (s3control-access-grants-instance-resource-policy-lifecycle#task#5).
- DeletePublicAccessBlock: expected 204, got 200 (s3control-account-public-access-block-lifecycle#task#4).
Bodies matched (empty) in every case; only the status line diverges.

## Steps to Reproduce
1. Boot the emulator and replay fixture s3c-access-point-name-validation-splits-into-two-identities, step teardown#9: create an access point, then DELETE /v20180820/accesspoint/{Name} with x-amz-account-id set. Observe HTTP 200; the recording has 204.
2. Repeat with the other bundled steps: PUT and DELETE /v20180820/accesspoint/{Name}/policy, DELETE /v20180820/accessgrantsinstance/grant/{id}, DELETE /v20180820/accessgrantsinstance/location/{id}, DELETE /v20180820/accessgrantsinstance/resourcepolicy and DELETE /v20180820/configuration/publicAccessBlock. Each answers 200 where the recording answers 204.
