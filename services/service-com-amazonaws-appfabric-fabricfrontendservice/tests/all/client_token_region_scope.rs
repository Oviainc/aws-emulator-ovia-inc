// SPDX-License-Identifier: CC0-1.0

//! The REGION an idempotency token is scoped to, driven over the wire (restJson1)
//! through the same router the emulator boots.
//!
//! RULE: an idempotency token is spent against the Region the call was signed for
//! as well as the operation it was passed to, because every Region is its own
//! endpoint holding its own resources — so the same token with the same body in a
//! second Region is a fresh create THERE (a new resource, whose ARN names that
//! Region), never a replay of the first Region's resource and never an
//! `IdempotentParameterMismatch` conflict; within ONE Region the token still
//! replays its resource, and still conflicts when the parameters differ.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `uuid_of`,
//! `fixture_region`) is the one `app_bundle_ingestion_lifecycle` builds; only the
//! Region-pair tables are new here.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, fixture_region, uuid_of, Answer, Emulator,
};

/// A client token in the form the model's `UUID` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), built from
/// `seed` so a test can name the same token twice.
fn token(seed: u32) -> String {
    format!("{seed:08x}-1111-4111-8111-111111111111")
}

/// The create bodies the rule has to hold over: the optional members of
/// `CreateAppBundleRequest` (`customerManagedKeyIdentifier`, `tags`) beside the
/// token, because none of them may decide whether a token is Region-scoped.
///
/// Each row's body carries its own token (rows differ in token value too), and
/// the key identifier spans both forms the member accepts
/// (`^arn:.+$|^[a-f0-9]{8}-…$`).
fn bundle_bodies() -> Vec<(&'static str, Value)> {
    vec![
        ("the token alone", json!({"clientToken": token(0x10)})),
        (
            "a key by bare id",
            json!({"clientToken": token(0x11),
                   "customerManagedKeyIdentifier": "11111111-2222-4333-8444-555555555555"}),
        ),
        (
            "a key by ARN",
            json!({"clientToken": token(0x12),
                   "customerManagedKeyIdentifier":
                       "arn:aws:kms:us-east-1:000000000000:key/66666666-7777-4888-8999-aaaaaaaaaaaa"}),
        ),
        (
            "two tags",
            json!({"clientToken": token(0x13),
                   "tags": [{"key": "env", "value": "prod"}, {"key": "team", "value": "data"}]}),
        ),
        (
            "an empty tag list",
            json!({"clientToken": token(0x14), "tags": []}),
        ),
        (
            "a key and a tag",
            json!({"clientToken": token(0x15),
                   "customerManagedKeyIdentifier": "99999999-8888-4777-8666-555555555555",
                   "tags": [{"key": "only", "value": "one"}]}),
        ),
    ]
}

/// The Region pair row `index` runs in: two distinct Regions, drawn from the
/// fixture list so the pairs differ along both halves and not every row uses the
/// default scope.
fn region_pair(index: usize) -> (String, String) {
    (fixture_region(index), fixture_region(index + 1))
}

/// `GET /appbundles/{appBundleIdentifier}`, signed for `region`.
async fn get_bundle(emulator: &Emulator, region: &str, identifier: &str) -> Answer {
    emulator
        .call_in_region(
            region,
            "GET",
            &format!("/appbundles/{}", encode(identifier)),
            None,
        )
        .await
}

/// Assert a create answered 201 and report the created resource's ARN.
fn created_arn(answer: &Answer, resource: &str, case: &str) -> String {
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected HTTP 201, got {} with body {}",
        answer.status, answer.body
    );
    arn_of(&answer.body, resource)
}

