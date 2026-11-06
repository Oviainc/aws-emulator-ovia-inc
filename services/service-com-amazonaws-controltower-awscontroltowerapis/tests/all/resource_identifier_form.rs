// SPDX-License-Identifier: CC0-1.0

//! The FORM of the resource ids this service mints, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees.
//!
//! The rule under test, asserted over a TABLE spanning every resource this service mints
//! an id for rather than one of them: a resource id is sixteen characters of `[A-Z0-9]`,
//! which is a DIFFERENT kind of token from an `operationIdentifier`, so the two are never
//! confusable in one response body — and the ARN carrying that id still resolves through
//! the resource's own reporting call.
//!
//! Every form written out by hand here names its source: the resource-id form is
//! `BaselineArn`'s pattern
//! `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$`, the one
//! Control Tower resource ARN the model constrains precisely, and `OperationIdentifier`
//! is separately `@length(36..36)` with pattern
//! `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$` — both from
//! `smithy_query(query="shape", shape="BaselineArn,OperationIdentifier")` on this crate's
//! `smithy-model/`. The landing zone versions the rows create with come from
//! `service_..._awscontroltowerapis::state::CATALOG_LANDING_ZONE_VERSIONS`, which is what
//! `GetLandingZone` reports back.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::baseline_operations::{enable as enable_baseline, enablements};
use crate::enabled_control_lifecycle::{control, enable as enable_control, ou};
use crate::landing_zone_lifecycle::{
    call, call_in_region, emulator, is_operation_identifier, manifest_governing, set_up_emulator,
};

/// How many characters a Control Tower resource id carries: the `[A-Z0-9]{16}` of
/// `BaselineArn`'s pattern.
const RESOURCE_ID_LENGTH: usize = 16;

/// Whether `value` is a Control Tower resource id: exactly [`RESOURCE_ID_LENGTH`]
/// characters, each an ASCII digit or an uppercase ASCII letter.
pub(crate) fn is_resource_id(value: &str) -> bool {
    value.len() == RESOURCE_ID_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
}

/// The reporting call that resolves an ARN of `kind`: its path and the member naming the
/// resource.
fn reader(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "landingzone" => ("/get-landingzone", "landingZoneIdentifier"),
        "enabledcontrol" => ("/get-enabled-control", "enabledControlIdentifier"),
        "enabledbaseline" => ("/get-enabled-baseline", "enabledBaselineIdentifier"),
        other => panic!("no reporting call is known for the resource kind {other:?}"),
    }
}

/// Assert everything the rule says about ONE minted resource, handing back its resource
/// id so the caller can check the ids of a whole table are distinct: the id after the
/// ARN's `kind/` segment is a resource id, it is not an `operationIdentifier` and differs
/// from the one returned beside it, and the ARN resolves through `kind`'s reporting call.
async fn assert_minted(
    router: &MultiServiceRouter,
    case: &str,
    kind: &str,
    arn: &str,
    operation_identifier: &str,
) -> String {
    let id = arn
        .rsplit_once(&format!(":{kind}/"))
        .map(|(_, id)| id.to_string())
        .unwrap_or_default();
    assert!(
        is_resource_id(&id),
        "case {case:?}: expected a {RESOURCE_ID_LENGTH}-character [A-Z0-9] resource id after \
         {kind:?}/, got {id:?} in {arn:?}"
    );
    assert!(
        !is_operation_identifier(&id),
        "case {case:?}: a resource id must not carry the OperationIdentifier form, got \
         {id:?} in {arn:?}"
    );
    assert!(
        is_operation_identifier(operation_identifier),
        "case {case:?}: expected a 36-character lowercase-hex operationIdentifier, got \
         {operation_identifier:?}"
    );
    assert_ne!(
        id, operation_identifier,
        "case {case:?}: the resource id and the operationIdentifier must be different \
         tokens, got {id:?} twice"
    );

    let (path, member) = reader(kind);
    let resolved = call(
        router,
        path,
        serde_json::json!({ member.to_string(): arn }),
    )
    .await;
    assert_eq!(
        resolved.status, 200,
        "case {case:?}: expected the minted ARN {arn:?} to resolve through {path}, got {} \
         body {:?}",
        resolved.status, resolved.body
    );

    id
}

