Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: launch-wizard.list-tags-for-resource-nonexistent-arn-not-found launch-wizard.tag-resource-exceeds-max-tags-rejected launch-wizard.untag-resource-exceeds-max-tagkeys-rejected
Issue-Shape-Id: com.amazonaws.launchwizard#LaunchWizard
Issue-Key: launch-wizard.list-tags-for-resource-nonexistent-arn-not-found|run#5|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Key: launch-wizard.tag-resource-exceeds-max-tags-rejected|run#6|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Key: launch-wizard.untag-resource-exceeds-max-tagkeys-rejected|run#6|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Launch Wizard tagging operations unusable: ListTagsForResource/TagResource/UntagResource are scaffold stubs, and modeled tag-size validation pre-empts the resource-existence check AWS applies first

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource, TagResource, UntagResource

## Description
None of the three Launch Wizard tagging operations produce AWS's error contract for a well-formed but nonexistent deployment ARN: two answer a constraint refusal that AWS never reaches, and one answers 501. 3 causes, all in launch-wizard.

Cause 1 of 3: ListTagsForResource answers 501 NotImplemented instead of the tagging-layer not-found and ARN-parse errors.
AWS: ListTagsForResource with a syntactically valid, UUID-shaped deployment ARN that names no existing deployment fails with error code NotFoundException (the shared tagging-API not-found code, distinct from the operation-specific ResourceNotFoundException the deployment reads raise), message "Resource was not found"; an ARN whose deployment id is not UUID-shaped fails ARN parsing first with error code BadRequestException, message "Invalid input resource arn". Recorded behavior only for both codes — the Smithy model declares only ResourceNotFoundException/ValidationException for this operation and defines neither NotFoundException nor BadRequestException, so the wire codes come from the recording.
Emulator: error code "NotImplemented" ("operation ListTagsForResource is scaffolded but not implemented in this emulator") for both ARNs; the handler is still the scaffold stub.
Why: a client cannot read tags at all, and its not-found vs malformed-ARN branches are never exercised — a 501 is neither catchable as NotFoundException nor as BadRequestException.
Fix: the ListTagsForResource handler — implement ARN parsing (BadRequestException for a non-UUID deployment id) ahead of the deployment lookup (NotFoundException when absent), then return the stored tag map. Traces cite the stub: services/service-com-amazonaws-launchwizard-launchwizard/src/ops/list_tags_for_resource.rs lines 140-158.
Traces: launch-wizard.list-tags-for-resource-nonexistent-arn-not-found.

Cause 2 of 3: TagResource refuses an oversized tags map with ValidationException on a nonexistent ARN, where AWS refuses the resource first.
AWS: TagResource against a well-formed, UUID-shaped deployment ARN that names no deployment fails with NotFoundException, message "Resource was not found", REGARDLESS of whether the tags map is within bounds or exceeds the modeled 200-entry cap — resource existence is checked ahead of, and dominates, map-size validation. Smithy: TagResourceInput.tags -> Tags @length 1..200, resourceArn @httpLabel required; NotFoundException is recorded behavior (not in the model).
Emulator: returns ValidationException for the 201-entry map — the modeled @length cap is enforced by the pre-handler validation walk, so the request never reaches the handler; and the handler it would reach is itself a scaffold stub returning 501 (so the within-bounds control would also diverge).
Why: error-precedence inversion. A client that keys on NotFoundException to decide "this resource is gone, stop retrying" instead sees a 400 ValidationException blaming its tag map, and a caller fixing the tag map then gets a 501 rather than the real not-found answer.
Fix: the TagResource handler plus the ordering between this service's modeled-constraint pre-validation and the resource lookup for the @httpLabel resourceArn — the existence check must run before the tags @length refusal for this operation.
Traces: launch-wizard.tag-resource-exceeds-max-tags-rejected; stub at services/service-com-amazonaws-launchwizard-launchwizard/src/ops/tag_resource.rs lines 137-155.

Cause 3 of 3: UntagResource has the identical precedence inversion for tagKeys.
AWS: UntagResource against a nonexistent (UUID-shaped) deployment ARN fails with NotFoundException, message "Resource was not found", whether tagKeys is within bounds or exceeds the modeled 200-entry list cap. Smithy: UntagResourceInput.tagKeys -> TagKeyList @length 1..200, @httpQuery tagKeys, required.
Emulator: returns ValidationException for the 201-key list (pre-handler @length refusal); the handler behind it is also a scaffold stub.
Why: same as Cause 2 — the not-found branch of a client's untag path is unreachable, and the wrong error class is reported for a request AWS refuses for a different reason.
Fix: the UntagResource handler plus the same existence-before-shape-validation ordering.
Traces: launch-wizard.untag-resource-exceeds-max-tagkeys-rejected.

## Evidence
launch-wizard.list-tags-for-resource-nonexistent-arn-not-found — expected NotFoundException (message "Resource was not found") for arn:aws:launchwizard:<region>:<account>:deployment/<uuid4>, and BadRequestException (message "Invalid input resource arn") for a non-UUID deployment id; got code 'NotImplemented'.
launch-wizard.tag-resource-exceeds-max-tags-rejected — TagResource(resourceArn=<nonexistent uuid arn>, tags={201 entries}) expected NotFoundException; got 'ValidationException'.
launch-wizard.untag-resource-exceeds-max-tagkeys-rejected — UntagResource(resourceArn=<nonexistent uuid arn>, tagKeys=[201 keys]) expected NotFoundException; got 'ValidationException'.
Model consulted: services/service-com-amazonaws-launchwizard-launchwizard/smithy-model/com-amazonaws-launchwizard-launchwizard.json (TagResourceInput.tags @length 1..200, UntagResourceInput.tagKeys @length 1..200, ListTagsForResource GET /tags/{resourceArn}; neither NotFoundException nor BadRequestException is modeled). Emulator source consulted: src/ops/tag_resource.rs and src/ops/list_tags_for_resource.rs, both returning Scaffolded::not_implemented.

## Steps to Reproduce
1. Boot the emulator; point a botocore launch-wizard client at it and build bogus_arn = f"arn:aws:launchwizard:{region}:{account}:deployment/{uuid.uuid4()}".
2. Cause 1: lw.list_tags_for_resource(resourceArn=bogus_arn) -> expect NotFoundException, observe NotImplemented; repeat with a deployment id that is not UUID-shaped -> expect BadRequestException.
3. Cause 2: lw.tag_resource(resourceArn=bogus_arn, tags={f"k{i}": f"v{i}" for i in range(201)}) -> expect NotFoundException, observe ValidationException; then the within-bounds control lw.tag_resource(resourceArn=bogus_arn, tags={"k0": "v0"}).
4. Cause 3: lw.untag_resource(resourceArn=bogus_arn, tagKeys=[f"k{i}" for i in range(201)]) -> expect NotFoundException, observe ValidationException.
