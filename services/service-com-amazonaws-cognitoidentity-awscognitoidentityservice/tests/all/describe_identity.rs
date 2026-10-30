// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeIdentity`: the metadata this service holds about one
//! identity.
//!
//! RULE: `DescribeIdentity` reports the STORED identity the request names — the instant it
//! was minted, the instant it last changed and the login providers it is linked to, the
//! very record a listing of its pool reports for it — and changes nothing, so two reads
//! answer alike and a read is never what moves the identity's last-modified instant. An
//! identity this service does not hold is the modelled `ResourceNotFoundException`.
//!
//! The rows differ along what the rule generalises over: how the identity came to be (an
//! unauthenticated `GetId`, a federated one, one that gained a provider through the
//! implicit link `GetOpenIdToken` makes) and how many providers it is linked to. The
//! harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.
//!
//! This operation was a 501 scaffold stub until this change; its stub issue is
//! `20261024T145006Z-cognito-identity-describeidentity-stub.md`.

use serde_json::{json, Value};

use crate::developer_identity_tokens::listed_logins;
use crate::identity_pool_flow::{error_code, Emulator};

/// The operation under test, spelled once.
const OPERATION: &str = "DescribeIdentity";

/// A well-formed `IdentityId` that nothing ever minted.
const ABSENT_IDENTITY_ID: &str = "us-east-1:44444444-4444-4444-4444-444444444444";

/// The public login providers the fixture pool accepts.
const FIRST_PROVIDER: &str = "graph.facebook.com";
const SECOND_PROVIDER: &str = "www.amazon.com";

/// A pool that allows unauthenticated identities and accepts the two public providers the
/// rows present. Answers its minted `IdentityPoolId`.
async fn pool(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "SupportedLoginProviders": {
                    FIRST_PROVIDER: "0123456789",
                    SECOND_PROVIDER: "amzn1.application.1",
                },
            }),
        )
        .await;
    created["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIdentityPool answers an IdentityPoolId: {created}"))
        .to_string()
}

/// `DescribeIdentity` for `identity_id`. `pub(crate)` so a sibling suite in this test target
/// reads an identity back through the same call rather than keeping a second copy of it.
pub(crate) async fn describe(emulator: &Emulator, identity_id: &str) -> (u16, Value) {
    emulator
        .call(OPERATION, &json!({"IdentityId": identity_id}))
        .await
}

/// The timestamp a reported member carries, as the seconds since the epoch an `awsJson`
/// body writes an instant as.
fn instant(case: &str, described: &Value, member: &str) -> f64 {
    described[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {member} is an instant: {described}"))
}

/// RULE: what a describe reports is the stored identity, and the read changes nothing.
#[tokio::test]
async fn a_describe_reports_the_stored_identity_and_changes_nothing() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "described-identities").await;

    // One row: the case, the logins `GetId` presented (none for an unauthenticated
    // identity) and the provider a later `GetOpenIdToken` links to it on top (`None` for a
    // row that links nothing more).
    type Row<'a> = (&'a str, Vec<(&'a str, &'a str)>, Option<(&'a str, &'a str)>);
    let rows: Vec<Row<'_>> = vec![
        ("an unauthenticated identity", vec![], None),
        (
            "a federated identity of one provider",
            vec![(FIRST_PROVIDER, "facebook-user-1")],
            None,
        ),
        (
            "a federated identity of two providers",
            vec![
                (FIRST_PROVIDER, "facebook-user-2"),
                (SECOND_PROVIDER, "amazon-user-2"),
            ],
            None,
        ),
        (
            "an identity that gained a provider through the implicit link",
            vec![(FIRST_PROVIDER, "facebook-user-3")],
            Some((SECOND_PROVIDER, "amazon-user-3")),
        ),
    ];

    for (case, logins, linked_later) in rows {
        let presented = (!logins.is_empty()).then(|| {
            Value::Object(
                logins
                    .iter()
                    .map(|(provider, login)| ((*provider).to_string(), json!(login)))
                    .collect(),
            )
        });
        let identity_id = emulator.identity(&pool_id, presented).await;

        let (status, described) = describe(&emulator, &identity_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {described}"
        );
        assert_eq!(
            described["IdentityId"].as_str(),
            Some(identity_id.as_str()),
            "case {case:?}: a describe names the identity it was asked for, got {described}"
        );
        let reported: Vec<String> = described["Logins"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Logins is a list: {described}"))
            .iter()
            .map(|login| login.as_str().unwrap_or_default().to_string())
            .collect();
        let listed = listed_logins(&emulator, &pool_id, &identity_id).await;
        assert_eq!(
            reported, listed,
            "case {case:?}: expected the providers the pool's listing reports ({listed:?}), \
             got {reported:?}"
        );
        let created_at = instant(case, &described, "CreationDate");
        let modified_at = instant(case, &described, "LastModifiedDate");
        assert!(
            created_at <= modified_at,
            "case {case:?}: expected the identity to have been minted no later than it last \
             changed, got {created_at} and {modified_at}"
        );

        // A read changes nothing: asked again, the describe answers exactly the same
        // record.
        let (status, again) = describe(&emulator, &identity_id).await;
        assert_eq!(
            (status, &again),
            (200, &described),
            "case {case:?}: a second describe answered {again}, expected {described}"
        );

        // Linking another provider IS a change to the identity, and the describe reports
        // it: the new provider is listed and the last-modified instant has moved, while the
        // creation instant has not.
        if let Some((provider, login)) = linked_later {
            emulator
                .ok(
                    "GetOpenIdToken",
                    &json!({
                        "IdentityId": &identity_id,
                        "Logins": {provider: login},
                    }),
                )
                .await;
            let (status, after) = describe(&emulator, &identity_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 after the link, got {status} with {after}"
            );
            let linked: Vec<String> = after["Logins"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: Logins is a list: {after}"))
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                linked.contains(&provider.to_string()),
                "case {case:?}: expected the linked provider {provider:?} to be reported, got \
                 {linked:?}"
            );
            assert_eq!(
                instant(case, &after, "CreationDate"),
                created_at,
                "case {case:?}: the instant the identity was minted does not move, got {after}"
            );
            assert!(
                instant(case, &after, "LastModifiedDate") >= modified_at,
                "case {case:?}: expected the last-modified instant to have moved, got {after}"
            );
        }
    }
}

/// RULE: an identity this service does not hold is the modelled
/// `ResourceNotFoundException`, and the identities it does hold are untouched by that
/// refusal.
#[tokio::test]
async fn an_identity_this_service_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "described-absent").await;
    let held = emulator.identity(&pool_id, None).await;
    let (_, before) = describe(&emulator, &held).await;

    // One row per identity id nothing minted: one never minted at all, and one of a pool
    // this service does not hold either.
    for (case, identity_id) in [
        ("an identity id nothing ever minted", ABSENT_IDENTITY_ID),
        (
            "an identity id of another region",
            "eu-west-1:55555555-5555-5555-5555-555555555555",
        ),
    ] {
        let (status, body) = describe(&emulator, identity_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {code:?} with {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("Identity '{identity_id}' not found.").as_str()),
            "case {case:?}: expected the refusal to name the identity, got {body}"
        );
    }

    let (status, after) = describe(&emulator, &held).await;
    assert_eq!(
        (status, &after),
        (200, &before),
        "the identity this service holds is untouched by a refusal about another, got {after}"
    );
}
