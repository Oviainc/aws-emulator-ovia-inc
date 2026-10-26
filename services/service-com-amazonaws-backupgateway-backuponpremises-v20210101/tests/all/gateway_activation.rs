// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACTIVATION of a backup gateway: where an `ActivationKey` comes
//! from, and what `CreateGateway` does with the one a client presents.
//!
//! Every case drives real requests through the `MultiServiceRouter` — the harness of
//! `gateway_lifecycle` — so what is asserted is the wire behaviour a client sees, and the
//! only way a later request can see what an earlier one wrote is the shared store.
//!
//! The rule under test: an activation key is ISSUED by the activation handshake a deployed
//! appliance makes (the unsigned `GET /?activationRegion=…` this service serves) and
//! REDEEMED by the create that activates that appliance, so `CreateGateway` mints a gateway
//! only for a key this service issued for the region the request is addressed to and has not
//! redeemed yet; every other key — one nothing issued, one already redeemed, one issued for
//! another region — is refused with HTTP 400 `ValidationException` / `ErrorCode`
//! `ActivationKeyNotFound` and "The specified activation key was not found.", before
//! anything is written, and a key the modelled constraints refuse (len 1..50, pattern
//! `^[0-9a-zA-Z\-]+$`, `smithy_query(query="operation",operation="CreateGateway")`) is
//! refused by the boundary validator with the same modelled error.
//!
//! The tables span the class each rule generalises over: keys differing in length (both
//! published bounds and one past each), case, digit-only and hyphen-only spelling, and the
//! five-group form an appliance prints — a key of exactly the shape this service ISSUES but
//! that no handshake issued is refused like any other, so what decides is the activation
//! record and not the spelling; regions differing from the one a key was issued for; and,
//! on the boundary-refusal side, every class of character the pattern excludes.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use serde_json::json;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, BACKUP_VM, MAX_PENDING_ACTIVATION_KEYS};

use crate::gateway_lifecycle::{
    activation_handshake, build_engine, build_router, call, call_in_region, create_body,
    gateway_count, issued_activation_key, issued_activation_key_for, pending_activation_count,
    REGION,
};

/// The sentence AWS states for a key that names no activation
/// (`issues/.../backup-gateway-negative-paths-succeed-or-return-the.md`), spelled out here
/// rather than read off the crate, so the assertion is independent of the code under test.
const NOT_FOUND_MESSAGE: &str = "The specified activation key was not found.";

/// The `ValidationException$ErrorCode` that refusal carries.
const NOT_FOUND_CODE: &str = "ActivationKeyNotFound";

/// The modelled `ActivationKey` constraints: `@length(min: 1, max: 50)` and `@pattern
/// ^[0-9a-zA-Z\-]+$` (`smithy_query(query="operation",operation="CreateGateway")`), spelled
/// out here so an issued key is checked against the MODEL rather than against the minting
/// code.
fn satisfies_modelled_activation_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 50
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Keys the modelled constraints accept and no handshake ever issued.
///
/// The rows differ along everything the rule generalises over: both published length
/// bounds, digits only, hyphens only, mixed case, a lower-case key of the form a client
/// would invent, and the five hyphenated groups of five an appliance prints — the shape this
/// service's OWN issued keys take, which is refused all the same because what decides is the
/// activation record.
fn unissued_rows() -> Vec<(&'static str, String)> {
    vec![
        ("one character, the modelled length minimum", "k".to_string()),
        ("two characters", "k9".to_string()),
        ("digits only", "0123456789".to_string()),
        ("hyphens only", "---".to_string()),
        ("hyphens at both ends", "-ABCDEF123456-".to_string()),
        ("mixed case and hyphens", "aB3-dE6".to_string()),
        (
            "the five hyphenated groups an appliance prints",
            "29AV1-3OFV9-VVIUB-NKT0I-LRO6V".to_string(),
        ),
        (
            "a lower-case key a client invented",
            "unissued-activation-key-0001".to_string(),
        ),
        ("at the 50-character maximum", "K".repeat(50)),
    ]
}

