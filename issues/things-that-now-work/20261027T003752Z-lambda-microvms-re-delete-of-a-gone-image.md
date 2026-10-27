Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.deleted-image-name-is-released-for-reuse lambda-microvms.image-build-fans-out-one-build-per-chipset-generation lambda-microvms.managed-image-read-through-customer-api-is-rejected
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms.deleted-image-name-is-released-for-reuse|_client_error#16|AssertionError|raise AssertionError("the call unexpectedly SUCCEEDED where real AWS rejects it")
Issue-Key: lambda-microvms.image-build-fans-out-one-build-per-chipset-generation|run#12|AssertionError|assert all(isinstance(g, str) and g.isdigit() for g in generations), (
Issue-Key: lambda-microvms.managed-image-read-through-customer-api-is-rejected|run#18|AssertionError|assert code == "AccessDeniedException", (
Issue-Commit: d558c4fc1331790598d7d8dd6ea4bd673681392c

# lambda-microvms: re-delete of a gone image succeeds instead of 404, build chipsetGeneration is not a numeric generation, and managed ARNs are 404 instead of 403 on customer reads

> _Note: this issue was identified against the code base at commit `d558c4fc1331790598d7d8dd6ea4bd673681392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMicrovmImage

## Description
This issue bundles 3 distinct root causes in the lambda-microvms emulator, each grounded in its own failing behavioral check. A fixer must close all three (Cause 1 of 3, Cause 2 of 3, Cause 3 of 3) before submitting.

=== Cause 1 of 3: DeleteMicrovmImage on an image that is already GONE returns 200/DELETED instead of refusing with ResourceNotFoundException ===
Failing check: lambda-microvms.deleted-image-name-is-released-for-reuse

AWS: DeleteMicrovmImage is modeled with ResourceNotFoundException (errorKind client, httpError 404, message @required, "The specified resource does not exist.") among its errors (Smithy: DeleteMicrovmImage errors = AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400; http DELETE /2025-09-09/microvm-images/{imageIdentifier}, success code 200). The operation's own model doc calls the delete idempotent ("deleting an image that has already been deleted succeeds without error"), but that doc describes only the window in which the record still exists in DELETING. Recorded behavior (the check states it explicitly, us-west-2, 2026-09-11, ~65 s from DELETING to gone) is the opposite once the record is REALLY gone: a DELETE addressed to an identifier that no longer resolves is refused with ResourceNotFoundException and the message "MicroVM image not found: <imageIdentifier>". In that template "MicroVM image not found: " is FIXED text and <imageIdentifier> is the value the request carried in the {imageIdentifier} label - never a literal from one recording. Note this is a DIFFERENT wording from the read path's not-found message, so the delete path needs its own message rather than reusing GetMicrovmImage's.

Emulator: the handler treats an unresolvable identifier as success. services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/delete_microvm_image.rs lines 178-195: `match store.image_mut(&input.image_identifier) { Some(image) => ..., None => crate::state::IMAGE_STATE_DELETED.to_string() }` - the `None` arm synthesizes state DELETED and returns a 200 DeleteMicrovmImageOutput. Replay: the earlier assertions of the check all pass (the first delete answers DELETING, the image stays readable in DELETING, then GetMicrovmImage starts answering ResourceNotFoundException and the image leaves ListMicrovmImages), and the check then fails on the re-delete with "AssertionError: the call unexpectedly SUCCEEDED where real AWS rejects it" (check_lambda_microvms_deleted_image_name_is_released_for_reuse.py:60 via _client_error():159).

Why: a client cannot tell "the delete I issued is in flight" from "there is nothing here". Reconciliation and cleanup loops use the 404 as the terminal signal that a resource is fully gone and that its name is reusable; against the emulator they get an unconditional 200 with a state the service never actually held, so a delete of a typo'd or foreign identifier is reported as a successful deletion of a resource that was never touched.

Fix: DeleteMicrovmImage's handler - the identifier-resolution branch in services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/delete_microvm_image.rs - must return the modeled ResourceNotFoundException (404) with the delete path's own message template for an identifier that resolves to no record, keeping the idempotent 200/DELETING answer only while the record still exists with a deletion in flight. The message belongs beside the other message builders in crate::state (next to image_not_found_message).

Traces: lambda-microvms.deleted-image-name-is-released-for-reuse. NOTE for the fixer: this check's remaining assertions were never reached and are therefore UNVERIFIED - after the 404 it also requires that a replacement image created with the SAME name is issued the SAME imageArn (the ARN is derived from the name), that its createdAt is strictly after the deleted one's, that it starts in CREATING, and that an independent GetMicrovmImage confirms both. Expect those to need work too. Citations: ops/delete_microvm_image.rs:178-195; crate::state::image_not_found_message (src/state.rs:178-180) for the read path's distinct wording.

=== Cause 2 of 3: MicrovmImageBuildSummary$chipsetGeneration reports a chipset-family string ("GRAVITON_2") instead of a numeric generation ===
Failing check: lambda-microvms.image-build-fans-out-one-build-per-chipset-generation

AWS: MicrovmImageBuildSummary models `chipset` (Chipset enum, @required, "The target chipset for the build.") and `chipsetGeneration` separately (NonBlankString, @required, len 1..2048, pattern ^[^\s]+$, "The target chipset generation for the build."). The model publishes no value list for chipsetGeneration, so this is recorded behavior: the chipset NAME lives in `chipset` (GRAVITON) and `chipsetGeneration` carries only the GENERATION NUMBER as a decimal string - the recorded fan-out (us-west-2, 2026-09-11) produced generations {'3', '4'} with architecture ARM_64 and chipset GRAVITON on every build. The requirement is the FORM: chipsetGeneration is a numeric string (digits only), one distinct value per build in the fan-out; the particular numbers in any one recording are values, not part of the contract.

Emulator: the generation is spelled as a chipset-family token. services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs lines 562-573 define BUILD_TARGETS with `chipset: "GRAVITON", chipset_generation: "GRAVITON_2"` and `chipset: "GRAVITON", chipset_generation: "GRAVITON_3"`, and state.rs:1153-1168 (`mint_builds`) copies `target.chipset_generation` straight onto each MicrovmImageBuild. Replay: "AssertionError: builds covered chipset generations ['GRAVITON_2', 'GRAVITON_3'], expected numeric generation strings" (check_lambda_microvms_image_build_fans_out_one_build_per_chipset_generation.py:53). The fan-out itself is correct (two distinct builds, one per generation, both ARM_64/GRAVITON) - only the generation's encoding diverges.

Why: chipsetGeneration is a per-build selector a client filters on - ListMicrovmImageBuilds takes `chipsetGeneration` as an httpQuery parameter. A caller that was told '3' by AWS and asks the emulator for generation '3' gets an empty page, and any client that parses or compares the generation numerically (to require "generation >= 3", or to render "Graviton gen 3") fails on 'GRAVITON_2'. It also duplicates the chipset family into a member whose sibling `chipset` already carries it.

Fix: the build-target table in the service state - BUILD_TARGETS in services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state.rs - must carry the generation as a bare decimal string, leaving the family in `chipset`. Every surface that projects it (ListMicrovmImageBuilds / GetMicrovmImageBuild, and the `chipsetGeneration` httpQuery filter on ListMicrovmImageBuilds) then matches on the same form.

Traces: lambda-microvms.image-build-fans-out-one-build-per-chipset-generation. Citations: src/state.rs:548-573 (BuildTarget + BUILD_TARGETS); src/state.rs:1138-1170 (mint_builds).

=== Cause 3 of 3: customer-facing reads of an AWS-managed image ARN answer 404 ResourceNotFoundException instead of 403 AccessDeniedException ===
Failing check: lambda-microvms.managed-image-read-through-customer-api-is-rejected

AWS: the ARNs ListManagedMicrovmImages returns are AWS-owned - the account segment is literally `aws` (arn:<partition>:lambda:<region>:aws:microvm-image:<name>), and the check verifies that segment on every managed item before probing. Those images EXIST, so the customer-facing read paths hide them behind an authorization refusal rather than reporting absence: AccessDeniedException (Smithy: errorKind client, httpError 403, "You do not have sufficient access to perform this action."), which is modeled on BOTH GetMicrovmImage and ListMicrovmImageVersions. Recorded behavior: GetMicrovmImage and ListMicrovmImageVersions on a managed ARN both answer AccessDeniedException/403; the message names the calling principal's own ARN, so its variable part is caller identity and only the code and the 403 status are required (the check asserts the code only). The complementary recorded claim in the same check: the managed catalogue's OWN ListManagedMicrovmImageVersions answers an ABSENT managed ARN with an empty page ({"items": []}), not an error.

Emulator: GetMicrovmImage resolves the managed ARN through crate::state::resolve_image_reference, which finds nothing in the account's own store, falls into ImageReference::Absent and returns ResourceNotFoundException (404) built by crate::state::image_not_found_message - services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/get_microvm_image.rs lines 180-208, which has no managed-namespace branch at all. Replay: "AssertionError: GetMicrovmImage on the managed ARN 'arn:aws:lambda:us-west-2:aws:microvm-image:al2023-1' answered 'ResourceNotFoundException', expected AccessDeniedException". Everything before that assertion passed (the managed catalogue is readable through its own operations, every managed ARN's account segment is `aws`, ListManagedMicrovmImageVersions returns versions, the newly created customer image appears in ListMicrovmImages and the two namespaces do not leak into each other), so the divergence is confined to the authorization verdict.

Why: 403 and 404 mean different things. A caller handed 404 for the very base image the service just advertised concludes that base image does not exist and aborts or retries forever, instead of recognizing a permission boundary it is never meant to cross; SDK error-classification and retry layers also treat the two differently.

Fix: the customer-facing read handlers that accept a MicrovmImageIdentifier - GetMicrovmImage's and ListMicrovmImageVersions' handlers - must classify an identifier in the AWS-owned managed namespace BEFORE the own-account lookup and return the modeled AccessDeniedException (403). The predicate already exists in crate::state (the managed-namespace check CreateMicrovmImage uses for baseImageArn), so this is a branch ordering fix in the handlers, not new state.

RELATION TO AN EXISTING OPEN ISSUE: 20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md, Cause 3 of 3, describes the same 403-vs-404 defect for GetMicrovmImage (grounded in lambda-microvms.representation-managed-microvm-image-fields-round-trip). The replay report did NOT mark the present check as owned by it, and this check carries claims that issue does not: the SAME refusal is required from ListMicrovmImageVersions on a managed ARN, and ListManagedMicrovmImageVersions must answer an ABSENT managed ARN with an empty page rather than an error. A fixer closing that issue's Cause 3 should extend it to those two surfaces and then re-replay this check.

Traces: lambda-microvms.managed-image-read-through-customer-api-is-rejected. The assertions after the failure (ListMicrovmImageVersions -> AccessDeniedException, ListManagedMicrovmImageVersions on an absent managed ARN -> {"items": []}) were never reached and are UNVERIFIED. Citations: ops/get_microvm_image.rs:180-208 (Absent -> ResourceNotFoundException, no managed-namespace branch); src/state.rs:178-180 (image_not_found_message); src/state.rs:205-215 (base_image_not_found_message, the managed-namespace wording that already exists).

## Evidence
Behavioral suite: 2/36 passed (5.6%); lambda-microvms 2/34, 32 failed. Of the failing clusters, 4 are marked ALREADY FILED against unimplemented scaffold stubs (GetMicrovmImageVersion - blocking 23 checks, UntagResource, UpdateMicrovmImage, and EC2's DescribeVpcEndpointServices) and 4 are marked OWNED by 20261025T210105Z-lambda-microvms-image-builds-never-fail-image-store.md; the 3 remaining clusters are the 3 causes above.

Cause 1 of 3 - re-delete of a gone image is not refused:
  Expected (AWS, recorded): DeleteMicrovmImage(imageIdentifier=<arn of an image whose deletion has completed>) -> ResourceNotFoundException, HTTP 404, message "MicroVM image not found: <imageIdentifier>" ("MicroVM image not found: " fixed; <imageIdentifier> from the request's URI label).
  Actual (emulator): HTTP 200 with {imageIdentifier: <echoed>, state: "DELETED"}.
    check_lambda_microvms_deleted_image_name_is_released_for_reuse.py:60 -> _client_error():159
    AssertionError: the call unexpectedly SUCCEEDED where real AWS rejects it
  Passing preconditions in the same check (so the divergence is the refusal, not the delete): first DeleteMicrovmImage returned state DELETING; GetMicrovmImage immediately after still read DELETING; the image then became unreadable (404) and left ListMicrovmImages.
  Emulator source: ops/delete_microvm_image.rs:178-195, `None => crate::state::IMAGE_STATE_DELETED.to_string()`.

Cause 2 of 3 - chipsetGeneration encoding:
  Expected (AWS, recorded us-west-2 2026-09-11): one build per chipset generation, each MicrovmImageBuildSummary with architecture="ARM_64", chipset="GRAVITON", chipsetGeneration = a decimal-digit string (recorded set {'3','4'}), all buildIds distinct, GetMicrovmImageBuild returning the same (imageArn, imageVersion, chipsetGeneration) tuple ListMicrovmImageBuilds summarised.
  Actual (emulator): chipsetGeneration = ['GRAVITON_2', 'GRAVITON_3'].
    check_lambda_microvms_image_build_fans_out_one_build_per_chipset_generation.py:53
    AssertionError: builds covered chipset generations ['GRAVITON_2', 'GRAVITON_3'], expected numeric generation strings
  Passing preconditions: >=2 build records produced by one create, generations distinct.
  Emulator source: src/state.rs:562-573 (BUILD_TARGETS literals), src/state.rs:1153-1168 (mint_builds copies them).

Cause 3 of 3 - managed namespace answers 404 instead of 403:
  Expected (AWS): GetMicrovmImage(imageIdentifier=<ARN from ListManagedMicrovmImages, account segment `aws`>) -> AccessDeniedException, HTTP 403; ListMicrovmImageVersions(imageIdentifier=<same>) -> AccessDeniedException, HTTP 403; ListManagedMicrovmImageVersions(imageIdentifier=arn:aws:lambda:<region>:aws:microvm-image:<absent>) -> HTTP 200 {"items": []}.
  Actual (emulator): ResourceNotFoundException, HTTP 404 from GetMicrovmImage.
    check_lambda_microvms_managed_image_read_through_customer_api_is_rejected.py:67
    AssertionError: GetMicrovmImage on the managed ARN 'arn:aws:lambda:us-west-2:aws:microvm-image:al2023-1' answered 'ResourceNotFoundException', expected AccessDeniedException
  Passing preconditions: ListManagedMicrovmImages non-empty with every ARN's account segment == 'aws'; ListManagedMicrovmImageVersions on a published managed ARN returned versions; the created customer image appeared in ListMicrovmImages; no overlap in either direction between the customer and managed listings.
  Emulator source: ops/get_microvm_image.rs:180-208.

Model grounding (services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json):
  DeleteMicrovmImage: http DELETE /2025-09-09/microvm-images/{imageIdentifier}, code 200, @idempotent; errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); doc "This operation is idempotent; deleting an image that has already been deleted succeeds without error."
  ResourceNotFoundException: errorKind client, httpError 404, message @required, "The specified resource does not exist."
  AccessDeniedException: errorKind client, httpError 403, "You do not have sufficient access to perform this action."
  GetMicrovmImage errors: AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400)
  ListMicrovmImageVersions errors: AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400)
  MicrovmImageBuildSummary members: imageArn @required, imageVersion @required, buildId @required, buildState @required (BuildState enum), architecture @required (Architecture enum, "Supported value: ARM_64"), chipset @required (Chipset enum, "The target chipset for the build."), chipsetGeneration @required (NonBlankString len 1..2048 pattern ^[^\s]+$, "The target chipset generation for the build."), stateReason, createdAt @required
  ListMicrovmImageBuilds input: chipsetGeneration as httpQuery "chipsetGeneration" (NonBlankString) - the filter that must match the projected form

## Steps to Reproduce
Cause 1 of 3 (re-delete of a gone image must be refused):
1. mvm = client('lambda-microvms'); base = first ARN from ListManagedMicrovmImages; role = an IAM role that trusts lambda.amazonaws.com.
2. name = <unique>; image_arn = CreateMicrovmImage(name=name, baseImageArn=base, buildRoleArn=role, codeArtifact={'uri': 's3://'+name+'-absent/code.zip'})['imageArn'].
3. Poll GetMicrovmImage until state leaves CREATING.
4. DeleteMicrovmImage(imageIdentifier=image_arn) -> state 'DELETING' (correct today); GetMicrovmImage still reads 'DELETING' (correct today).
5. Poll GetMicrovmImage until it answers ResourceNotFoundException, and confirm the name is absent from ListMicrovmImages (correct today).
6. DeleteMicrovmImage(imageIdentifier=image_arn) a SECOND time.
   Emulator: HTTP 200, state='DELETED'. AWS: ResourceNotFoundException, HTTP 404, message 'MicroVM image not found: <image_arn>'.
7. (Unverified downstream, expect to fix too) CreateMicrovmImage with the SAME name -> AWS issues the SAME imageArn as the deleted image, createdAt strictly greater than the original's, state 'CREATING'; an independent GetMicrovmImage(image_arn) confirms both.

Cause 2 of 3 (chipsetGeneration form):
1. mvm = client('lambda-microvms'); base = first ARN from ListManagedMicrovmImages.
2. created = CreateMicrovmImage(name=<unique>, baseImageArn=base, buildRoleArn=<role>, codeArtifact={'uri': 's3://<unique>-absent/code.zip'}, tags={...}); image_arn, version = created['imageArn'], created['imageVersion'].
3. ListMicrovmImageBuilds(imageIdentifier=image_arn, imageVersion=version) (page through nextToken).
   Emulator: chipsetGeneration values 'GRAVITON_2', 'GRAVITON_3'. AWS: decimal-digit strings, one per generation (recorded '3' and '4'), with chipset='GRAVITON' and architecture='ARM_64' on each.
4. ListMicrovmImageBuilds(imageIdentifier=image_arn, imageVersion=version, chipsetGeneration='3') -- must select the matching build; against the emulator no value a client learned from AWS can match.
5. GetMicrovmImageBuild(imageIdentifier, imageVersion, buildId) for each build -- must return the same (imageArn, imageVersion, chipsetGeneration) tuple the listing summarised.

Cause 3 of 3 (managed namespace authorization):
1. items = client('lambda-microvms').ListManagedMicrovmImages()['items']; arn = sorted(i['imageArn'] for i in items)[0] (account segment is 'aws').
2. GetMicrovmImage(imageIdentifier=arn).
   Emulator: ResourceNotFoundException, HTTP 404. AWS: AccessDeniedException, HTTP 403.
3. ListMicrovmImageVersions(imageIdentifier=arn).
   AWS: AccessDeniedException, HTTP 403.
4. ListManagedMicrovmImageVersions(imageIdentifier='arn:aws:lambda:<region>:aws:microvm-image:<a-name-that-does-not-exist>').
   AWS: HTTP 200 with exactly {'items': []} -- an empty page, not an error.
