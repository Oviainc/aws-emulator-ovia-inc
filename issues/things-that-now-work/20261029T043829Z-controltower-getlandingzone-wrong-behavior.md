Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: f13a20395d3dd7f956ca916372b3c37cd3e2a76c

# A landing zone minted under a non-canonical signing region is unaddressable: every landingZoneIdentifier operation refuses the ARN CreateLandingZone handed back

_Filed: 2026-10-29T04:38:29Z_
_Operation: GetLandingZone | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f13a20395d3dd7f956ca916372b3c37cd3e2a76c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateLandingZone` builds the landing zone's ARN — which is also the store's key and the only identifier a caller is ever given for a landing zone — by interpolating `crate::state::request_region(ctx.parts())` verbatim. `request_region` returns the region field of the request's SigV4 credential scope with NO normalisation and NO character check, and the framework that parses that scope only rejects an EMPTY region (`vendor/server/src/lib.rs:1800-1802`), while `engine::emulator_arn` interpolates whatever it is given (`vendor/server/src/lib.rs:162-164`).

Every operation that takes a `landingZoneIdentifier` (`GetLandingZone`, `UpdateLandingZone`, `ResetLandingZone`, `DeleteLandingZone`) then runs that identifier through `crate::state::non_landing_zone_identifier_message` -> `is_landing_zone_arn`, which REQUIRES the ARN's region segment to be made only of `[a-z0-9-]` (state.rs:3367-3370). So when a client signs with a region string carrying anything else — an uppercase letter (`US-WEST-2`), an underscore, a dot — the create succeeds, `ListLandingZones` lists the ARN, and every single reporting or mutating call on that ARN answers 400 `ValidationException` saying the ARN "is not the ARN of an AWS Control Tower landing zone" — the exact ARN the same service minted and reported.

The landing zone is then permanently stranded: it cannot be read, updated, reset or DELETED, and because the one-per-account rule is read off `store.landing_zones` (create_landing_zone.rs:253-263), `CreateLandingZone` is refused from then on too ("this account already has a landing zone"). Persisted state that no caller can reach or remove.

The crate states the rule it breaks in two places: `is_landing_zone_arn`'s own doc says the form it checks for IS `landing_zone_arn`'s output, and `non_landing_zone_identifier_message`'s refusal tells the caller a landing zone is named by "the ARN CreateLandingZone and ListLandingZones report for it" — which is precisely the value being refused. The crate's sibling resolution path for the other two resource kinds has no such trap: `resolve_enabled_baseline_arn` matches "against the map key exactly" and `UpdateEnabledControl`/`GetEnabledControl` look the ARN up in the map directly, so only the landing-zone path imposes a shape rule the minting helper is not held to.

The same unchecked value reaches `GetBaseline`, where it breaks the model as well: `GetBaselineOutput$arn` is typed `BaselineArn` (pattern `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$`), and `baseline_arn(&request_region(...), id)` with a non-canonical region produces a value outside that pattern — which also cannot be fed back into `GetBaseline` (whose input carries the same type, so the front door refuses it) or into `EnableBaseline` (whose `is_baseline_arn` applies the same `[a-z0-9-]` region class, state.rs:2706-2708), contradicting the helper's stated round-trip rule. One fix — holding the region that reaches a minted ARN to the form the crate's own ARN recognisers require — covers both.

## Evidence

The mint, taking the signing region verbatim — services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/create_landing_zone.rs:265-266:
```
        let region = crate::state::request_region(ctx.parts());
        let arn = crate::state::landing_zone_arn(&region, &crate::state::new_landing_zone_id());
