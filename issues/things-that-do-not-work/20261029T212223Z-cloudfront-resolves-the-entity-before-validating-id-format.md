Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-anycast-ip-list-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-anycast-ip-list-error-paths cloudfront-anycast-ip-list-get-not-found cloudfront-anycast-ip-list-lifecycle cloudfront-continuous-deployment-policy-errors cloudfront-continuous-deployment-policy-not-found-errors cloudfront-distribution-not-found-error-paths cloudfront-distribution-tenant-not-found-errors cloudfront-key-group-error-paths cloudfront-key-group-lifecycle cloudfront-public-key-duplicate-and-notfound-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetAnycastIpList|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|GetAnycastIpList|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetAnycastIpList|4xx->4xx|status
Issue-Key: cloudfront|UpdateAnycastIpList|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|UpdateAnycastIpList|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|UpdateAnycastIpList|4xx->4xx|status
Issue-Key: cloudfront|DeleteContinuousDeploymentPolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|DeleteContinuousDeploymentPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteContinuousDeploymentPolicy|4xx->4xx|status
Issue-Key: cloudfront|DisassociateDistributionWebACL|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|DisassociateDistributionWebACL|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DisassociateDistributionWebACL|4xx->4xx|status
Issue-Key: cloudfront|DeleteDistributionTenant|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|DeleteDistributionTenant|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteDistributionTenant|4xx->4xx|status
Issue-Key: cloudfront|AssociateDistributionTenantWebACL|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|AssociateDistributionTenantWebACL|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|AssociateDistributionTenantWebACL|4xx->4xx|status
Issue-Key: cloudfront|DisassociateDistributionTenantWebACL|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|DisassociateDistributionTenantWebACL|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DisassociateDistributionTenantWebACL|4xx->4xx|status
Issue-Key: cloudfront|CreateInvalidationForDistributionTenant|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|CreateInvalidationForDistributionTenant|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateInvalidationForDistributionTenant|4xx->4xx|status
Issue-Key: cloudfront|GetInvalidationForDistributionTenant|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetKeyGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront resolves the entity before validating id format or If-Match, so argument errors surface as NotFound

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAnycastIpList

## Description
Ten causes, one shared fault: validation order. AWS validates the SHAPE of the path identifier (and the presence/validity of If-Match) before it tries to resolve the entity, so a syntactically invalid id is a 400 InvalidArgument and a missing If-Match is a 400 InvalidIfMatchVersion. The emulator skips straight to the state lookup, fails to find anything, and answers 404 with a not-found code - a different status, a different error code and a different message on every one of these paths.

Fix for every cause: move the id-format check (and, for causes 3-4, the If-Match presence check) ahead of the state lookup in that operation's validation, and raise the modelled InvalidArgument / InvalidIfMatchVersion.

Causes 1-2 (AnycastIpList id format).
AWS: 400 `InvalidArgument` with the fixed message `Id format is invalid` for an id that is not a well-formed anycast-IP-list id.
Emulator: 404 `EntityNotFound` with `The specified Anycast IP list does not exist`.
Why: a client that passed a typo'd id is told the list is absent, so it retries/creates instead of fixing the argument.
Cause 1 of 10: GetAnycastIpList - traces cloudfront-anycast-ip-list-error-paths#task#2 (also cloudfront-anycast-ip-list-get-not-found#task#1, cloudfront-anycast-ip-list-lifecycle#task#2).
Cause 2 of 10: UpdateAnycastIpList - traces cloudfront-anycast-ip-list-lifecycle#task#4.

Causes 3-4 (If-Match validated after the lookup).
AWS: 400 `InvalidIfMatchVersion` with the fixed message `The If-Match version is missing or not valid for the resource.` - the header is checked first, so this is returned even when the named resource does not exist.
Emulator: 404 with the resource's not-found code and message.
Why: the caller is told the resource is gone when the real defect is a missing/invalid If-Match, so it never adds the header.
Cause 3 of 10: DeleteContinuousDeploymentPolicy - emulator answers 404 `NoSuchContinuousDeploymentPolicy` 'The specified continuous deployment policy does not exist.' Traces cloudfront-continuous-deployment-policy-errors#task#3 (also cloudfront-continuous-deployment-policy-not-found-errors#task#2).
Cause 4 of 10: DisassociateDistributionWebACL - emulator answers 404 `EntityNotFound` 'The specified distribution does not exist.' Traces cloudfront-distribution-not-found-error-paths#task#10.

