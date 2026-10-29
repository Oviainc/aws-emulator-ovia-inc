Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-authorization-token-and-repository-endpoint
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-authorization-token-and-repository-endpoint codeartifact-domain-error-paths codeartifact-package-group-error-paths codeartifact-package-group-hierarchy-and-association codeartifact-package-group-lifecycle-and-hierarchy codeartifact-repository-error-paths codeartifact-repository-lifecycle-and-conflicts codeartifact-resource-not-found-envelopes codeartifact-tag-resource-error-paths
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|GetAuthorizationToken|4xx->4xx|$.message
Issue-Key: codeartifact|PutDomainPermissionsPolicy|4xx->4xx|$.message
Issue-Key: codeartifact|DescribePackageGroup|4xx->4xx|$.message
Issue-Key: codeartifact|CreatePackageGroup|4xx->4xx|$.message
Issue-Key: codeartifact|DeletePackageGroup|4xx->4xx|$.message
Issue-Key: codeartifact|CreateRepository|4xx->4xx|$.message
Issue-Key: codeartifact|GetPackageVersionAsset|4xx->4xx|$.message
Issue-Key: codeartifact|TagResource|4xx->4xx|$.message
Issue-Key: codeartifact|ListTagsForResource|4xx->4xx|$.message
Issue-Key: codeartifact|UntagResource|4xx->4xx|$.message
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodeArtifact control-plane refusal messages are the emulator's own sentences, not AWS's

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAuthorizationToken

