Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53profiles.get-profile-nonexistent-not-found route53profiles.delete-profile-lifecycle route53profiles.disassociate-profile-removes-association route53profiles.delete-profile-with-active-association-rejected route53profiles.associate-profile-duplicate-vpc-rejected
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: route53profiles.get-profile-nonexistent-not-found|run#6|AssertionError|assert error["Message"] == expected_message, f"expected {expected_message!r}, got {error['Message']!r}"
Issue-Key: route53profiles.delete-profile-lifecycle|run#10|AssertionError|assert error["Message"] == expected_message, f"expected {expected_message!r}, got {error['Message']!r}"
Issue-Key: route53profiles.disassociate-profile-removes-association|run#14|AssertionError|assert gone["Message"] == gone_message, f"expected {gone_message!r}, got {gone['Message']!r}"
Issue-Key: route53profiles.delete-profile-with-active-association-rejected|run#10|AssertionError|assert error["Message"] == expected_message, f"expected {expected_message!r}, got {error['Message']!r}"
Issue-Key: route53profiles.associate-profile-duplicate-vpc-rejected|run#10|AssertionError|assert error["Code"] == "LimitExceededException", f"expected LimitExceededException, got {error['Code']!r}"
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# route53profiles error reporting diverges from AWS: ResourceNotFoundException/ConflictException messages lack the [RSLVR-*] templates, and a VPC already held by another Profile is refused with the wrong error code

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProfile

## Description
Three distinct root causes in the route53profiles error channel. Every one of them is a REFUSAL the emulator already produces, so each is purely about the code/message the client sees, not about an unimplemented operation. (Cause 3 is a wrong error CODE, not only wording.)