/// Keys the modelled constraints refuse: one past each published length bound, and one per
/// class of character the pattern excludes.
fn malformed_rows() -> Vec<(&'static str, String)> {
    vec![
        ("empty, one below the length minimum", String::new()),
        (
            "51 characters, one past the length maximum",
            "K".repeat(51),
        ),
        ("an underscore", "key_1".to_string()),
        ("a space", "key 1".to_string()),
        ("a dot", "key.1".to_string()),
        ("a non-ASCII letter", "kéy".to_string()),
        ("a plus sign", "key+1".to_string()),
    ]
}

/// The handshake issues a key of the modelled form, one per request, and records each one
/// pending.
#[tokio::test]
async fn the_activation_handshake_issues_one_key_per_request() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, the query the handshake carries) — the documented request names the region and
    // the gateway type, and a client may ask for the key without the redirect; a parameter
    // this service does not read changes nothing.
    let rows: [(&str, String); 5] = [
        ("the region alone", format!("activationRegion={REGION}")),
        (
            "the region and the gateway type",
            format!("activationRegion={REGION}&gatewayType={BACKUP_VM}"),
        ),
        (
            "the documented request, with no_redirect",
            format!("activationRegion={REGION}&gatewayType={BACKUP_VM}&no_redirect"),
        ),
        (
            "another region",
            "activationRegion=eu-central-1".to_string(),
        ),
        (
            "the region, asked for twice",
            format!("activationRegion={REGION}&activationRegion=us-east-1"),
        ),
    ];

    let mut keys = BTreeSet::new();
    for (index, (case, query)) in rows.iter().enumerate() {
        let (status, key) = activation_handshake(&router, query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the handshake must issue a key, got {status} {key:?}"
        );
        assert!(
            satisfies_modelled_activation_key(&key),
            "case {case:?}: an issued key must satisfy the modelled ActivationKey \
             constraints, got {key:?}"
        );
        assert!(
            keys.insert(key.clone()),
            "case {case:?}: the key {key:?} was already issued to another handshake"
        );
        assert_eq!(
            pending_activation_count(&engine),
            index + 1,
            "case {case:?}: every handshake must record exactly one pending activation, \
             got {}",
            pending_activation_count(&engine)
        );
    }

    // Nothing the handshake does creates a gateway: a key is an activation, not a resource.
    assert_eq!(
        gateway_count(&engine),
        0,
        "the handshake must create no gateway, got {}",
        gateway_count(&engine)
    );
}

/// A handshake this service cannot serve is refused, and records nothing.
#[tokio::test]
async fn a_handshake_this_service_cannot_serve_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, the query the handshake carries, what the refusal must name)
    //
    // `GatewayType` declares exactly one value, `BACKUP_VM`
    // (`smithy_query(query="shape",shape="GatewayType")`), so every other spelling names a
    // type this service cannot activate; and the region a handshake names is kept on the
    // activation it records, so an over-long one is refused rather than stored.
    let rows: [(&str, String, &str); 9] = [
        (
            "another gateway type",
            format!("activationRegion={REGION}&gatewayType=BACKUP_PHYSICAL"),
            BACKUP_VM,
        ),
        (
            "the one gateway type, misspelled",
            format!("activationRegion={REGION}&gatewayType=backup_vm"),
            BACKUP_VM,
        ),
        (
            "a storage-gateway type",
            format!("activationRegion={REGION}&gatewayType=FILE_S3"),
            BACKUP_VM,
        ),
        (
            "an empty gateway type",
            format!("activationRegion={REGION}&gatewayType="),
            BACKUP_VM,
        ),
        (
            "a region one character past the bound",
            format!("activationRegion={}", "a".repeat(65)),
            "64",
        ),
        (
            "a region far past the bound",
            format!("activationRegion={}", "r".repeat(4_096)),
            "64",
        ),
        (
            "a region spelled with an underscore",
            "activationRegion=us_west_2".to_string(),
            "letters, digits and hyphens",
        ),
        (
            "a region carrying a path separator",
            "activationRegion=us-west-2%2F..%2Fus-east-1".to_string(),
            "letters, digits and hyphens",
        ),
        (
            "an empty region",
            "activationRegion=".to_string(),
            "letters, digits and hyphens",
        ),
    ];

    for (case, query, named) in &rows {
        let (status, body) = activation_handshake(&router, query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a handshake this service cannot serve must be refused, got \
             {status} {body:?}"
        );
        assert!(
            body.contains(named),
            "case {case:?}: the refusal must name {named:?}, got {body:?}"
        );
        assert!(
            !body.contains(query.as_str()),
            "case {case:?}: the refusal must not echo the request back, got {body:?}"
        );
    }
    assert_eq!(
        pending_activation_count(&engine),
        0,
        "a refused handshake must record no activation, got {}",
        pending_activation_count(&engine)
    );
}