/// RULE: every resource id this service mints is sixteen characters of `[A-Z0-9]` — the
/// form the model's one precisely constrained Control Tower resource ARN spells out —
/// never the hyphenated lowercase-hex UUID an `operationIdentifier` carries, so a
/// response's resource id and its operation identifier stay distinguishable tokens; the
/// minted ARN still addresses the resource.
///
/// The rows span every resource this service mints an id for and the inputs the rule
/// generalises over: the landing zone version, its governed-region list and the signed
/// region the ARN is minted in; the catalogued control and its organizational-unit
/// target; and the whole `enablements()` baseline table (several baselines, versions,
/// targets, parameter lists and tag maps).
#[tokio::test]
async fn every_minted_resource_id_is_sixteen_uppercase_alphanumerics() {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut kinds: BTreeSet<String> = BTreeSet::new();

    // An account holds at most one landing zone, so each landing zone row gets its own
    // emulator; the rows differ in the version, the governed-region list and the region
    // the request is signed for, which is the one the ARN is minted in.
    for (case, region, version, governed) in [
        (
            "landing zone at the oldest offered version",
            "us-west-2",
            "2.0",
            vec!["us-west-2"],
        ),
        (
            "landing zone at the newest offered version, two governed regions",
            "eu-west-1",
            "3.3",
            vec!["eu-west-1", "us-west-2"],
        ),
        (
            "landing zone at a version in between",
            "ap-south-1",
            "3.1",
            vec!["ap-south-1"],
        ),
    ] {
        let router = emulator();
        let created = call_in_region(
            &router,
            region,
            "/create-landingzone",
            serde_json::json!({
                "version": version,
                "manifest": manifest_governing(&governed),
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected create-landingzone to answer 200, got {} body {:?}",
            created.status, created.body
        );
        let id = assert_minted(
            &router,
            case,
            "landingzone",
            &created.string("arn"),
            &created.string("operationIdentifier"),
        )
        .await;
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: resource id {id:?} was already minted by an earlier call"
        );
        kinds.insert("landingzone".to_string());
    }

    // Enabled controls and enabled baselines share one emulator: neither is bounded to
    // one per account, so every row's id is minted from the same store. The account is set
    // up first, because Control Tower enables a control or a baseline only for an account
    // it has been set up in.
    let router = set_up_emulator().await;
    for (case, control_identifier, target) in [
        (
            "enabled control on an organizational unit",
            "AWS-GR_ENCRYPTED_VOLUMES",
            "4aa1",
        ),
        (
            "another control on the same target",
            "AWS-GR_RESTRICT_ROOT_USER",
            "4aa1",
        ),
        (
            "the first control on another target",
            "AWS-GR_ENCRYPTED_VOLUMES",
            "4aa2",
        ),
    ] {
        let (arn, operation_identifier) = enable_control(
            &router,
            serde_json::json!({
                "controlIdentifier": control(control_identifier),
                "targetIdentifier": ou(target),
            }),
        )
        .await;
        let id = assert_minted(&router, case, "enabledcontrol", &arn, &operation_identifier).await;
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: resource id {id:?} was already minted by an earlier call"
        );
        kinds.insert("enabledcontrol".to_string());
    }

    for row in enablements() {
        let enabled = enable_baseline(&router, &row).await;
        assert_eq!(
            enabled.status, 200,
            "case {:?}: expected enable-baseline to answer 200, got {} body {:?}",
            row.case, enabled.status, enabled.body
        );
        let case = format!("enabled baseline: {}", row.case);
        let id = assert_minted(
            &router,
            &case,
            "enabledbaseline",
            &enabled.string("arn"),
            &enabled.string("operationIdentifier"),
        )
        .await;
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: resource id {id:?} was already minted by an earlier call"
        );
        kinds.insert("enabledbaseline".to_string());
    }

    assert_eq!(
        kinds,
        BTreeSet::from([
            "enabledbaseline".to_string(),
            "enabledcontrol".to_string(),
            "landingzone".to_string(),
        ]),
        "expected one row per minted resource kind, got {kinds:?}"
    );
}

/// RULE (as an invariant over a BATCH): the minter draws every resource id from the whole
/// `[A-Z0-9]` alphabet at the full width, and no two mints collide — a batch is where both
/// are visible, because uniqueness and the alphabet's coverage are properties of a SET.
#[tokio::test]
async fn minted_resource_ids_are_unique_and_span_the_alphabet() {
    // Bounded, and large enough that same-tick mints occur: a tight loop mints far
    // faster than the clock's resolution. 512 ids carry 8192 characters, so a character
    // the minter can emit is missing from the batch only with vanishing probability.
    const MINTS: usize = 512;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut characters: BTreeSet<char> = BTreeSet::new();
    for ordinal in 0..MINTS {
        let id = service_com_amazonaws_controltower_awscontroltowerapis::state::new_resource_id();
        assert!(
            is_resource_id(&id),
            "mint {ordinal}: expected a {RESOURCE_ID_LENGTH}-character [A-Z0-9] resource id, \
             got {id:?}"
        );
        characters.extend(id.chars());
        assert!(
            seen.insert(id.clone()),
            "mint {ordinal}: {id:?} was already minted — resource ids must be unique"
        );
    }
    assert_eq!(
        seen.len(),
        MINTS,
        "expected {MINTS} distinct resource ids, got {}",
        seen.len()
    );

    // The alphabet of `[A-Z0-9]`: 10 digits and 26 uppercase letters.
    let alphabet: BTreeSet<char> = ('0'..='9').chain('A'..='Z').collect();
    let missing: Vec<char> = alphabet.difference(&characters).copied().collect();
    assert!(
        missing.is_empty(),
        "expected the minted ids to span the whole [A-Z0-9] alphabet, these characters \
         never appeared: {missing:?}"
    );
    let unexpected: Vec<char> = characters.difference(&alphabet).copied().collect();
    assert!(
        unexpected.is_empty(),
        "expected only [A-Z0-9] characters in minted ids, got {unexpected:?}"
    );
}
