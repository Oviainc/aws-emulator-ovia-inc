Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gameliftstreams.disassociate-application-not-associated-rejected gameliftstreams.update-stream-group-default-application-not-found-rejected
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams.disassociate-application-not-associated-rejected|run#8|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: gameliftstreams.update-stream-group-default-application-not-found-rejected|run#11|AssertionError|assert code == "ValidationException", (
Issue-Commit: 81fc6333f2dd5c26c146222a43c6f73baa469449

# gameliftstreams: 2 control-plane defects — DisassociateApplications answers 501 NotImplemented, and UpdateStreamGroup misclassifies an unknown DefaultApplicationIdentifier as ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `81fc6333f2dd5c26c146222a43c6f73baa469449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateApplications, UpdateStreamGroup

## Description
Two net-new root causes in the gameliftstreams emulator, both grounded in failing behavioral checks. Cause 1 of 2 and Cause 2 of 2 are independent and must BOTH be closed before submitting.

=== Cause 1 of 2: DisassociateApplications is an unimplemented scaffold stub (501 NotImplemented) ===
Failing check: gameliftstreams.disassociate-application-not-associated-rejected

AWS: Smithy: DisassociateApplications is POST /streamgroups/{Identifier}/disassociations, @idempotent, input Identifier (required, Identifier, len 1..128, pattern ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^: ]*):([0-9]{12}):([^: ]*)$)$) and ApplicationIdentifiers (required, Identifiers, len 1..50); output Arn + ApplicationArns; modeled errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). Real AWS behavior exercised by the check: (a) an ApplicationIdentifiers entry that names no existing application is refused with HTTP 404 ResourceNotFoundException whose message is "Resource '<application-arn>' does not exist (Internal ID: <uuid>)" — fixed text "Resource '" + "' does not exist", the variable part being the ARN the supplied application identifier resolves to in the request's partition/region/account, and the trailing " (Internal ID: <uuid>)" being a per-request value this service appends to every error message; (b) an application that exists but is NOT associated with the stream group is an idempotent no-op that succeeds with ApplicationArns reflecting the applications still associated (empty when none remain), not a rejection; (c) disassociating a genuinely associated application removes the link from BOTH sides — the stream group drops it from AssociatedApplications and GetApplication drops the group from AssociatedStreamGroups.

Emulator: every DisassociateApplications call answers HTTP 501 with error code NotImplemented and message "operation DisassociateApplications is scaffolded but not implemented in this emulator". The check's first assertion fails with: expected ResourceNotFoundException disassociating a nonexistent application identifier, got 'NotImplemented'.

Why: a client cannot unlink an application from a stream group at all. Teardown and reconfiguration flows break: the not-found case is never refused with the modeled 404, the never-associated case never gets its idempotent success, and the real-removal case leaves the association permanently in place, so AssociatedApplications / AssociatedStreamGroups never converge. A 501 is also not in the operation's modeled error set, so SDK error handling for this operation never matches.

Fix: implement the handler body of this operation — the unlink must mutate both sides of the association in the service state, and the identifier-resolution refusal belongs in the same handler's validation prologue (the pattern already used by UpdateStreamGroup for Identifier/DefaultApplicationIdentifier).

Traces: gameliftstreams.disassociate-application-not-associated-rejected; handler still carrying the SCAFFOLDED marker and the not_implemented return at services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/ops/disassociate_applications.rs lines 4-6 and 188. No existing open issue names DisassociateApplications (the filed stub issues cover RemoveStreamGroupLocations and UntagResource only), and the replay report did not mark this check as already filed.

=== Cause 2 of 2: UpdateStreamGroup returns ResourceNotFoundException for a DefaultApplicationIdentifier that does not exist, instead of ValidationException with the not-associated message ===
Failing check: gameliftstreams.update-stream-group-default-application-not-found-rejected

AWS: Smithy: UpdateStreamGroup is PATCH /streamgroups/{Identifier}; DefaultApplicationIdentifier is a notProperty member (Identifier, len 1..128, same ARN-or-id pattern); modeled errors include both ValidationException(400) and ResourceNotFoundException(404). AWS documents that DefaultApplicationIdentifier "only sets the default application in a stream group" and that associating an application requires AssociateApplications — so AWS does NOT perform a global application lookup for this member: any identifier that is not currently associated with the target stream group (including one that names no application at all) is a bad value for the member and is refused with HTTP 400 ValidationException and the message "Application '<application-arn>' cannot be set as default because it is not currently associated with the stream group (Internal ID: <uuid>)". Fixed text: "Application '" + "' cannot be set as default because it is not currently associated with the stream group". Variable parts: the ARN the supplied DefaultApplicationIdentifier resolves to in the request's partition/region/account, and the per-request "(Internal ID: <uuid>)" suffix. The 404 ResourceNotFoundException stays reserved for the stream group named by the path Identifier. The rejected call must leave the group's DefaultApplication unset, and a plain Description update on the same group must still succeed.

Emulator: for a DefaultApplicationIdentifier that resolves to no stored application the handler takes a resource-lookup branch and answers HTTP 404 ResourceNotFoundException with the generic "Resource '<arn>' does not exist" message. Replay: AssertionError: expected ValidationException for a DefaultApplicationIdentifier that does not exist; real AWS does not resolve this as a ResourceNotFoundException lookup, got 'ResourceNotFoundException'. Secondary part of the same cause: when the application DOES exist but is not associated, the emulator does reach a ValidationException but words it "application <id> is not associated with stream group <group-id>; use AssociateApplications to associate it first", which is neither AWS's fixed text nor AWS's variable substitution (AWS names the application ARN, not the bare id, and does not name the group id).