```
where the region is unchecked — src/state.rs:3491-3495:
```
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
```
and src/state.rs:3414-3420:
```
pub fn landing_zone_arn(region: &str, landing_zone_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("landingzone/{landing_zone_id}"),
    )
}
```

The acceptance rule that rejects it — src/state.rs:3357-3372 (region class at :3367-3370):
```
pub fn is_landing_zone_arn(identifier: &str) -> bool {
    let Some((partition, service, region, account, resource)) = arn_segments(identifier) else {
        return false;
    };
    (20..=2048).contains(&identifier.len())
        && partition.starts_with("aws")
...
        && !region.is_empty()
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
```

The same function's doc, asserting the two are the same form — src/state.rs:3345-3351:
```
/// Whether `identifier` is a landing zone ARN at all.
///
/// A landing zone is addressed by the ARN this service reports for it — the only
/// identifier a caller is ever handed for one (`CreateLandingZoneOutput$arn`,
/// `LandingZoneSummary$arn`, both typed `Arn`: length 20..2048, pattern
/// `^arn:aws[0-9a-zA-Z_\-:\/]+$`) — and the form that ARN carries is
/// [`landing_zone_arn`]'s:
```

The refusal a caller gets for that very ARN — src/state.rs:3390-3399:
```
pub fn non_landing_zone_identifier_message(identifier: &str) -> Option<String> {
    if is_landing_zone_arn(identifier) {
        return None;
    }
    Some(format!(
        "landingZoneIdentifier {identifier:?} is not the ARN of an AWS Control Tower landing \
         zone; a landing zone is named by the ARN CreateLandingZone and ListLandingZones \
         report for it, of the form \
         arn:aws:controltower:<region>:<account>:landingzone/<id>"
```
called before anything is resolved by src/ops/get_landing_zone.rs:165-172, src/ops/update_landing_zone.rs:194-201, src/ops/reset_landing_zone.rs:175-182 and src/ops/delete_landing_zone.rs:183-190, e.g. get_landing_zone.rs:165-172:
```
        if let Some(message) =
            crate::state::non_landing_zone_identifier_message(&input.landing_zone_identifier)
        {
            return Err(
                GetLandingZoneErrorModeled::ValidationException(ValidationException { message })
                    .into(),
            );
        }
```

The framework imposes no region form — vendor/server/src/lib.rs:1800-1802:
```
    if access_key_id.is_empty() || date.is_empty() || region.is_empty() || service.is_empty() {
        return None;
    }
```
and vendor/server/src/lib.rs:162-164:
```
pub fn emulator_arn(service: &str, region: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}")
}
```
(routing reads only the scope's SERVICE field, normalised at vendor/server/src/lib.rs:1512-1516, so the region reaches the handler untouched.)

The sibling resources have no such trap — src/state.rs:1843-1857:
```
    /// The stored ARN an `enabledBaselineIdentifier` names, or `None` when no enabled
    /// baseline matches.
    ///
    /// Matched against the map key exactly, for the same reason
    /// [`resolve_landing_zone_arn`] is: the identifier of an enablement is the ARN this
    /// service minted for it
```

Second symptom, same unchecked value, model-level: GetBaselineOutput$arn is `BaselineArn` — `"BaselineArn": {"type": "string", "pattern": "^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$"}` (smithy_query query="shape" shape="BaselineArn") — while src/ops/get_baseline.rs:194-197 builds it from the same region:
```
            arn: crate::state::baseline_arn(
                &crate::state::request_region(ctx.parts()),
                baseline.id,
            ),
```
and the helper's doc states the round-trip the result then fails — src/state.rs:2648-2649:
```
/// digits in the account position for exactly that reason, and the account-less form is
/// the one a caller round-trips back into `GetBaseline` or `EnableBaseline`.
```
with `is_baseline_arn` applying the same region class at src/state.rs:2706-2708.

Test coverage: the crate's own region test states the rule universally but exercises only canonical regions — tests/all/landing_zone_identifier.rs:330-353:
```
/// RULE (that it is the ARN the service MINTED, wherever it minted it): the identifier a
/// create hands back resolves in the region it was called for, and the resource id that
/// ARN ends in does not — for every region, not only the one the other rows sign for.
#[tokio::test]
async fn only_the_minted_arn_resolves_in_any_region() {
    for region in [SIGNED_REGION, "eu-west-1", "ap-south-1"] {
```
(`SIGNED_REGION = "us-west-2"`, tests/all/landing_zone_lifecycle.rs:111; `call_in_region` substitutes the region straight into the credential scope, tests/all/landing_zone_lifecycle.rs:89-96 and :126-130). No test drives a region outside `[a-z0-9-]`, so nothing in the crate justifies the divergence.

## Steps to Reproduce

1. POST /create-landingzone signed with a credential scope whose region is not all `[a-z0-9-]` — e.g. `Authorization: AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/US-WEST-2/controltower/aws4_request, ...` (the test helper `call_in_region(&router, "US-WEST-2", "/create-landingzone", json!({"version":"3.3","manifest":simple_manifest()}))`) — observe 200 and `arn` = `arn:aws:controltower:US-WEST-2:000000000000:landingzone/<ID>`.
2. POST /list-landingzones with the same signing region — the ARN above is listed, so the landing zone exists and is reportable.
3. POST /get-landingzone with `{"landingZoneIdentifier": "<that exact arn>"}` — observe 400 ValidationException, "landingZoneIdentifier ... is not the ARN of an AWS Control Tower landing zone; a landing zone is named by the ARN CreateLandingZone and ListLandingZones report for it", because `is_landing_zone_arn` (state.rs:3367-3370) rejects the uppercase region segment the mint just wrote.
4. Repeat with /update-landingzone, /reset-landingzone and /delete-landingzone and the same ARN — all four answer the same 400, so the landing zone can never be read, changed or torn down.
5. POST /create-landingzone again (any region) — refused with 400 "this account already has a landing zone (<that arn>)" (create_landing_zone.rs:253-263), so the one-per-account slot is occupied by a resource no call can address or delete.
6. Same signing region, POST /get-baseline with `{"baselineIdentifier":"arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2"}` — the response `arn` comes back as `arn:aws:controltower:US-WEST-2::baseline/17BSJV3IGJ2QSGA2`, outside the `BaselineArn` pattern the model declares for that output member, and sending that value back into /get-baseline or /enable-baseline is refused.