Cause 1 of 3 — ResourceNotFoundException message does not use AWS's RSLVR template.
AWS: Route 53 Profiles reports a missing resource (a Profile id, a Profile-association id) through ResourceNotFoundException at HTTP 400 (Smithy: GetProfile / DeleteProfile / GetProfileAssociation all model ResourceNotFoundException, errorKind client, httpError 400; the model documents it only as "The resource you are associating is not found" — the message text itself is recorded behavior only). The recorded message is a single fixed template shared by every resource kind of this service: the fixed text `[RSLVR-05007] Can't find the resource with ID "` + the id + `".` — i.e. the Resolver-family error code prefix `[RSLVR-05007]`, the fixed sentence `Can't find the resource with ID`, and ONE variable part: the id taken verbatim from the request (`rp-…` for a Profile, `rpassoc-…` for an association). Nothing else in the message varies, and the template is identical for Profiles and for Profile associations.
Emulator: emits its own prose instead — `Profile rp-00000000000000000000 was not found.` for a Profile and `Profile association rpassoc-… was not found.` for an association. The error code (ResourceNotFoundException) and status are right; the message is wrong text, and the emulator varies the wording by resource kind where AWS does not.
Why: clients and test suites that match on the documented `[RSLVR-05007]` code embedded in the message (the Resolver/Profiles family surfaces its sub-error code only there, not in the Error.Code) never match, so retry/branching logic keyed on the RSLVR code silently falls through to a generic failure path. The divergence is visible both on a never-created id and immediately after a successful delete/disassociate, so it also breaks "assert the resource is gone" assertions.
Fix: the shared message builders in this service's state module — `profile_not_found` and `profile_association_not_found` — which every handler routes its not-found refusal through; both must build the single `[RSLVR-05007] Can't find the resource with ID "<id>".` template, filling `<id>` from the request.
Traces: route53profiles.get-profile-nonexistent-not-found (GetProfile on `rp-00000000000000000000`), route53profiles.delete-profile-lifecycle (GetProfile after a successful DeleteProfile), route53profiles.disassociate-profile-removes-association (GetProfileAssociation after a successful DisassociateProfile). Builders cited: services/service-com-amazonaws-route53profiles-route53profiles/src/state.rs lines 373-391; call sites e.g. src/ops/get_profile.rs lines 154-158 and src/ops/delete_profile.rs lines 162-167.

Cause 2 of 3 — DeleteProfile's refusal for a still-associated Profile has the wrong message and names the wrong resource.
AWS: DeleteProfile refuses while any association still names the Profile, with ConflictException at HTTP 400 (Smithy: DeleteProfile errors include ConflictException, errorKind client, httpError 400, documented "The request you submitted conflicts with an existing request"; message text is recorded behavior only). The recorded message's fixed text is `[RSLVR-05102] The Profile is associated with a resource: "` + an id + `". Delete the association and try again.` Its one variable part is the id of the blocking PROFILE ASSOCIATION (`rpassoc-…`), read from the emulator's/service's own state — NOT the ProfileId from the request.
Emulator: returns ConflictException (code correct) with `Profile rp-… still has resource associations and cannot be deleted.` — wrong fixed text, no `[RSLVR-05102]` code, and it reports the requested ProfileId where AWS reports the blocking association's id, so the caller is not told which association to delete.
Why: a client that parses the message to find the association it must delete before retrying the delete gets a Profile id instead, so an automated "disassociate then delete" remediation loop has nothing to act on; matching on `[RSLVR-05102]` also fails.
Fix: the DeleteProfile handler's conflict branch — it must look the blocking association up in state (the state helper that answers whether a Profile still has associations currently returns only a boolean, so it needs to yield the association's id) and build the `[RSLVR-05102]` template from it.
Traces: route53profiles.delete-profile-with-active-association-rejected. Code cited: services/service-com-amazonaws-route53profiles-route53profiles/src/ops/delete_profile.rs lines 171-181, and the boolean-only helper `profile_has_associations` at src/state.rs lines 251-255.

Cause 3 of 3 — AssociateProfile returns ConflictException where AWS returns LimitExceededException when the VPC is already associated with a DIFFERENT Profile.
AWS: a VPC can have only one Profile associated with it (AssociateProfile's own modeled documentation: "A VPC can have only one Profile associated with it, but a Profile can be associated with 1000 of VPCs"). AWS expresses the second association of an already-associated VPC as a QUOTA refusal, not a conflict: LimitExceededException (Smithy: AssociateProfile models AccessDeniedException, ConflictException, InvalidParameterException, LimitExceededException, ResourceExistsException, ResourceNotFoundException, ThrottlingException, ValidationException — all errorKind client, httpError 400). Recorded message, fixed text plus one variable part: `VPC [` + the ResourceId from the request + `] exceeded its maximum profile association limit.`
Emulator: classifies the same request as ConflictException with `Resource <ResourceId> is already associated with Profile <otherProfileId> by association <assocId>; a resource can have only one Profile associated with it.` Wrong Error.Code and wrong message. (The emulator's reasoning is visible in the handler comment: it deliberately splits same-Profile → ResourceExistsException and different-Profile → ConflictException.)
Why: this is a code-level divergence, not just wording. Callers branch on the code: LimitExceededException is the retryable/quota path (the suite's own profile-creation helper retries precisely on LimitExceededException), while ConflictException reads as a request conflict that will never clear. A client written against AWS either fails outright or stops retrying where AWS would have had it back off, and quota dashboards/alarms keyed on LimitExceededException never fire.
Why it is not Cause 2: different operation, different code, different trigger — AssociateProfile's admission check, not DeleteProfile's conflict check.
Fix: the AssociateProfile handler's already-associated branch — the different-Profile case must raise LimitExceededException with the `VPC [<ResourceId>] exceeded its maximum profile association limit.` message (ResourceId from the request). The ceiling-based LimitExceededException already in that handler is a separate, unrelated branch and must not be confused with it.
Traces: route53profiles.associate-profile-duplicate-vpc-rejected. Code cited: services/service-com-amazonaws-route53profiles-route53profiles/src/ops/associate_profile.rs lines 190-215 (the ConflictException branch) and lines 220-233 (the unrelated store-ceiling LimitExceededException).

## Evidence
Cause 1:
- GetProfile(ProfileId="rp-00000000000000000000"): expected Error.Code=ResourceNotFoundException with Message '[RSLVR-05007] Can\'t find the resource with ID "rp-00000000000000000000".'; got Message 'Profile rp-00000000000000000000 was not found.' (check route53profiles.get-profile-nonexistent-not-found).
- GetProfile after DeleteProfile: expected '[RSLVR-05007] Can\'t find the resource with ID "rp-100c3e9a00000001".'; got 'Profile rp-100c3e9a00000001 was not found.' (route53profiles.delete-profile-lifecycle).
- GetProfileAssociation after DisassociateProfile: expected '[RSLVR-05007] Can\'t find the resource with ID "rpassoc-4480756700000002".'; got 'Profile association rpassoc-4480756700000002 was not found.' (route53profiles.disassociate-profile-removes-association).
(The `rp-…`/`rpassoc-…` values above are the per-run ids of those replays; the requirement is the template, with the id taken from the request.)

Cause 2:
- DeleteProfile on a Profile with one live VPC association: Error.Code=ConflictException (matches) but expected Message '[RSLVR-05102] The Profile is associated with a resource: "rpassoc-5214343c00000002". Delete the association and try again.'; got 'Profile rp-5214343c00000001 still has resource associations and cannot be deleted.' Note the expected id is the ASSOCIATION id, the actual is the PROFILE id.

Cause 3:
- AssociateProfile(ProfileId=profile_b, ResourceId=vpc already associated with profile_a): expected Error.Code=LimitExceededException with Message 'VPC [<vpcId>] exceeded its maximum profile association limit.'; got Error.Code='ConflictException'. Assertion: "expected LimitExceededException, got 'ConflictException'".
All three causes return HTTP 400, which is correct; the divergence is code and/or message.

## Steps to Reproduce
Cause 1:
1. GetProfile(ProfileId="rp-00000000000000000000") against the emulator -> inspect Error.Message.
2. CreateProfile, DeleteProfile, then GetProfile on the deleted id -> inspect Error.Message.
3. CreateProfile + CreateVpc + AssociateProfile, DisassociateProfile, then GetProfileAssociation(ProfileAssociationId=<assocId>) -> inspect Error.Message.
Each must read '[RSLVR-05007] Can\'t find the resource with ID "<id>".'

Cause 2:
1. CreateVpc; CreateProfile; AssociateProfile(ProfileId, ResourceId=vpc) -> note the returned ProfileAssociation.Id.
2. DeleteProfile(ProfileId) while the association is COMPLETE.
3. Expect ConflictException with '[RSLVR-05102] The Profile is associated with a resource: "<associationId>". Delete the association and try again.'

Cause 3:
1. CreateVpc; CreateProfile twice (profile_a, profile_b).
2. AssociateProfile(ProfileId=profile_a, ResourceId=vpc); wait for COMPLETE.
3. AssociateProfile(ProfileId=profile_b, ResourceId=vpc).
4. Expect Error.Code=LimitExceededException and Message 'VPC [<vpcId>] exceeded its maximum profile association limit.'; the emulator answers ConflictException.
5. Positive control that must keep working: DisassociateProfile(profile_a, vpc), then the same AssociateProfile(profile_b, vpc) must succeed.
