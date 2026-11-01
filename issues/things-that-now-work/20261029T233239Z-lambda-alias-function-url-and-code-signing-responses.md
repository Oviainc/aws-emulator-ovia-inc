Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-alias-lifecycle
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-alias-lifecycle lambda-alias-traffic-shifting lambda-delete-alias-nonexistent-is-a-no-op lambda-version-alias-routing lambda-function-url-config-lifecycle lambda-code-signing-config-enforcement lambda-function-codesigning-attach-error-paths lambda-function-scaling-config-nonexistent-function lam-page-create-alias-numeric-name lambda-resource-policy-on-alias
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|GetAlias|4xx->4xx|$.Type
Issue-Key: lambda|CreateAlias|4xx->4xx|$.Type
Issue-Key: lambda|UpdateAlias|4xx->4xx|$.Type
Issue-Key: lambda|GetFunctionUrlConfig|4xx->4xx|$.Type
Issue-Key: lambda|PutFunctionCodeSigningConfig|4xx->4xx|$.Type
Issue-Key: lambda|GetFunctionScalingConfig|4xx->4xx|$.Type
Issue-Key: lambda|GetAlias|2xx->2xx|$.Description
Issue-Key: lambda|CreateAlias|2xx->2xx|$.Description
Issue-Key: lambda|ListAliases|2xx->2xx|$.Aliases[*].Description
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda alias, function-URL and code-signing responses drop members AWS returns (Description, Type)

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAlias

## Description
Shared fault: alias / function-URL / code-signing responses are serialized with optional members dropped — `Type` on error bodies and `Description` on alias bodies. Ten net-new clusters.

AWS: Smithy AliasConfiguration declares `Description` (target Description, @length 0..256); AWS returns it as the empty string when the alias was created without one, on CreateAlias, GetAlias and each ListAliases entry. Smithy ResourceNotFoundException/ResourceConflictException/PreconditionFailedException each declare a `Type` member, populated with the fixed string `User` on these client errors.

Emulator: `Description` is omitted entirely from alias payloads when unset, and `Type` is omitted from every error body below.

Why: a client that reads `alias.Description` gets a missing key rather than an empty string (SDKs that distinguish absent from empty mis-render, and round-trip comparisons fail); a client that classifies failures by `Type` cannot.

Fix: the alias serializer must render `Description` for an unset description, and the shared modeled-exception serializer must emit `Type`.

Cause 1 of 10: GetAlias, 404 — fixture lambda-alias-lifecycle step task#10; `Type: "User"` missing.
Cause 2 of 10: CreateAlias, 409 — fixture lambda-alias-traffic-shifting step task#11; `Type` missing.
Cause 3 of 10: UpdateAlias, 412 — fixture lambda-alias-traffic-shifting step task#12; `Type` missing.
Cause 4 of 10: CreateAlias, 404 — fixture lambda-version-alias-routing step task#7; `Type` missing.
Cause 5 of 10: GetFunctionUrlConfig, 404 — fixture lambda-function-url-config-lifecycle step task#13; `Type` missing.
Cause 6 of 10: PutFunctionCodeSigningConfig, 404 — fixture lambda-code-signing-config-enforcement step task#11; `Type` missing.
Cause 7 of 10: GetFunctionScalingConfig, 404 — fixture lambda-function-scaling-config-nonexistent-function step task#3; `Type` missing.
Cause 8 of 10: GetAlias, 200 — fixture lambda-delete-alias-nonexistent-is-a-no-op step task#8; `Description` (empty string for an alias created without one) omitted.
Cause 9 of 10: CreateAlias, 201 — fixture lam-page-create-alias-numeric-name step task#5; `Description` omitted.
Cause 10 of 10: ListAliases, 200 — fixture lam-page-create-alias-numeric-name step task#6; `Aliases[0].Description` omitted.

## Evidence
Clusters 1-7 detail: `$.Type: missing (expected 'User', got None)` with expected status == actual status. Cluster 8/9 detail: `$.Description: missing (expected '', got None)`. Cluster 10 detail: `$.Aliases[0].Description: missing (expected '', got None)`. Smithy: AliasConfiguration.Description is a modeled member; the three exception shapes declare `Type`.

## Steps to Reproduce
Replay the corpus. Fixture lam-page-create-alias-numeric-name step task#5: CreateAlias without Description and read the 201 body — no `Description` key. Fixture lambda-alias-lifecycle step task#10: GetAlias for a missing alias and read the 404 body — no `Type` key.