/// An activation key the service never issued is refused, and writes nothing.
#[tokio::test]
async fn an_activation_key_the_service_never_issued_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    for (case, key) in &unissued_rows() {
        let display_name = format!("gw-unissued-{}", case.len());
        let refused = call(
            &router,
            "CreateGateway",
            &create_body(&display_name, key, &BTreeMap::new()),
        )
        .await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
                refused.str_at("/Message"),
            ),
            (
                400,
                Some("ValidationException"),
                Some(NOT_FOUND_CODE),
                Some(NOT_FOUND_MESSAGE)
            ),
            "case {case:?}: a key naming no activation must be refused with 400 \
             ValidationException / {NOT_FOUND_CODE}, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.str_at("/GatewayArn").is_none(),
            "case {case:?}: a refused create must hand back no GatewayArn, got {:?}",
            refused.body
        );

        // The refusal is atomic: no record, and nothing for a listing to report.
        assert_eq!(
            gateway_count(&engine),
            0,
            "case {case:?}: a refused create must leave the store empty, got {}",
            gateway_count(&engine)
        );
        let listed = call(&router, "ListGateways", &json!({})).await;
        let names: Vec<String> = listed
            .body
            .get("Gateways")
            .and_then(|gateways| gateways.as_array())
            .map(|gateways| {
                gateways
                    .iter()
                    .map(|gateway| gateway["GatewayDisplayName"].to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            (listed.status, names.as_slice()),
            (200, &[] as &[String]),
            "case {case:?}: the display name {display_name:?} must never be listed, got {:?}",
            listed.body
        );
    }
}

/// An activation key outside the modelled constraints is refused, and writes nothing.
#[tokio::test]
async fn an_activation_key_outside_the_modelled_constraints_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    for (case, key) in &malformed_rows() {
        let refused = call(
            &router,
            "CreateGateway",
            &create_body("gw-malformed-key", key, &BTreeMap::new()),
        )
        .await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: an ActivationKey outside the modelled constraints must be \
             refused with 400 ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.str_at("/GatewayArn").is_none(),
            "case {case:?}: a refused create must hand back no GatewayArn, got {:?}",
            refused.body
        );
    }

    assert_eq!(
        gateway_count(&engine),
        0,
        "a refused create must leave the store empty, got {}",
        gateway_count(&engine)
    );
    let listed = call(&router, "ListGateways", &json!({})).await;
    assert_eq!(
        (listed.status, listed.gateway_arns()),
        (200, Vec::<String>::new()),
        "a refused CreateGateway must leave no gateway for ListGateways to report, got {:?}",
        listed.body
    );
}

