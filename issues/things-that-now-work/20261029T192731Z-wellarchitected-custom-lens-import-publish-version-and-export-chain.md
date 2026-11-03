Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-custom-lens-import-version-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-custom-lens-import-version-lifecycle
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|GetLens|4xx->2xx|$.Message
Issue-Key: wellarchitected|GetLens|4xx->2xx|$.ResourceId
Issue-Key: wellarchitected|GetLens|4xx->2xx|$.ResourceType
Issue-Key: wellarchitected|GetLens|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|GetLens|4xx->2xx|status
Issue-Key: wellarchitected|ExportLens|2xx->4xx|$.LensJSON
Issue-Key: wellarchitected|ExportLens|2xx->4xx|status
Issue-Key: wellarchitected|CreateLensVersion|2xx->4xx|$.LensArn
Issue-Key: wellarchitected|CreateLensVersion|2xx->4xx|$.LensVersion
Issue-Key: wellarchitected|CreateLensVersion|2xx->4xx|status
Issue-Key: wellarchitected|CreateLensVersion|4xx->4xx|$.Message
Issue-Key: wellarchitected|CreateLensVersion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|CreateLensVersion|4xx->4xx|status
Issue-Key: wellarchitected|GetLens|2xx->2xx|$.Lens.LensVersion
Issue-Key: wellarchitected|GetLens|2xx->2xx|$.Lens.ShareInvitationId
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# Custom lens import, publish, version and export chain is not addressable after ImportLens

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLens

## Description
This bundle has 5 causes. After ImportLens stores a custom lens, the emulator's lens store does not model the DRAFT/PUBLISHED split the way AWS does and does not resolve the imported lens on the version/export paths, so the whole publish-and-version lifecycle diverges.

Cause 1 of 5: GetLens on an imported-but-unpublished lens returns the lens instead of ResourceNotFound.
AWS: GetLens (GET /lenses/{LensAlias}, Smithy: errors ResourceNotFoundException httpError 404) treats a custom lens that has been imported but not published as not retrievable: it answers 404 with ResourceNotFoundException{Message, ResourceId, ResourceType}. ResourceType is 'LENS', ResourceId is the lens id from the request path, and the message is the fixed text '[ResourceNotFound] LENS with id <lensId> not found. Lens is not published.' — fixed except for <lensId>, which is the lens id taken from the request.
Emulator: returns 200 and serves the draft lens; no Message/ResourceId/ResourceType and no x-amzn-errortype header.
Why: a client uses GetLens to decide whether a lens is published yet; a 200 makes an unpublished lens look usable, and the publish step is skipped.
Fix: GetLens handler's lens-status gate (refuse a lens whose only version is DRAFT) and the ResourceNotFoundException serializer.
Traces: wellarchitected-custom-lens-import-version-lifecycle#task#2.

Cause 2 of 5: ExportLens cannot find the imported lens.
AWS: ExportLens (GET /lenses/{LensAlias}/export) returns 200 with ExportLensOutput$LensJSON, the stored lens document for the requested lens/version.
Emulator: returns 404 and no LensJSON.
Why: a client cannot read back the lens it just imported, so round-tripping or diffing a custom lens is impossible.
Fix: ExportLens handler's lens-alias resolution against the ImportLens-created lens.
Traces: wellarchitected-custom-lens-import-version-lifecycle#task#3.