## Description
This issue bundles EIGHT distinct net-new root causes found by replaying the simparity-wire-aws-v1 wire corpus against the emulator. In every one of them the emulator picks the right error class and the right HTTP status, and then emits a sentence the service never writes: the refusal message is re-worded, re-punctuated, re-quoted or re-framed. An error message is part of the wire contract, so each one is its own defect. A fixer must close all eight before submitting; each cause names its own failing fixture step. (All names, ids, accounts, digests and numbers quoted below are one run's values filling a template — only the fixed text and the rule for the variable parts are the requirement.)

=== Cause 1 of 8: GetAuthorizationToken's in-gap durationSeconds refusal is re-worded ===
AWS: Smithy GetAuthorizationToken (POST /v1/authorization-token) takes durationSeconds -> AuthorizationTokenDurationSeconds @range(min 0, max 43200), documented as "Valid values are 0 and any number between 900 (15 minutes) and 43200 (12 hours)" — a DISCONTINUOUS accepted set, so a non-zero value under 900 is a service-rule refusal, not a modeled-range one: ValidationException, HTTP 400, message "Invalid durationSeconds value of <N>: value must be 0 or between 900 and 43200 (inclusive)." Fixed text: "Invalid durationSeconds value of " and ": value must be 0 or between 900 and 43200 (inclusive)."; <N> is the durationSeconds the request sent.
Emulator: ValidationException/400 (correct class) with "durationSeconds value <N> is not valid; valid values are 0 and any number between 900 and 43200."
Why: a client or CLI test that matches/surfaces the service's validation sentence sees text AWS never emits, and cannot tell the service-rule refusal from the modeled @range refusal, which has its own distinct "1 validation error detected: ..." sentence.
Fix: the message built by the duration guard in the GetAuthorizationToken handler's validation branch (src/ops/get_authorization_token.rs); the above-ceiling end must keep going through the shared modeled-constraint sentence.
Traces: codeartifact-authorization-token-and-repository-endpoint#task#5. The open behavioral issue 20261029T180849Z-codeartifact-control-plane-10-fidelity-divergences-in-getauthorizationtoken.md describes this same wording in its Cause 1, but it carries behavioral check ids only and does NOT carry this fixture step, so the wire step is still unowned and is filed here.

=== Cause 2 of 8: PutDomainPermissionsPolicy leaks a JSON-parser diagnostic instead of the service's sentence ===
AWS: PutDomainPermissionsPolicy (PUT /v1/domain/permissions/policy) models ValidationException httpError 400; policyDocument is PolicyDocument @length(1..7168) @pattern(^[\P{C}\s]+$), so a syntactically broken document passes the modeled constraints and is refused by the service's own policy parser with the fixed sentence "Policy document isn't a valid policy document" — entirely fixed text, naming neither the member nor the parse position.
Emulator: ValidationException/400 with "policyDocument is not valid JSON: expected ident at line 1 column 2." — the emulator's serde diagnostic (member name, line/column), which is not service text and varies with the input.
Why: the parser's line/column wording is an implementation detail; a client matching or displaying the service's sentence breaks, and the emulator exposes which JSON library it uses.
Fix: the policy-document parse branch of the PutDomainPermissionsPolicy handler (src/ops/put_domain_permissions_policy.rs) and the shared policy-document validation helper in src/state.rs — map any parse failure onto the single fixed sentence.
Traces: codeartifact-domain-error-paths#task#5.

=== Cause 3 of 8: DescribePackageGroup's not-found sentence is tidied up ===
AWS: DescribePackageGroup (GET /v1/package-group) models ResourceNotFoundException httpError 404. For an absent group the message is "Package Group not found not found. Package Group '<pattern>' in domain <domain> owned by account <account> does not exist" — note the service's capitalisation "Package Group", its doubled "not found not found.", the pattern single-quoted, the domain and account NOT quoted, and NO trailing period. <pattern>, <domain> and <account> come from the request and the owning account; everything else is fixed text.
Emulator: ResourceNotFoundException/404 with "Package group not found. Package group '<pattern>' in domain '<domain>' owned by account '<account>' does not exist." — lower-cased, doubled clause dropped, domain and account quoted, trailing period added.
Why: the quirky doubled text is exactly what a tidied emulator message hides; clients that match or render the service sentence diverge.
Fix: the package-group not-found message helper in src/state.rs as used by src/ops/describe_package_group.rs (and by the other package-group readers that share it). Same wording as Cause 10 of the open behavioral issue 20261029T180849Z-...-10-fidelity-divergences-in-getauthorizationtoken.md, which does not carry these wire steps.
Traces: codeartifact-package-group-error-paths#task#2 (cluster also covers codeartifact-package-group-hierarchy-and-association#task#13 and codeartifact-package-group-lifecycle-and-hierarchy#task#21).

=== Cause 4 of 8: CreatePackageGroup's duplicate-group ConflictException is re-worded ===
AWS: CreatePackageGroup (POST /v1/package-group) models ConflictException httpError 409. For a pattern already held in the domain the message is "Package Group '<pattern>' already exists in domain <domain> owned by account <account>" — fixed text "Package Group '", "' already exists in domain ", " owned by account "; the domain and account are unquoted and there is no trailing period.
Emulator: ConflictException/409 with "Package group '<pattern>' in domain '<domain>' owned by '<account>' already exists." — different word order and capitalisation, values quoted, "account" dropped from the owner clause, trailing period added.
Why: a duplicate create is the normal idempotency signal for provisioning code, which frequently matches the sentence it surfaces.
Fix: the duplicate-pattern refusal message in src/ops/create_package_group.rs / the shared package-group conflict helper in src/state.rs.
Traces: codeartifact-package-group-error-paths#task#4 (cluster also covers codeartifact-package-group-hierarchy-and-association#task#10).

=== Cause 5 of 8: DeletePackageGroup's root-group refusal is re-worded ===
AWS: DeletePackageGroup (DELETE /v1/package-group) models ValidationException httpError 400; the domain's root group '/*' cannot be removed and the refusal message is "The root package group, '/*', cannot be deleted." — entirely fixed text (the pattern '/*' is the root pattern itself, not a request-dependent value), with the comma-set clause as AWS words it.
Emulator: ValidationException/400 with "Package group '/*' is the root package group of its domain and cannot be deleted."
Why: clients branching on or displaying the service sentence see text AWS never emits.
Fix: the root-group guard's message in src/ops/delete_package_group.rs.
Traces: codeartifact-package-group-hierarchy-and-association#task#14.

=== Cause 6 of 8: CreateRepository's duplicate-name ConflictException is re-worded ===
AWS: CreateRepository (POST /v1/repository) models ConflictException httpError 409. For a repository name already taken inside the addressed domain the message is "Repository <repository> already exists in domain <domain>." — fixed text "Repository ", " already exists in domain ", "."; <repository> and <domain> are the request's own values, neither is quoted, and the owning account is NOT part of the sentence.
Emulator: ConflictException/409 with "Repository '<repository>' in domain '<domain>' owned by '<account>' already exists." — reordered, quoted, plus an owner clause AWS does not emit.
Why: same as Cause 4 — IaC/provisioning layers surface this sentence verbatim.
Fix: the duplicate-name refusal in src/ops/create_repository.rs / the shared repository-conflict message helper in src/state.rs. Also described as Cause 3 of the open behavioral issue 20261029T180849Z-...-10-fidelity-divergences-in-getauthorizationtoken.md, which carries no wire step for it.
Traces: codeartifact-repository-error-paths#task#3 (cluster also covers codeartifact-repository-lifecycle-and-conflicts#task#6).

=== Cause 7 of 8: GetPackageVersionAsset's asset-not-found sentence does not name the coordinate the way AWS does ===
AWS: GetPackageVersionAsset (GET /v1/package/version/asset) models ResourceNotFoundException httpError 404. For an asset name the version does not hold, the message is "Asset not found in repo. RepositoryName = '<repository>', packageCoordinate = 'PackageVersionCoordinateEntity{name=<package>, type=<FORMAT in upper case>, namespace=<namespace>, version=<version>, displayName=<namespace>/<package>/<version>}', assetName = '<asset>'." — the fixed text is "Asset not found in repo. RepositoryName = '", "', packageCoordinate = 'PackageVersionCoordinateEntity{name=", ", type=", ", namespace=", ", version=", ", displayName=", "}', assetName = '", "'."; every variable part comes from the request's coordinate (the format upper-cased, the displayName composed from namespace/package/version).
Emulator: ResourceNotFoundException/404 with "Asset <asset> not found in package version <package> <version> in repository <repository>." — its own sentence, omitting the coordinate entity entirely.
Why: the coordinate entity is the part that tells a caller WHICH coordinate was resolved (format and namespace included); clients that parse it, and tests asserting the service sentence, both fail.
Fix: the asset-lookup failure message in src/ops/get_package_version_asset.rs / the shared asset-not-found helper in src/state.rs.
Traces: codeartifact-resource-not-found-envelopes#task#7.

=== Cause 8 of 8: the tagging family refuses a malformed resourceArn with its own sentence instead of the ARN-parser sentence ===
AWS: TagResource (POST /v1/tag), ListTagsForResource and UntagResource all take resourceArn -> Arn @length(1..1011) @pattern(^\S+$) and all model ValidationException httpError 400. A string that satisfies the pattern but is not a parseable ARN is refused by the shared ARN parser with "Invalid ARN: <resourceArn> can't be parsed because it doesn't have enough separators." — fixed text "Invalid ARN: " and " can't be parsed because it doesn't have enough separators.", with <resourceArn> echoed verbatim from the request. The sentence is about ARN SYNTAX, and is the same for all three operations.
Emulator: ValidationException/400 with "resourceArn <resourceArn> is not a CodeArtifact resource ARN." on all three — a service-ownership sentence rather than the parser's syntax sentence, so a syntactically broken ARN and a well-formed ARN of another service become indistinguishable.
Why: clients and tests distinguish "this is not an ARN" from "this is not MY ARN"; the emulator collapses the two and emits text AWS never produces for either.
Fix: the resourceArn parsing/validation step shared by src/ops/tag_resource.rs, src/ops/list_tags_for_resource.rs and src/ops/untag_resource.rs (the ARN-parse helper in src/state.rs) — raise the parser sentence when the string cannot be parsed as an ARN at all.
Traces: codeartifact-tag-resource-error-paths#task#2 (TagResource), codeartifact-tag-resource-error-paths#task#3 (ListTagsForResource), codeartifact-tag-resource-error-paths#task#4 (UntagResource).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator booted from this checkout: 344 steps, 272 passed, 72 mismatched, score 79.1%, 30 divergence clusters — all on service "codeartifact". None of the clusters below carries a stub_path or an owned_by, i.e. no open issue carries their replay keys. Verdicts verbatim from the replay output:

Cause 1 — [mismatch] codeartifact-authorization-token-and-repository-endpoint#task#5 GetAuthorizationToken, status 400 -> 400: $.message: value_mismatch (expected 'Invalid durationSeconds value of 100: value must be 0 or between 900 and 43200 (inclusive).', got 'durationSeconds value 100 is not valid; valid values are 0 and any number between 900 and 43200.')

Cause 2 — [mismatch] codeartifact-domain-error-paths#task#5 PutDomainPermissionsPolicy, 400 -> 400: $.message: value_mismatch (expected "Policy document isn't a valid policy document", got 'policyDocument is not valid JSON: expected ident at line 1 column 2.')

Cause 3 — [mismatch] codeartifact-package-group-error-paths#task#2 DescribePackageGroup, 404 -> 404 (3 steps in cluster): $.message: value_mismatch (expected "Package Group not found not found. Package Group '/npm/<group>-missing/*' in domain <domain> owned by account 000000000000 does not exist", got "Package group not found. Package group '/npm/<group>-missing/*' in domain '<domain>' owned by account '000000000000' does not exist.")

Cause 4 — [mismatch] codeartifact-package-group-error-paths#task#4 CreatePackageGroup, 409 -> 409 (2 steps in cluster): $.message: value_mismatch (expected "Package Group '/npm/<group>/*' already exists in domain <domain> owned by account 000000000000", got "Package group '/npm/<group>/*' in domain '<domain>' owned by '000000000000' already exists.")

Cause 5 — [mismatch] codeartifact-package-group-hierarchy-and-association#task#14 DeletePackageGroup, 400 -> 400: $.message: value_mismatch (expected "The root package group, '/*', cannot be deleted.", got "Package group '/*' is the root package group of its domain and cannot be deleted.")

Cause 6 — [mismatch] codeartifact-repository-error-paths#task#3 CreateRepository, 409 -> 409 (2 steps in cluster): $.message: value_mismatch (expected 'Repository <repo> already exists in domain <domain>.', got "Repository '<repo>' in domain '<domain>' owned by '000000000000' already exists.")

Cause 7 — [mismatch] codeartifact-resource-not-found-envelopes#task#7 GetPackageVersionAsset, 404 -> 404: $.message: value_mismatch (expected "Asset not found in repo. RepositoryName = '<repo>', packageCoordinate = 'PackageVersionCoordinateEntity{name=<pkg>, type=GENERIC, namespace=sim2real, version=1.0.0, displayName=sim2real/<pkg>/1.0.0}', assetName = 'absent-asset.bin'.", got 'Asset absent-asset.bin not found in package version <pkg> 1.0.0 in repository <repo>.')

Cause 8 — [mismatch] codeartifact-tag-resource-error-paths#task#2 TagResource, #task#3 ListTagsForResource, #task#4 UntagResource, all 400 -> 400, all three identical: $.message: value_mismatch (expected "Invalid ARN: arn:aws:codeartifact:not-a-real-arn can't be parsed because it doesn't have enough separators.", got 'resourceArn arn:aws:codeartifact:not-a-real-arn is not a CodeArtifact resource ARN.')

In all eight the HTTP status and the error code (x-amzn-errortype) match AWS; only the message text diverges.

## Steps to Reproduce
Boot the emulator from this checkout and replay simparity-wire-aws-v1 (the fixture#step for each cause is listed under Traces / Issue-Trace-Ids). Per cause, by hand:
Cause 1: create_domain(domain=D); get_authorization_token(domain=D, durationSeconds=100) -> expect ValidationException/400 "Invalid durationSeconds value of 100: value must be 0 or between 900 and 43200 (inclusive)."
Cause 2: create_domain(domain=D); put_domain_permissions_policy(domain=D, policyDocument='{not json') -> expect ValidationException/400 "Policy document isn't a valid policy document".
Cause 3: create_package_group(domain=D, packageGroup='/npm/G/*'); delete it; describe_package_group(domain=D, packageGroup='/npm/G/*') -> expect ResourceNotFoundException/404 "Package Group not found not found. Package Group '/npm/G/*' in domain D owned by account <account> does not exist" (no trailing period).
Cause 4: create_package_group(domain=D, packageGroup='/npm/G/*') twice -> expect ConflictException/409 "Package Group '/npm/G/*' already exists in domain D owned by account <account>".
Cause 5: delete_package_group(domain=D, packageGroup='/*') -> expect ValidationException/400 "The root package group, '/*', cannot be deleted."
Cause 6: create_repository(domain=D, repository=R) twice -> expect ConflictException/409 "Repository R already exists in domain D."
Cause 7: publish a generic version (namespace=N, package=P, version=V) into R, then get_package_version_asset(..., asset='absent-asset.bin') -> expect ResourceNotFoundException/404 with the "Asset not found in repo. RepositoryName = 'R', packageCoordinate = 'PackageVersionCoordinateEntity{name=P, type=GENERIC, namespace=N, version=V, displayName=N/P/V}', assetName = 'absent-asset.bin'." sentence.
Cause 8: tag_resource(resourceArn='arn:aws:codeartifact:not-a-real-arn', tags=[...]), list_tags_for_resource(same arn), untag_resource(same arn, tagKeys=[...]) -> each expects ValidationException/400 "Invalid ARN: arn:aws:codeartifact:not-a-real-arn can't be parsed because it doesn't have enough separators."
