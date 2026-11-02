Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-version-draft-composition
Issue-Step: task#12
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-version-draft-composition resiliencehub-app-version-publish
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|ResolveAppVersionResources|2xx->2xx|$.resolutionId
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# resiliencehub ResolveAppVersionResources returns a counter-derived pseudo-UUID resolutionId instead of an AWS-shaped resolution id

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResolveAppVersionResources

## Description
One cause.

=== Cause 1 of 1: the server-generated resolutionId does not have the shape AWS gives a resource-resolution id ===
AWS: ResolveAppVersionResources (POST /resolve-app-version-resources, success 200) returns a required `resolutionId` targeting String255 (@length 1..255) documented as "The identifier for a specific resolution"; the same value is the `resolutionId` input of DescribeAppVersionResourcesResolutionStatus. The model constrains only the length, so beyond that this is recorded behavior only: the recording asserts the resolutionId's SHAPE and not its literal text (the fixture's expected value is the shape marker '<shaped-not-asserted>', because the id itself is minted per run), and the emulator's value fails that shape assertion. The recorded id is a canonical, opaque, randomly-generated resolution identifier - a conforming RFC-4122 version-4 UUID, whose version nibble is '4' and whose variant nibble is one of 8/9/a/b, with no structure a client can read anything out of. Hypothesis: AWS mints it from a random source per resolution, which is why no part of a recorded id repeats or counts.
Emulator: it answers 200 with a resolutionId of the form '<unix-nanos-high>-<nanos-mid>-<nanos-low>-0000-<12-hex-sequence>' - a time-plus-counter pseudo-UUID in 8-4-4-4-12 layout whose third group carries clock bits rather than a UUID version nibble, whose fourth group is the literal '0000' (no valid RFC-4122 variant nibble), and whose final group is a zero-padded monotonic process counter shared with every other id this service mints. The replay reports `$.resolutionId: value_mismatch (expected '<shaped-not-asserted>', got a counter-tailed pseudo-UUID)`; the HTTP status (200) and every other output member of the step match.
Why: a client that treats the resolutionId as an opaque UUID - parsing it with a UUID type, storing it in a uuid column, or validating it against a UUID shape before persisting or logging it - rejects or corrupts the value the emulator hands back, even though round-tripping the raw string into DescribeAppVersionResourcesResolutionStatus happens to work. The counter tail additionally makes resolution ids guessable and correlated across unrelated applications and policies, so a test that asserts two resolutions of different applications share no derivable structure, or that an id is not enumerable, passes against AWS and fails here.
Fix: the service's id minting used by this operation's state writer - the resolution-id generator behind ResourceResolution::started_at / the shared mint_resource_id helper in the resiliencehub crate's state module - should produce a canonically shaped random identifier (correct version and variant nibbles, no embedded clock or process counter) for resolutionId, keeping it unique within the emulator. Uniqueness can stay satisfied without encoding a sequence number in the wire value.
Traces: resiliencehub-app-version-draft-composition#task#12 (same cluster also covers resiliencehub-app-version-publish#task#6).

Related but NOT covering this: the open issue 20261029T195027Z-resiliencehub-mints-synthetic-appcomponent-ids-and-drops-physicalresource.md is about AppComponent ids being minted instead of taken from the request's component name, and about missing physicalResource identity members; it says nothing about resolutionId and does not carry this step's replay key.

## Evidence
Step resiliencehub-app-version-draft-composition#task#12, operation ResolveAppVersionResources: expected HTTP 200, got HTTP 200 - the status and all other members agree. Body diff reported by the replay:
  $.resolutionId: value_mismatch (expected '<shaped-not-asserted>', got '18e33715-eb7e-c23c-0000-000000000004')
The same divergence_key covers resiliencehub-app-version-publish#task#6 (service=resiliencehub|ResolveAppVersionResources|status=200->200|keys=$.resolutionId).

Reading the emulator value: '18e33715' and 'eb7e' are the high/mid bits of the minting instant in unix nanoseconds, 'c23c' is the low nanosecond bits sitting where a UUID version nibble belongs (leading nibble 'c', not '4'), '0000' is the high half of the process-wide mint counter sitting where the RFC-4122 variant nibble belongs (leading nibble '0', not 8/9/a/b), and '000000000004' is that counter's low half - the fourth id minted by this process. The specific nanosecond digits and the counter value 4 are per-run values and are NOT the requirement; the requirement is that the id carries no clock or counter structure and satisfies the shape the recording asserts for an AWS resolution id.

Smithy citations (services/service-com-amazonaws-resiliencehub-awsresiliencehub/smithy-model/com-amazonaws-resiliencehub-awsresiliencehub.json): ResolveAppVersionResources output members appArn (Arn, required), appVersion (EntityVersion, required), resolutionId (String255, @length 1..255, required), status (ResourceResolutionStatusType: Failed|InProgress|Pending|Success, required); http POST /resolve-app-version-resources, code 200. DescribeAppVersionResourcesResolutionStatus takes resolutionId (String255) as input. The model states no format for resolutionId beyond the length, so the shape requirement is the recorded one.

Emulator code path (citation only): services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/resolve_app_version_resources.rs:194-201 returns resolution.resolution_id from state::start_resolution; services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/state.rs:223-228 (ResourceResolution::started_at) and :651-668 (mint_resource_id) build the time-and-counter 8-4-4-4-12 string.

## Steps to Reproduce
1. CreateApp(name=<app>) -> appArn.
2. Add at least one Application Component and one resource to the draft version (CreateAppVersionAppComponent, then CreateAppVersionResource), so the draft is resolvable.
3. ResolveAppVersionResources(appArn=<appArn>, appVersion="draft").
4. Inspect the 200 body's $.resolutionId: the emulator returns a pseudo-UUID whose third group holds clock bits, whose fourth group is '0000', and whose last group is a zero-padded mint counter (e.g. ...-0000-00000000000N, N incrementing by one per id the process mints). AWS returns an opaque, randomly generated resolution id with canonical UUID version/variant nibbles and no counter.
5. Call ResolveAppVersionResources twice in a row and compare the two ids: the tails differ by exactly one, confirming the counter is on the wire.
6. Full replay: replay_wire_corpus() and re-check resiliencehub-app-version-draft-composition#task#12 and resiliencehub-app-version-publish#task#6.