Causes 5-9 (DistributionTenant id format).
AWS: 400 `InvalidArgument` with `Id format is invalid` (and, on the invalidation read path, `DistributionTenantId format is invalid`) for a malformed tenant id.
Emulator: 404 `EntityNotFound` with "The specified distribution tenant doesn't exist."
Why: as causes 1-2; the tenant-id syntax rule is never enforced anywhere.
Cause 5 of 10: DeleteDistributionTenant - traces cloudfront-distribution-tenant-not-found-errors#task#2.
Cause 6 of 10: AssociateDistributionTenantWebACL - traces cloudfront-distribution-tenant-not-found-errors#task#3.
Cause 7 of 10: DisassociateDistributionTenantWebACL - traces cloudfront-distribution-tenant-not-found-errors#task#4.
Cause 8 of 10: CreateInvalidationForDistributionTenant - traces cloudfront-distribution-tenant-not-found-errors#task#5.
Cause 9 of 10: GetInvalidationForDistributionTenant - status matches (404) but the message must be `DistributionTenantId format is invalid`, naming the offending member; the emulator sends the tenant not-found sentence. Traces cloudfront-distribution-tenant-not-found-errors#task#6.

Cause 10 of 10: GetKeyGroup with a malformed key-group id.
AWS: 404 `NoSuchResource` (Smithy: GetKeyGroup's only modelled error) but with the message `Id format is invalid` - AWS reports the argument defect in the message even on the 404.
Emulator: `The specified key group does not exist.`
Why: the caller cannot tell a bad id from an absent key group.
Fix: GetKeyGroup's id-format check, ahead of the lookup.
Traces: cloudfront-key-group-error-paths#task#2 (also cloudfront-key-group-lifecycle#task#2, cloudfront-public-key-duplicate-and-notfound-errors#task#4).

## Evidence
1. GetAnycastIpList: expected 400 / InvalidArgument / 'Id format is invalid'; got 404 / EntityNotFound / 'The specified Anycast IP list does not exist'.
2. UpdateAnycastIpList: same expected/actual triple.
3. DeleteContinuousDeploymentPolicy: expected 400 / InvalidIfMatchVersion / 'The If-Match version is missing or not valid for the resource.'; got 404 / NoSuchContinuousDeploymentPolicy / 'The specified continuous deployment policy does not exist.'
4. DisassociateDistributionWebACL: expected 400 / InvalidIfMatchVersion / same message; got 404 / EntityNotFound / 'The specified distribution does not exist.'
5-8. DeleteDistributionTenant, AssociateDistributionTenantWebACL, DisassociateDistributionTenantWebACL, CreateInvalidationForDistributionTenant: each expected 400 / InvalidArgument / 'Id format is invalid'; each got 404 / EntityNotFound / "The specified distribution tenant doesn't exist."
9. GetInvalidationForDistributionTenant: status 404 matches; expected message 'DistributionTenantId format is invalid', got "The specified distribution tenant doesn't exist."
10. GetKeyGroup: status 404 matches; expected message 'Id format is invalid', got 'The specified key group does not exist.'

All ten expected messages are AWS's fixed text; 'Id'/'DistributionTenantId' are the request's own member names, not per-run values.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Causes 1-2, 5-9: call the operation with a path id that is not of the resource type's id form (e.g. a plainly wrong token in place of an ipl-*/dt_* id).
3. Causes 3-4: call DeleteContinuousDeploymentPolicy / DisassociateDistributionWebACL with no If-Match header, naming an id that does not exist.
4. Cause 10: GetKeyGroup with a malformed key-group id.
5. Each answers a not-found code/message instead of the argument or If-Match error, and causes 1-8 also answer 404 where AWS answers 400.
6. Representative step: fixture cloudfront-anycast-ip-list-error-paths, step task#2 (GetAnycastIpList).