Why: a client that distinguishes "the stream group is gone" (404, stop retrying / re-create the group) from "this application cannot be the default yet" (400, call AssociateApplications first) is routed down the wrong path, and error-message matching on the documented wording never fires. The status code is wrong too (404 vs 400), so retry/alarm classification differs.

Fix: in the UpdateStreamGroup handler, collapse the unknown-application branch into the not-associated refusal: a DefaultApplicationIdentifier that resolves to no association on the target group — whether or not an application with that identifier exists — must return ValidationException with AWS's message template built from the resolved application ARN, and the ResourceNotFoundException path must remain only for the stream group named by the path Identifier.

Traces: gameliftstreams.update-stream-group-default-application-not-found-rejected; the diverging lookup branch is services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/ops/update_stream_group.rs lines 428-442 (ResourceNotFoundException for the unknown application) and lines 463-474 (the not-associated ValidationException whose wording diverges).

## Evidence
Cause 1 of 2 — gameliftstreams.disassociate-application-not-associated-rejected [mismatch]
  Expected (AWS): DisassociateApplications(Identifier=<stream-group-arn>, ApplicationIdentifiers=[<nonexistent-app-id>]) -> HTTP 404, code ResourceNotFoundException, message "Resource '<arn:aws:gameliftstreams:<region>:<account>:application/<app-id>>' does not exist (Internal ID: <uuid>)"; then the same call for an existing-but-unassociated application -> HTTP 200 with ApplicationArns == [] (idempotent no-op); then after AssociateApplications, the call removes the link from both GetStreamGroup.AssociatedApplications and GetApplication.AssociatedStreamGroups.
  Actual (emulator): HTTP 501, code NotImplemented, message "operation DisassociateApplications is scaffolded but not implemented in this emulator".
  Replay assertion: "expected ResourceNotFoundException disassociating a nonexistent application identifier 'a-missing-3f3c911f4cef', got 'NotImplemented'" (check line 34). The id in that text is a per-run generated value, not part of the requirement.

Cause 2 of 2 — gameliftstreams.update-stream-group-default-application-not-found-rejected [mismatch]
  Setup: CreateStreamGroup(StreamClass=gen4n_high, one LocationConfiguration) polled to Status ACTIVE, DefaultApplication unset.
  Expected (AWS): UpdateStreamGroup(Identifier=<group-arn>, DefaultApplicationIdentifier=<nonexistent-app-id>) -> HTTP 400, code ValidationException, message "Application '<arn:aws:gameliftstreams:<region>:<account>:application/<app-id>>' cannot be set as default because it is not currently associated with the stream group (Internal ID: <uuid>)"; GetStreamGroup still reports no DefaultApplication; UpdateStreamGroup(Description=<new>) then succeeds and GetStreamGroup reflects the new Description.
  Actual (emulator): HTTP 404, code ResourceNotFoundException, generic message "Resource '<arn>' does not exist (Internal ID: <uuid>)".
  Replay assertion: "expected ValidationException for a DefaultApplicationIdentifier that does not exist ('a-b969cfd290b0'); real AWS does not resolve this as a ResourceNotFoundException lookup, got 'ResourceNotFoundException'" (check line 62). The application id shown is a per-run generated value.
  Related wording divergence in the same branch family: for an application that exists but is unassociated the emulator emits ValidationException "application <id> is not associated with stream group <group-id>; use AssociateApplications to associate it first" instead of AWS's "Application '<app-arn>' cannot be set as default because it is not currently associated with the stream group".

## Steps to Reproduce
Cause 1 of 2 (DisassociateApplications 501):
1. Create an S3 bucket and put an object at build/start.sh; CreateApplication(RuntimeEnvironment={UBUNTU,22_04_LTS}, ExecutablePath=start.sh, ApplicationSourceUri=s3://<bucket>/build) and wait for READY.
2. CreateStreamGroup(StreamClass=gen4n_high, LocationConfigurations=[{LocationName=<region>, MaximumCapacity=2}]) and wait for ACTIVE.
3. Call DisassociateApplications(Identifier=<group-arn>, ApplicationIdentifiers=["a-doesnotexist"]).
   Observe HTTP 501 / NotImplemented; expected HTTP 404 ResourceNotFoundException naming the resolved application ARN.
4. Call DisassociateApplications with the real but unassociated application id — expected HTTP 200 with ApplicationArns == [].
5. AssociateApplications, confirm the ARN appears in GetStreamGroup.AssociatedApplications, then DisassociateApplications and confirm it is gone from GetStreamGroup.AssociatedApplications and from GetApplication.AssociatedStreamGroups.

Cause 2 of 2 (UpdateStreamGroup misclassification):
1. CreateStreamGroup(Description=<d>, StreamClass=gen4n_high, LocationConfigurations=[{LocationName=<region>, MaximumCapacity=2}]); poll GetStreamGroup until Status == ACTIVE. Confirm DefaultApplication is unset.
2. Call UpdateStreamGroup(Identifier=<group-arn>, DefaultApplicationIdentifier="a-<random>") for an application id that does not exist.
   Observe HTTP 404 ResourceNotFoundException "Resource '<arn>' does not exist"; expected HTTP 400 ValidationException "Application '<resolved-app-arn>' cannot be set as default because it is not currently associated with the stream group (Internal ID: <uuid>)".
3. GetStreamGroup and confirm DefaultApplication is still unset.
4. Call UpdateStreamGroup(Identifier=<group-arn>, Description=<new>) and confirm it succeeds and GetStreamGroup reflects <new>.