Cause 3 of 5: CreateLensVersion cannot find the imported lens.
AWS: CreateLensVersion (POST /lenses/{LensAlias}/versions) returns 200 with LensArn (the custom lens's account-scoped ARN, 'arn:aws:wellarchitected:<region>:<accountId>:lens/<lensId>') and LensVersion (the version name supplied in the request).
Emulator: returns 404 and neither member.
Why: a custom lens can never be published, so nothing downstream (sharing, association, review) can use it.
Fix: CreateLensVersion handler's lens lookup/state transition.
Traces: wellarchitected-custom-lens-import-version-lifecycle#task#4.

Cause 4 of 5: re-publishing an existing version answers ResourceNotFound 404 instead of Conflict 409.
AWS: CreateLensVersion (Smithy errors include ConflictException httpError 409) refuses a version name that already exists for that lens with 409 ConflictException; the message is the fixed text '[Conflict] A lens <lensArn> with version name <lensVersion> already exists.' with <lensArn> the lens ARN and <lensVersion> the requested version name taken from the request.
Emulator: returns 404 with x-amzn-errortype 'ResourceNotFoundException' and '[ResourceNotFound] No custom lens with alias <lensArn> was found in account <accountId>.'
Why: a client distinguishes 'already published' (retry/alias is fine) from 'no such lens' (hard error) by the error class; the emulator collapses them, and the wrong class is also non-retryable in the wrong direction.
Fix: CreateLensVersion handler — resolve the lens first, then raise ConflictException on a duplicate version name.
Traces: wellarchitected-custom-lens-import-version-lifecycle#task#5.

Cause 5 of 5: GetLens reports the lens still at DRAFT and omits ShareInvitationId.
AWS: GetLensOutput$Lens (Lens{LensArn, LensVersion, Name, Description, Owner, ShareInvitationId, Tags}) reports LensVersion as the published version name after CreateLensVersion, and emits ShareInvitationId (empty when the lens was not reached through a share invitation) rather than omitting it.
Emulator: returns 200 with Lens.LensVersion 'DRAFT' and Lens.ShareInvitationId absent.
Why: a client reading the version off GetLens sees the lens as unpublished immediately after publishing it, and a client reading ShareInvitationId dereferences a missing member.
Fix: the lens store's version bookkeeping on publish, plus the Lens serializer.
Traces: wellarchitected-custom-lens-import-version-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1, 5 mismatch clusters, all service=wellarchitected, all fixture wellarchitected-custom-lens-import-version-lifecycle.

Cause 1 — task#2, GetLens, expected 404, got 200: $.ResourceId expected 'e510af8d62dacf03c0217f149e5c483b', got None; $.ResourceType expected 'LENS', got None; $.Message expected '[ResourceNotFound] LENS with id e510af8d62dacf03c0217f149e5c483b not found. Lens is not published.', got None; $headers.x-amzn-errortype expected (shaped), got None. (Fixed text: '[ResourceNotFound] LENS with id ', ' not found. Lens is not published.'; variable: the lens id.)

Cause 2 — task#3, ExportLens, expected 200, got 404: $.LensJSON expected the imported lens document (schemaVersion 2021-11-01, one pillar, one question, two choices, two riskRules), got None.

Cause 3 — task#4, CreateLensVersion, expected 200, got 404: $.LensArn expected 'arn:aws:wellarchitected:us-west-2:000000000000:lens/e510af8d62dacf03c0217f149e5c483b', got None; $.LensVersion expected '1.0', got None.

Cause 4 — task#5, CreateLensVersion, expected 409, got 404: $.Message expected '[Conflict] A lens arn:aws:wellarchitected:us-west-2:000000000000:lens/e510af8d62dacf03c0217f149e5c483b with version name 1.0 already exists.', got '[ResourceNotFound] No custom lens with alias arn:aws:wellarchitected:us-west-2:000000000000:lens/e510af8d62dacf03c0217f149e5c483b was found in account 000000000000.'; $headers.x-amzn-errortype expected (shaped), got 'ResourceNotFoundException'.

Cause 5 — task#6, GetLens, 200/200: $.Lens.LensVersion expected '1.0', got 'DRAFT'; $.Lens.ShareInvitationId expected '' (present), got absent.

Smithy: GetLens GET /lenses/{LensAlias} -> 200, errors ResourceNotFoundException(404)/ValidationException(400); ExportLens GET /lenses/{LensAlias}/export -> ExportLensOutput$LensJSON; CreateLensVersion POST /lenses/{LensAlias}/versions -> {LensArn, LensVersion}, errors include ConflictException httpError 409; ResourceNotFoundException{Message, ResourceId, ResourceType}; ConflictException{Message, ResourceId, ResourceType}; Lens{LensArn, LensVersion, Name, Description, Owner, ShareInvitationId, Tags}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 and look at fixture wellarchitected-custom-lens-import-version-lifecycle.
3. PUT /importLens with a valid custom lens document, then GET /lenses/{lensId} before publishing — the emulator answers 200 instead of 404 ResourceNotFoundException with ResourceType 'LENS' (cause 1).
4. GET /lenses/{lensId}/export — 404 instead of 200 with LensJSON (cause 2).
5. POST /lenses/{lensId}/versions with LensVersion '1.0' — 404 instead of 200 with LensArn/LensVersion (cause 3).
6. POST the same version again — 404 ResourceNotFoundException instead of 409 ConflictException (cause 4).
7. GET /lenses/{lensId} after publishing — Lens.LensVersion reads 'DRAFT' and ShareInvitationId is absent (cause 5).