/// Every issued activation key mints its own gateway, which reads back — and the key it
/// redeemed is then spent.
#[tokio::test]
async fn every_issued_activation_key_mints_one_gateway_and_is_then_spent() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, GatewayDisplayName, Tags) — the rows differ in what the create carries BESIDE
    // the activation, which the redemption does not key on.
    let rows: [(&str, &str, BTreeMap<String, String>); 4] = [
        ("no tags", "gw-activated-plain", BTreeMap::new()),
        (
            "one tag",
            "gw-activated-tagged",
            [("env".to_string(), "test".to_string())]
                .into_iter()
                .collect(),
        ),
        (
            "two tags",
            "g",
            [
                ("env".to_string(), "prod".to_string()),
                ("owner".to_string(), "qa".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        (
            "the longest display name the model allows",
            // Name: len 1..100 (`smithy_query(query="shape",shape="Name")`).
            "g1234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567",
            BTreeMap::new(),
        ),
    ];

    let mut arns = BTreeSet::new();
    for (case, display_name, tags) in &rows {
        let key = issued_activation_key(&router).await;
        let created = call(
            &router,
            "CreateGateway",
            &create_body(display_name, &key, tags),
        )
        .await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: an issued ActivationKey must create a gateway, got {} {:?}",
            created.status,
            created.body
        );
        let arn = created.gateway_arn().to_string();
        assert!(
            arn.starts_with(&format!("arn:aws:backup-gateway:{REGION}:"))
                && arn.contains(":gateway/"),
            "case {case:?}: the create must report a generated gateway ARN, got {arn:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: the ARN {arn:?} was already handed to another gateway"
        );

        // The round trip: the record the create persisted is what a read reports.
        let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": arn })).await;
        assert_eq!(
            (
                fetched.status,
                fetched.str_at("/Gateway/GatewayArn"),
                fetched.str_at("/Gateway/GatewayDisplayName"),
                fetched.str_at("/Gateway/GatewayType"),
            ),
            (
                200,
                Some(arn.as_str()),
                Some(*display_name),
                Some(BACKUP_VM)
            ),
            "case {case:?}: the created gateway must read back through GetGateway, got {} {:?}",
            fetched.status,
            fetched.body
        );

        // The key identifies the appliance, and no member of the reported `Gateway` carries
        // it, so a read repeats neither the member nor its value.
        let reported = fetched
            .body
            .get("Gateway")
            .and_then(|gateway| gateway.as_object())
            .unwrap_or_else(|| panic!("case {case:?}: no Gateway object in {:?}", fetched.body));
        assert!(
            !reported.contains_key("ActivationKey"),
            "case {case:?}: the reported gateway must carry no ActivationKey member, got \
             {reported:?}"
        );
        assert!(
            reported
                .values()
                .all(|value| value.as_str() != Some(key.as_str())),
            "case {case:?}: no member of the reported gateway may repeat the activation \
             key, got {reported:?}"
        );

        // The activation is SPENT: the same key cannot activate a second gateway. The
        // second create carries a display name of its own, inside the modelled length, so
        // what refuses it is the spent activation and not its name.
        let replayed = call(
            &router,
            "CreateGateway",
            &create_body(
                &format!("gw-replayed-{}", case.len()),
                &key,
                &BTreeMap::new(),
            ),
        )
        .await;
        assert_eq!(
            (
                replayed.status,
                replayed.code.as_deref(),
                replayed.str_at("/ErrorCode"),
                replayed.str_at("/Message"),
            ),
            (
                400,
                Some("ValidationException"),
                Some(NOT_FOUND_CODE),
                Some(NOT_FOUND_MESSAGE)
            ),
            "case {case:?}: a redeemed activation key must not activate a second gateway, \
             got {} {:?}",
            replayed.status,
            replayed.body
        );
    }

    // One record per redeemed activation, and nothing pending that a create did not spend.
    assert_eq!(
        (gateway_count(&engine), pending_activation_count(&engine)),
        (rows.len(), 0),
        "the store must hold one gateway per redeemed activation and no pending key, got \
         {} and {}",
        gateway_count(&engine),
        pending_activation_count(&engine)
    );
    let listed = call(&router, "ListGateways", &json!({})).await;
    assert_eq!(
        (
            listed.status,
            listed.gateway_arns().into_iter().collect::<BTreeSet<_>>()
        ),
        (200, arns.clone()),
        "ListGateways must report exactly the gateways the creates minted, got {} {:?}",
        listed.status,
        listed.body
    );
}

/// The activation registry is bounded: at its ceiling a new handshake drops the key that
/// has been pending longest, and that key then activates nothing.
#[tokio::test]
async fn the_activation_registry_is_bounded_and_drops_the_key_pending_longest() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Fill the registry to its ceiling through the state API rather than a thousand
    // requests: what is under test is the behaviour AT the ceiling. Each key is issued an
    // instant later than the one before, so which key is the oldest is unambiguous.
    let (oldest, newest) = {
        let state = service_state(&*engine).expect("the in-memory engine must expose this state");
        let mut store = state.lock();
        let base = SystemTime::now();
        let issued: Vec<String> = (0..MAX_PENDING_ACTIVATION_KEYS)
            .map(|index| {
                store.issue_activation_key(REGION, base + Duration::from_millis(index as u64))
            })
            .collect();
        assert_eq!(
            store.pending_activation_count(),
            MAX_PENDING_ACTIVATION_KEYS,
            "case seed: the registry must hold the ceiling before the next handshake, got {}",
            store.pending_activation_count()
        );
        (
            issued.first().expect("the ceiling is not zero").clone(),
            issued.last().expect("the ceiling is not zero").clone(),
        )
    };

    // One more handshake: it is served, and the registry still holds exactly the ceiling.
    let fresh = issued_activation_key(&router).await;
    assert_eq!(
        pending_activation_count(&engine),
        MAX_PENDING_ACTIVATION_KEYS,
        "the registry must not grow past its ceiling, got {}",
        pending_activation_count(&engine)
    );

    // (case, the key the create presents, whether the create must be served)
    let rows: [(&str, &str, bool); 3] = [
        ("the key pending longest, now dropped", oldest.as_str(), false),
        ("the key the last handshake issued", fresh.as_str(), true),
        ("a key the eviction did not reach", newest.as_str(), true),
    ];

    let mut served = 0usize;
    for (case, key, must_be_served) in &rows {
        let reply = call(
            &router,
            "CreateGateway",
            &create_body(&format!("gw-bound-{}", case.len()), key, &BTreeMap::new()),
        )
        .await;
        if *must_be_served {
            served += 1;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: an activation the registry still holds must be redeemable, \
                 got {} {:?}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                (
                    reply.status,
                    reply.code.as_deref(),
                    reply.str_at("/ErrorCode"),
                    reply.str_at("/Message"),
                ),
                (
                    400,
                    Some("ValidationException"),
                    Some(NOT_FOUND_CODE),
                    Some(NOT_FOUND_MESSAGE)
                ),
                "case {case:?}: a dropped activation must be refused like one never issued, \
                 got {} {:?}",
                reply.status,
                reply.body
            );
        }
        assert_eq!(
            gateway_count(&engine),
            served,
            "case {case:?}: only a redeemed activation may add a record, got {}",
            gateway_count(&engine)
        );
    }
}

