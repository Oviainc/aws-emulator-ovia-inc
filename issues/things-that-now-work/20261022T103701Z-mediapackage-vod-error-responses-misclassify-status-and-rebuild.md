Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-error-paths mediapackage-vod-packaging-group-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: mediapackage-vod|CreateAsset|4xx->4xx|$.errors
Issue-Key: mediapackage-vod|CreateAsset|4xx->4xx|$.message
Issue-Key: mediapackage-vod|CreateAsset|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediapackage-vod|CreateAsset|4xx->4xx|status
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|4xx->4xx|$.errors
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|4xx->4xx|$.message
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|4xx->4xx|status
Issue-Key: mediapackage-vod|CreatePackagingGroup|4xx->4xx|$.message
Issue-Key: mediapackage-vod|DescribeAsset|4xx->4xx|$.message
Issue-Key: mediapackage-vod|DeleteAsset|4xx->4xx|$.message
Issue-Key: mediapackage-vod|DescribePackagingGroup|4xx->4xx|$.message
Issue-Commit: adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf

# MediaPackage VOD error responses misclassify status and rebuild messages by the wrong rule

> _Note: this issue was identified against the code base at commit `adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAsset

## Description
Seven distinct causes on the MediaPackage VOD (restJson1) error paths. The emulator classifies a missing-dependency rejection as the wrong exception and HTTP status, never emits the per-member `errors` map AWS returns alongside `message`, and builds every not-found/conflict message by a different rule than AWS — AWS's messages are derived from the request identifier in a fixed form, so that form is a requirement, not a sample of free text. A fixer must close ALL SEVEN before submitting.

=== Cause 1 of 7: CreateAsset against a nonexistent packaging group answers 404 NotFoundException instead of 422 UnprocessableEntityException with an errors map ===
AWS: Smithy: CreateAsset POST /assets, errors include both UnprocessableEntityException (errorKind client, httpError 422, "The parameters sent in the request are not valid.") and NotFoundException (httpError 404). AWS treats an unresolvable packagingGroupId in the REQUEST BODY as a parameter-validation failure, not a missing addressed resource: the recording shows status 422, header x-amzn-errortype: UnprocessableEntityException, a per-member `errors` map keyed by the offending input member ({'packagingGroupId': ['does not exist']}) and a message in the member-and-constraint form built from the request ('packaging_group_id does not exist'). The `errors` member itself is recorded behavior only — the model's UnprocessableEntityException declares only Message.
Emulator: answers status 404 with x-amzn-errortype: NotFoundException, no `errors` member at all, and a message rebuilt from internal state ('PackagingGroup with Id <id> does not exist.').
Why: a client's error handling branches on the exception type and status: 422/UnprocessableEntityException means "my request body is wrong, fix the field and do not retry", 404/NotFoundException means "the thing I addressed is gone". The emulator sends the latter, so a client retries or treats its own asset id as missing instead of correcting packagingGroupId; and a client that surfaces per-field validation to a user gets no field attribution because `errors` is absent.
Fix: the CreateAsset handler's input validation in the mediapackagevod service crate — resolve packagingGroupId as a request-parameter check that raises UnprocessableEntityException/422, and the error serializer must emit the `errors` member alongside `message`.
Traces: mediapackage-vod-error-paths#task#2.

=== Cause 2 of 7: CreatePackagingConfiguration against a nonexistent packaging group answers 404 NotFoundException instead of 422 UnprocessableEntityException with an errors map ===
AWS: Smithy: CreatePackagingConfiguration POST /packaging_configurations, errors include UnprocessableEntityException (422) and NotFoundException (404). As in Cause 1, an unresolvable packagingGroupId in the request body is a 422 UnprocessableEntityException carrying x-amzn-errortype: UnprocessableEntityException, the per-member errors map ({'packagingGroupId': ['does not exist']}) and the member-form message ('packaging_group_id does not exist'); the `errors` member is recorded behavior only.
Emulator: answers 404 / NotFoundException, no errors map, message rebuilt from internal state.
Why: same as Cause 1, on the packaging-configuration create path — a client cannot tell a bad request field from a vanished resource.
Fix: the CreatePackagingConfiguration handler's input validation and the shared error serializer in the mediapackagevod service crate.
Traces: mediapackage-vod-error-paths#task#3.

=== Cause 3 of 7: the 422 for a PackagingConfiguration with more than one packaging type omits the errors map and uses a different message rule ===
AWS: Smithy: CreatePackagingConfiguration takes at most one of CmafPackage/DashPackage/HlsPackage/MssPackage (each modeled as an optional package block); violating that is UnprocessableEntityException (422). The recording shows AWS attributing the violation to the request as a whole — errors {'base': ['must include one single type of packaging settings']} — with the message built in the same resource-and-constraint form ('PackagingConfiguration must include one single type of packaging settings'). The `errors` member is recorded behavior only.
Emulator: gets the status right (422) but emits no `errors` member and a message of its own composition ('Exactly one of CmafPackage, DashPackage, HlsPackage or MssPackage must be specified.').
Why: a client that renders AWS's per-member validation errors (keying on 'base' for whole-request violations) has nothing to render, so the user sees a generic failure with no indication which part of the request to change.
Fix: the CreatePackagingConfiguration validation error construction in the mediapackagevod service crate — emit the `errors` map (base key for whole-request violations) and build the message in AWS's resource-and-constraint form.
Traces: mediapackage-vod-error-paths#task#4.

=== Cause 4 of 7: the CreatePackagingGroup duplicate-id conflict message is built by the wrong rule ===
AWS: Smithy: CreatePackagingGroup errors include UnprocessableEntityException (422, member Message jsonName message). Status and type agree with the emulator; the message is built from the request identifier in a fixed form — 'PackagingGroup with id=<requested id> already exists' — so that form (lower-case `id=` binding, trailing-period-free) is the requirement, not the recorded id value.
Emulator: answers 422 with a message rebuilt in its own form: 'A PackagingGroup with Id <id> already exists.'
Why: a client that parses the rejection to extract the conflicting id (or matches the message to decide "already exists, adopt it" versus "real validation failure") does not match, so idempotent create-or-adopt logic falls through to an error.
Fix: the CreatePackagingGroup conflict-message construction in the mediapackagevod service crate.
Traces: mediapackage-vod-error-paths#task#5.

=== Cause 5 of 7: the DescribeAsset not-found message is built by the wrong rule ===
AWS: Smithy: DescribeAsset GET /assets/{Id}, errors include NotFoundException (404, member Message jsonName message, "The requested resource does not exist."). Status and type agree; the message is derived from the requested id in the fixed form 'Asset with id=<requested id> not found'.
Emulator: answers 404 with 'Asset with Id <id> does not exist.' — a different binding form and a different terminal phrase.
Why: a client that keys retry/adopt behaviour on the not-found text, or extracts the id from it for logging and remediation, does not match AWS's form.
Fix: the NotFoundException message construction on the DescribeAsset path in the mediapackagevod service crate.
Traces: mediapackage-vod-error-paths#task#7.

=== Cause 6 of 7: the DeleteAsset not-found message is built by the wrong rule ===
AWS: Smithy: DeleteAsset DELETE /assets/{Id} (@http code 202), errors include NotFoundException (404). Status and type agree; the message is derived from the requested id in the fixed form 'Asset with id=<requested id> not found'.
Emulator: answers 404 with 'Asset with Id <id> does not exist.'
Why: same as Cause 5, on the delete path — this is a separate handler, so fixing DescribeAsset alone does not close it.
Fix: the NotFoundException message construction on the DeleteAsset path in the mediapackagevod service crate (share one message builder with Cause 5 rather than duplicating it).
Traces: mediapackage-vod-error-paths#task#8.

=== Cause 7 of 7: the DescribePackagingGroup not-found message is built by the wrong rule ===
AWS: Smithy: DescribePackagingGroup GET /packaging_groups/{Id}, errors include NotFoundException (404, member Message jsonName message). Status and type agree; the message is derived from the requested id in the fixed form 'PackagingGroup with id=<requested id> not found'.
Emulator: answers 404 with 'PackagingGroup with Id <id> does not exist.'
Why: same as Causes 5 and 6, on the packaging-group read path, which is the probe clients most often use to decide whether a group exists before creating or deleting it.
Fix: the NotFoundException message construction on the DescribePackagingGroup path in the mediapackagevod service crate.
Traces: mediapackage-vod-packaging-group-lifecycle#task#2.

Note: no open issue covers these. The only prior MediaPackage error-body issues in the repo are against mediapackagev2 (a different service crate) and are already resolved; they do not cover mediapackage-vod.

## Evidence
All seven causes are wire-replay divergences from trace set simparity-wire-aws-v1 (service mediapackage-vod, emulator up, 0 state_reset_failures).

Cause 1 — mediapackage-vod-error-paths#task#2 CreateAsset:
  status: expected 422, got 404
  $.errors: missing (expected {'packagingGroupId': ['does not exist']}, got None)
  $.message: value_mismatch (expected 'packaging_group_id does not exist', got 'PackagingGroup with Id sim2real-probe-mediapackage-vod-missing-group-cff00067 does not exist.')
  $headers.x-amzn-errortype: value_mismatch (expected 'UnprocessableEntityException', got 'NotFoundException')

Cause 2 — mediapackage-vod-error-paths#task#3 CreatePackagingConfiguration:
  status: expected 422, got 404
  $.errors: missing (expected {'packagingGroupId': ['does not exist']}, got None)
  $.message: value_mismatch (expected 'packaging_group_id does not exist', got 'PackagingGroup with Id sim2real-probe-mediapackage-vod-missing-group-cff00067 does not exist.')
  $headers.x-amzn-errortype: value_mismatch (expected 'UnprocessableEntityException', got 'NotFoundException')

Cause 3 — mediapackage-vod-error-paths#task#4 CreatePackagingConfiguration, expected 422 / actual 422:
  $.errors: missing (expected {'base': ['must include one single type of packaging settings']}, got None)
  $.message: value_mismatch (expected 'PackagingConfiguration must include one single type of packaging settings', got 'Exactly one of CmafPackage, DashPackage, HlsPackage or MssPackage must be specified.')

Cause 4 — mediapackage-vod-error-paths#task#5 CreatePackagingGroup, expected 422 / actual 422:
  $.message: value_mismatch (expected 'PackagingGroup with id=sim2real-probe-mediapackage-vod-pkggroup-cff00067 already exists', got 'A PackagingGroup with Id sim2real-probe-mediapackage-vod-pkggroup-cff00067 already exists.')

Cause 5 — mediapackage-vod-error-paths#task#7 DescribeAsset, expected 404 / actual 404:
  $.message: value_mismatch (expected 'Asset with id=sim2real-probe-mediapackage-vod-missing-asset-cff00067 not found', got 'Asset with Id sim2real-probe-mediapackage-vod-missing-asset-cff00067 does not exist.')

Cause 6 — mediapackage-vod-error-paths#task#8 DeleteAsset, expected 404 / actual 404:
  $.message: value_mismatch (expected 'Asset with id=sim2real-probe-mediapackage-vod-missing-asset-cff00067 not found', got 'Asset with Id sim2real-probe-mediapackage-vod-missing-asset-cff00067 does not exist.')

Cause 7 — mediapackage-vod-packaging-group-lifecycle#task#2 DescribePackagingGroup, expected 404 / actual 404:
  $.message: value_mismatch (expected 'PackagingGroup with id=sim2real-probe-mediapackage-vod-phantom-cb23b1c3 not found', got 'PackagingGroup with Id sim2real-probe-mediapackage-vod-phantom-cb23b1c3 does not exist.')

Model citations from services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json: UnprocessableEntityException {errorKind: client, httpError: 422, members: Message(jsonName message)}; NotFoundException {errorKind: client, httpError: 404, members: Message(jsonName message)}; both are bound to CreateAsset, CreatePackagingConfiguration, CreatePackagingGroup, DescribeAsset, DeleteAsset and DescribePackagingGroup.

The generated resource ids in the quoted messages are examples; the requirements are the statuses, the x-amzn-errortype values, the presence of the `errors` member and the request-derived message FORM ('<Resource> with id=<id> not found', '<Resource> with id=<id> already exists', '<member_name> does not exist').

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Cause 1: fixture mediapackage-vod-error-paths, step task#2 — CreateAsset with a packagingGroupId that does not exist; expect 422 / x-amzn-errortype UnprocessableEntityException with an errors map, observe 404 / NotFoundException with none.
4. Cause 2: same fixture, step task#3 — CreatePackagingConfiguration with a packagingGroupId that does not exist; same expectation, same divergence.
5. Cause 3: same fixture, step task#4 — CreatePackagingConfiguration carrying more than one of cmafPackage/dashPackage/hlsPackage/mssPackage; inspect the 422 body for the errors map and the message form.
6. Cause 4: same fixture, step task#5 — CreatePackagingGroup re-using an existing id; inspect the 422 message form.
7. Cause 5: same fixture, step task#7 — DescribeAsset on an absent asset id; inspect the 404 message form.
8. Cause 6: same fixture, step task#8 — DeleteAsset on an absent asset id; inspect the 404 message form.
9. Cause 7: fixture mediapackage-vod-packaging-group-lifecycle, step task#2 — DescribePackagingGroup on an absent group id; inspect the 404 message form.
