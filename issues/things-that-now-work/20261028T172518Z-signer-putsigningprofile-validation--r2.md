Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 02dafd84bb90353bc091e97f9b9d9a41e6e06fdd

# PutSigningProfile accepts an overrides.signingImageFormat the model documents as not selectable (JSON, "reserved for future use")

_Filed: 2026-10-28T17:25:18Z_
_Operation: PutSigningProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `02dafd84bb90353bc091e97f9b9d9a41e6e06fdd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutSigningProfile` validates the `overrides.signingConfiguration` half of `SigningPlatformOverrides` against the platform's published option set and refuses a value outside it with a 400, but it never looks at the sibling member `overrides.signingImageFormat` at all. The member is deserialized (`SigningPlatformOverrides.signing_image_format`), stored verbatim on the profile record, reported back by `GetSigningProfile`, snapshotted onto every signing job by `StartSigningJob` and reported again by `DescribeSigningJob` — so the value `JSON`, which the model documents as NOT one of the two formats a customer may select when overriding (it is "reserved for future use"), is accepted, persisted and propagated instead of being refused with the operation's modelled `ValidationException`.

Nothing in the crate holds this member to any option set: `unsupported_signing_configuration_override` reads only `overrides.signing_configuration`, and `SigningConfigurationMember` has exactly two variants (hashAlgorithm, encryptionAlgorithm). `IMAGE_FORMAT_VALUES` (the enum the boundary validator admits) contains `JSON`, so the reserved value reaches the handler and nothing stops it. The crate's own test comment states the rule the code does not implement: it names the member's "own documented options" as the two formats, and then only ever exercises `JSONDetached`.

The fix is confined to the reserved/undocumented value: `JSONEmbedded` and `JSONDetached` must keep being accepted on every platform, which is exactly what the existing test pins.

## Evidence

Smithy, `SigningPlatformOverrides$signingImageFormat` (smithy_query(query="shape", shape="SigningPlatformOverrides")):

> "A signed image is a JSON object. When overriding the default signing platform configuration, a customer can select either of two signing formats, `JSONEmbedded` or `JSONDetached`. (A third format value, `JSON`, is reserved for future use.) With `JSONEmbedded`, the signing image has the payload embedded in it. With `JSONDetached`, the payload is not be embedded in the signing image."

The handler validates only the OTHER half of the same structure — services/service-com-amazonaws-signer-wallabyservice/src/ops/put_signing_profile.rs:234-252:
```
        // An override SELECTS among the algorithms the platform publishes as
        // allowed, so a value outside its `allowedValues` asks for a signing
        // configuration the platform publishes none of — refused for the value it
        // carried (400), before anything is written, so no profile asks to be
        // signed under an algorithm its platform cannot sign under.
        if let Some((member, value)) = crate::state::unsupported_signing_configuration_override(
            platform,
            input.overrides.as_ref(),
        ) {
```
and that helper reads the `signingConfiguration` member only — src/state.rs:1389-1393:
```
pub fn unsupported_signing_configuration_override<'a>(
    entry: &SigningPlatformEntry,
    overrides: Option<&'a SigningPlatformOverrides>,
) -> Option<(SigningConfigurationMember, &'a str)> {
    let configuration = overrides.and_then(|overrides| overrides.signing_configuration.as_ref())?;
```
with only two members to judge (src/state.rs:1352-1359: `SigningConfigurationMember { HashAlgorithm, EncryptionAlgorithm }`).

The member exists and is stored but is never read by any handler: src/ops/types/shared_types.rs:80 `pub signing_image_format: Option<String>,`; src/ops/put_signing_profile.rs:319 `overrides: input.overrides,`; reported at src/ops/get_signing_profile.rs:228 `overrides: profile.overrides.clone(),`, snapshotted at src/ops/start_signing_job.rs:321 `overrides: profile.overrides,` and reported at src/ops/describe_signing_job.rs:216 `overrides: job.overrides.clone(),`. A grep of the crate for `signing_image_format` finds no validation site at all.

The reserved value is admitted by the boundary because the model's enum names it — src/ops/types/shared_types.rs:165:
```
pub const IMAGE_FORMAT_VALUES: &[&str] = &["JSON", "JSONDetached", "JSONEmbedded"];
```

The crate states the rule it does not enforce — tests/all/put_profile_platform_validation.rs:346-350:
```
        // An override of the signing IMAGE FORMAT is a different member with its
        // own documented options — "a customer can select either of two signing
        // formats, JSONEmbedded or JSONDetached"
        // (`SigningPlatformOverrides$signingImageFormat`) — so it rides along with
        // allowed algorithms without changing the verdict.
```

Test coverage: that case sends only `"signingImageFormat": "JSONDetached"` (tests/all/put_profile_platform_validation.rs:358) and tests/all/signing_profile_lifecycle.rs:270 likewise; no test in tests/all/ ever sends `JSON`, so no existing test pins acceptance of the reserved value.

## Steps to Reproduce

1. Boot the emulator.
2. PUT /signing-profiles/Image_Format_Probe with {"platformId":"AWSLambda-SHA384-ECDSA","overrides":{"signingImageFormat":"JSON"}} — observe HTTP 200 with an arn/profileVersion instead of HTTP 400 ValidationException naming the member.
3. GET /signing-profiles/Image_Format_Probe — observe overrides.signingImageFormat == "JSON" persisted and reported.
4. StartSigningJob on that profile and GET /signing-jobs/{jobId} — observe the job reports the same overrides.signingImageFormat == "JSON".
5. Contrast with the sibling member on the same platform: PUT /signing-profiles/Image_Format_Probe2 with {"platformId":"AWSLambda-SHA384-ECDSA","overrides":{"signingConfiguration":{"hashAlgorithm":"SHA256"}}} — refused HTTP 400 ValidationException because SHA256 is outside that platform's allowedValues.
6. Confirm the documented pair still has to be accepted: the same put with "signingImageFormat":"JSONDetached" and with "JSONEmbedded" must stay HTTP 200 (tests/all/put_profile_platform_validation.rs pins JSONDetached).