/// An activation key is issued for ONE region: a create addressed to another region's
/// endpoint presents a key that region never issued.
#[tokio::test]
async fn an_activation_key_is_redeemable_only_in_the_region_it_was_issued_for() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, the region the handshake asked the key for, the region the create is addressed
    // to, whether the create must be served)
    let rows: [(&str, &str, &str, bool); 5] = [
        ("the region it was issued for", REGION, REGION, true),
        ("another region's endpoint", REGION, "us-east-1", false),
        (
            "issued elsewhere, created here",
            "eu-central-1",
            REGION,
            false,
        ),
        (
            "a second region, in its own region",
            "ap-southeast-2",
            "ap-southeast-2",
            true,
        ),
        (
            "a region that differs only in its suffix",
            "us-west-1",
            "us-west-2",
            false,
        ),
    ];

    let mut served = 0usize;
    for (case, issued_in, created_in, must_be_served) in &rows {
        let key = issued_activation_key_for(&router, issued_in).await;
        let display_name = format!("gw-region-{}", case.len());
        let reply = call_in_region(
            &router,
            created_in,
            "CreateGateway",
            &create_body(&display_name, &key, &BTreeMap::new()),
        )
        .await;
        if *must_be_served {
            served += 1;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a key issued for {issued_in:?} must be redeemable there, \
                 got {} {:?}",
                reply.status, reply.body
            );
            let arn = reply.gateway_arn().to_string();
            assert!(
                arn.starts_with(&format!("arn:aws:backup-gateway:{created_in}:")),
                "case {case:?}: the minted ARN must carry the region the create was \
                 addressed to, got {arn:?}"
            );
        } else {
            assert_eq!(
                (
                    reply.status,
                    reply.code.as_deref(),
                    reply.str_at("/ErrorCode"),
                    reply.str_at("/Message"),
                ),
                (
                    400,
                    Some("ValidationException"),
                    Some(NOT_FOUND_CODE),
                    Some(NOT_FOUND_MESSAGE)
                ),
                "case {case:?}: a key issued for {issued_in:?} must not be redeemable in \
                 {created_in:?}, got {} {:?}",
                reply.status,
                reply.body
            );
        }
        assert_eq!(
            gateway_count(&engine),
            served,
            "case {case:?}: only a redeemed activation may add a record, got {}",
            gateway_count(&engine)
        );
    }
}