/// RULE: the same token with the same body in another Region is a create in THAT
/// Region — a distinct bundle whose ARN names the Region the caller signed for —
/// while each Region keeps replaying its own bundle for that token.
#[tokio::test]
async fn a_token_spent_in_one_region_creates_again_in_another() {
    for (index, (case, body)) in bundle_bodies().into_iter().enumerate() {
        let (first_region, second_region) = region_pair(index);
        let case = format!("{case} across {first_region} and {second_region}");
        // A fresh store per row: a Region holds only one app bundle.
        let emulator = Emulator::new();

        let first = emulator
            .create_app_bundle_in_region(&first_region, body.clone())
            .await;
        let first_arn = created_arn(&first, "appBundle", &case);

        // The byte-for-byte identical body, signed for another Region.
        let second = emulator
            .create_app_bundle_in_region(&second_region, body.clone())
            .await;
        let second_arn = created_arn(&second, "appBundle", &case);

        assert_ne!(
            second_arn, first_arn,
            "case {case:?}: a token spent in {first_region} must not replay its bundle in \
             {second_region}, got {second_arn:?}"
        );
        for (region, arn) in [
            (&first_region, &first_arn),
            (&second_region, &second_arn),
        ] {
            assert_eq!(
                arn,
                &format!(
                    "arn:aws:appfabric:{region}:000000000000:appbundle/{}",
                    uuid_of(arn)
                ),
                "case {case:?}: a bundle must carry the ARN of the Region it was created in, \
                 got {arn:?}"
            );
            // Each Region's bundle is PERSISTED, readable through GetAppBundle.
            let read = get_bundle(&emulator, region, arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 reading {arn:?}, got {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.body.get("appBundle"),
                if region == &first_region {
                    first.body.get("appBundle")
                } else {
                    second.body.get("appBundle")
                },
                "case {case:?}: expected the stored record for {arn:?}, got {}",
                read.body
            );
        }

        // Within a Region the token is still spent: each Region replays ITS OWN
        // bundle, however often the retry comes.
        for retry in 1..=2 {
            for (region, expected) in [
                (&first_region, &first_arn),
                (&second_region, &second_arn),
            ] {
                let replay = emulator
                    .create_app_bundle_in_region(region, body.clone())
                    .await;
                assert_eq!(
                    replay.status, 201,
                    "case {case:?} retry {retry} in {region}: expected HTTP 201, got {} with \
                     body {}",
                    replay.status, replay.body
                );
                assert_eq!(
                    &arn_of(&replay.body, "appBundle"),
                    expected,
                    "case {case:?} retry {retry} in {region}: expected the replayed bundle \
                     {expected:?}, got {}",
                    replay.body
                );
            }
        }
    }
}

/// RULE: because the token is Region-scoped, the parameter-mismatch conflict is a
/// WITHIN-Region judgement: the same token with different parameters in the Region
/// that spent it is HTTP 409 `ConflictException`, while the same token in another
/// Region — whatever its parameters — is a create there.
#[tokio::test]
async fn the_parameter_mismatch_conflict_is_judged_within_one_region() {
    // (case, the body the token was first spent on, a body differing in one
    // member, which the mismatch check has to tell apart)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a tag appears",
            json!({"clientToken": token(0x20)}),
            json!({"clientToken": token(0x20), "tags": [{"key": "a", "value": "1"}]}),
        ),
        (
            "a tag value differs",
            json!({"clientToken": token(0x21), "tags": [{"key": "a", "value": "1"}]}),
            json!({"clientToken": token(0x21), "tags": [{"key": "a", "value": "2"}]}),
        ),
        (
            "another customer-managed key",
            json!({"clientToken": token(0x22),
                   "customerManagedKeyIdentifier": "11111111-2222-4333-8444-555555555555"}),
            json!({"clientToken": token(0x22),
                   "customerManagedKeyIdentifier": "66666666-7777-4888-8999-aaaaaaaaaaaa"}),
        ),
        (
            "the key is dropped",
            json!({"clientToken": token(0x23),
                   "customerManagedKeyIdentifier": "11111111-2222-4333-8444-555555555555"}),
            json!({"clientToken": token(0x23)}),
        ),
    ];

    for (index, (case, first_body, other_body)) in cases.into_iter().enumerate() {
        let (first_region, second_region) = region_pair(index);
        let case = format!("{case} across {first_region} and {second_region}");
        let emulator = Emulator::new();

        let first = emulator
            .create_app_bundle_in_region(&first_region, first_body.clone())
            .await;
        let first_arn = created_arn(&first, "appBundle", &case);

        // Same Region, different parameters: the documented mismatch.
        let conflict = emulator
            .create_app_bundle_in_region(&first_region, other_body.clone())
            .await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: expected HTTP 409 for a parameter mismatch in {first_region}, got {} \
             with body {}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {}",
            conflict.body
        );

        // Another Region has not spent this token, so the SAME differing body is
        // simply a create there — not a mismatch, and not a replay.
        let elsewhere = emulator
            .create_app_bundle_in_region(&second_region, other_body)
            .await;
        let elsewhere_arn = created_arn(&elsewhere, "appBundle", &case);
        assert_ne!(
            elsewhere_arn, first_arn,
            "case {case:?}: a token unspent in {second_region} must mint a bundle there, got \
             {elsewhere_arn:?}"
        );
        assert!(
            elsewhere_arn.contains(&format!(":appfabric:{second_region}:")),
            "case {case:?}: expected an ARN in {second_region}, got {elsewhere_arn:?}"
        );
    }
}

/// RULE: the scope is the same for the nested creates — a token spent on
/// `CreateIngestion` in one Region does not replay that ingestion for a call
/// signed for another Region, which gets its own ingestion carrying its own
/// Region's ARN.
#[tokio::test]
async fn the_region_scope_holds_for_a_nested_create() {
    // The ingestion bodies the rule holds over: the token beside the members that
    // identify an ingestion (`app`, `tenantId`, `ingestionType`).
    let bodies: Vec<(&str, Value)> = vec![
        (
            "the token alone",
            json!({"clientToken": token(0x30), "app": "SLACK", "tenantId": "t-1",
                   "ingestionType": "auditLog"}),
        ),
        (
            "another tenant",
            json!({"clientToken": token(0x31), "app": "SLACK", "tenantId": "t-2",
                   "ingestionType": "auditLog"}),
        ),
        (
            "a tag",
            json!({"clientToken": token(0x32), "app": "SLACK", "tenantId": "t-3",
                   "ingestionType": "auditLog", "tags": [{"key": "env", "value": "prod"}]}),
        ),
    ];

    for (index, (case, body)) in bodies.into_iter().enumerate() {
        // The ingestions' parent is one bundle, created in the Region the harness
        // signs for by default so its app-authorization chain is satisfied there.
        let (first_region, second_region) = (fixture_region(0), fixture_region(index + 1));
        let case = format!("{case} across {first_region} and {second_region}");
        let emulator = Emulator::new();
        let bundle = created_arn(
            &emulator
                .create_app_bundle_in_region(&first_region, json!({}))
                .await,
            "appBundle",
            &case,
        );
        let app = body
            .get("app")
            .and_then(Value::as_str)
            .expect("a table row names its app");
        let tenant = body
            .get("tenantId")
            .and_then(Value::as_str)
            .expect("a table row names its tenant");
        // authorize → connect → ingest: the ingestion's dependency.
        emulator.authorize(&bundle, app, tenant).await;

        let uri = format!("/appbundles/{}/ingestions", encode(&bundle));
        let mut arns: BTreeSet<String> = BTreeSet::new();
        for region in [&first_region, &second_region] {
            let created = emulator
                .call_in_region(region, "POST", &uri, Some(body.clone()))
                .await;
            let arn = created_arn(&created, "ingestion", &case);
            assert!(
                arn.contains(&format!(":appfabric:{region}:")),
                "case {case:?}: expected an ingestion ARN in {region}, got {arn:?}"
            );
            assert!(
                arns.insert(arn.clone()),
                "case {case:?}: the token spent in {first_region} must not replay {arn:?} in \
                 {region}"
            );
            // The Region's own ingestion is PERSISTED under the bundle.
            let read = emulator
                .call_in_region(
                    region,
                    "GET",
                    &format!(
                        "/appbundles/{}/ingestions/{}",
                        encode(&bundle),
                        encode(&arn)
                    ),
                    None,
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 reading {arn:?}, got {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.body.get("ingestion"),
                created.body.get("ingestion"),
                "case {case:?}: expected the stored ingestion for {arn:?}, got {}",
                read.body
            );
        }
    }
}
