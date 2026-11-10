// SPDX-License-Identifier: CC0-1.0

//! Which claims of a presented token an OpenID Connect identity source's accepted-client
//! list is compared against.
//!
//! The rule under test: the clients a source lists — `audiences` for an `accessTokenOnly`
//! selection ("the access token `aud` claim values that you want to accept in your policy
//! store") and `clientIds` for an `identityTokenOnly` one ("the ID token audience, or
//! client ID, claim values that you want to accept") — are matched against the token's OWN
//! audience claims: the `aud` claim's value, or each value of the array it may be
//! (RFC 7519 §4.1.3), and the `client_id` claim only for a token that claims no `aud` at
//! all (RFC 9068 §2.2). So a token carrying an `aud` is judged by that `aud` whatever its
//! `client_id` says, a token carrying no `aud` is judged by its `client_id`, and a
//! non-empty configured list accepts exactly a token one of whose audiences it names.
//!
//! The same rule decides both arms of `tokenSelection`, so each row below is run against
//! the source of the arm it names; the Amazon Cognito arm of the same rule is pinned by
//! [`crate::is_authorized_with_token`], and the rest of what an OIDC source decides about
//! a token — its form, issuer, member, window and principal — by
//! [`crate::oidc_identity_source_authorization`], whose table carries one accepted and one
//! unlisted single-`aud` row that the two baseline rows here repeat deliberately, as the
//! contrast every other row in this table is read against.
//!
//! Each row's token is a compact JWS built here: the provider that would sign one lives
//! outside this process, so no key exists in it to check such a signature against and what
//! decides the call is the rules the token's own value settles — the audience among them.

use serde_json::{json, Value};

use crate::harness::{self, error_code, fault_message};
use crate::is_authorized_with_token::policy;
// The OIDC authorization suite owns this crate's OIDC helpers: the source writer, the
// token builder, the request body and the fixed audience refusal.
use crate::oidc_identity_source_authorization::{
    live, oidc_source, request, token, ACTION, AUDIENCE_NOT_ACCEPTED, DOCUMENT, PRINCIPAL_TYPE,
};

/// The issuer of the access-token source: an OIDC provider outside this emulator.
const ACCESS_ISSUER: &str = "https://audience.example.com";

/// The issuer of the identity-token source.
const IDENTITY_ISSUER: &str = "https://audience.example.org/tenant";

/// The two audiences the access-token source accepts, as the model's own example lists
/// several of them (`OpenIdConnectAccessTokenConfiguration$audiences`:
/// "https://myapp.example.com, https://myapp2.example.com").
const ACCEPTED_AUDIENCES: [&str; 2] = ["https://myapp.example.com", "https://myapp2.example.com"];

/// The two clients the identity-token source accepts
/// (`OpenIdConnectIdentityTokenConfiguration$clientIds`: "1example23456789,
/// 2example10111213").
const ACCEPTED_CLIENTS: [&str; 2] = ["1example23456789", "2example10111213"];

/// An audience neither source lists.
const UNLISTED: &str = "https://unlisted.example.net";

/// The principal every token below names, and the one the stores' policies permit.
const SUBJECT: &str = "carlos";

/// Which arm of `tokenSelection` a row drives: the source the token is presented to, and
/// so the request member it arrives in.
#[derive(Debug, Clone, Copy)]
enum Arm {
    /// An `accessTokenOnly` source, which processes the `accessToken` member.
    Access,
    /// An `identityTokenOnly` source, which processes the `identityToken` member.
    Identity,
}

impl Arm {
    /// The request member a token this arm's source processes is presented in.
    fn member(self) -> &'static str {
        match self {
            Self::Access => "accessToken",
            Self::Identity => "identityToken",
        }
    }
}

/// One row: the arm the token is presented to, the claims it carries besides the issuer,
/// the subject and the expiry, and whether that arm's source accepts the audience those
/// claims name.
struct Row {
    case: &'static str,
    arm: Arm,
    /// The audience claims under test, merged into the token's claims.
    claims: Value,
    accepted: bool,
}

/// A `permit` policy for the one principal every token names.
fn permit_subject(entity_id: &str) -> String {
    format!(
        "permit(principal == {PRINCIPAL_TYPE}::\"{entity_id}\", action == {}::\"{}\", \
         resource == MyCorp::Doc::\"{DOCUMENT}\");",
        ACTION.0, ACTION.1
    )
}

/// The configured list is compared against the token's `aud` claims, and against
/// `client_id` only when the token claims no `aud`.
#[tokio::test]
async fn an_oidc_sources_accepted_clients_are_matched_against_the_tokens_audience_claims() {
    let router = harness::router();

    // An access-token-only source accepting two audiences.
    let access_store = harness::store_with_validation_off(&router).await;
    oidc_source(
        &router,
        &access_store,
        &json!({
            "issuer": ACCESS_ISSUER,
            "tokenSelection": {
                "accessTokenOnly": {
                    "principalIdClaim": "sub",
                    "audiences": ACCEPTED_AUDIENCES,
                }
            },
        }),
    )
    .await;
    let access_permit = policy(&router, &access_store, permit_subject(SUBJECT)).await;

    // An identity-token-only source accepting two client ids, the other arm of the same
    // rule.
    let identity_store = harness::store_with_validation_off(&router).await;
    oidc_source(
        &router,
        &identity_store,
        &json!({
            "issuer": IDENTITY_ISSUER,
            "tokenSelection": {
                "identityTokenOnly": {
                    "principalIdClaim": "sub",
                    "clientIds": ACCEPTED_CLIENTS,
                }
            },
        }),
    )
    .await;
    let identity_permit = policy(&router, &identity_store, permit_subject(SUBJECT)).await;

    let row = |case: &'static str, arm: Arm, claims: Value, accepted: bool| Row {
        case,
        arm,
        claims,
        accepted,
    };
    let rows: Vec<Row> = vec![
        row(
            "an access token whose aud is the first accepted audience",
            Arm::Access,
            json!({ "aud": ACCEPTED_AUDIENCES[0] }),
            true,
        ),
        row(
            "an access token whose aud is the second accepted audience",
            Arm::Access,
            json!({ "aud": ACCEPTED_AUDIENCES[1] }),
            true,
        ),
        row(
            "an access token whose only aud is unlisted",
            Arm::Access,
            json!({ "aud": UNLISTED }),
            false,
        ),
        row(
            "an access token whose aud is unlisted while its client_id is accepted",
            Arm::Access,
            json!({ "aud": UNLISTED, "client_id": ACCEPTED_AUDIENCES[0] }),
            false,
        ),
        row(
            "an access token claiming an array of unlisted audiences and an accepted \
             client_id",
            Arm::Access,
            json!({ "aud": [UNLISTED, "https://other.example.net"], "client_id": ACCEPTED_AUDIENCES[1] }),
            false,
        ),
        row(
            "an access token claiming no aud and an accepted client_id",
            Arm::Access,
            json!({ "client_id": ACCEPTED_AUDIENCES[0] }),
            true,
        ),
        row(
            "an access token claiming no aud and an unlisted client_id",
            Arm::Access,
            json!({ "client_id": UNLISTED }),
            false,
        ),
        // An `aud` value is a `StringOrURI` naming a recipient the token is intended for
        // (RFC 7519 §4.1.3 and §2), so a value that is the empty string — and an `aud` array
        // holding no value at all — names no recipient: such a token claims no audience in
        // `aud`, and the client it names in `client_id` is the audience it does claim
        // (RFC 9068 §2.2).
        row(
            "an access token whose aud is empty, naming its client in client_id",
            Arm::Access,
            json!({ "aud": "", "client_id": ACCEPTED_AUDIENCES[1] }),
            true,
        ),
        row(
            "an access token whose aud is an empty array, naming its client in client_id",
            Arm::Access,
            json!({ "aud": [], "client_id": ACCEPTED_AUDIENCES[0] }),
            true,
        ),
        row(
            "an identity token whose aud is the first accepted client",
            Arm::Identity,
            json!({ "aud": ACCEPTED_CLIENTS[0] }),
            true,
        ),
        row(
            "an identity token claiming an array of audiences, the first one accepted",
            Arm::Identity,
            json!({ "aud": [ACCEPTED_CLIENTS[1], UNLISTED] }),
            true,
        ),
        row(
            "an identity token whose aud is unlisted while its client_id is accepted",
            Arm::Identity,
            json!({ "aud": UNLISTED, "client_id": ACCEPTED_CLIENTS[0] }),
            false,
        ),
        row(
            "an identity token claiming no aud and an accepted client_id",
            Arm::Identity,
            json!({ "client_id": ACCEPTED_CLIENTS[1] }),
            true,
        ),
        row(
            "an identity token naming neither aud nor client_id",
            Arm::Identity,
            json!({}),
            false,
        ),
    ];

    for Row {
        case,
        arm,
        claims,
        accepted,
    } in rows
    {
        let member = arm.member();
        let (store, issuer, permit) = match arm {
            Arm::Access => (&access_store, ACCESS_ISSUER, &access_permit),
            Arm::Identity => (&identity_store, IDENTITY_ISSUER, &identity_permit),
        };
        let mut token_claims = json!({ "iss": issuer, "sub": SUBJECT, "exp": live() });
        for (claim, value) in claims.as_object().expect("the row's claims are an object") {
            token_claims[claim.clone()] = value.clone();
        }
        let body = request(store, json!({ member: token(&token_claims) }));
        let (status, answered) = harness::call(&router, "IsAuthorizedWithToken", &body).await;

        if accepted {
            assert_eq!(
                (status, answered["decision"].as_str()),
                (200, Some("ALLOW")),
                "case {case:?}: a token one of whose audiences the source lists decides, got \
                 {status} with {answered}"
            );
            assert_eq!(
                answered["principal"],
                json!({ "entityType": PRINCIPAL_TYPE, "entityId": SUBJECT }),
                "case {case:?}: expected the principal the token's claims name, got {answered}"
            );
            assert_eq!(
                answered["determiningPolicies"][0]["policyId"].as_str(),
                Some(permit.as_str()),
                "case {case:?}: expected the store's permit to be the determining policy, got \
                 {answered}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} with {answered}"
        );
        let message = fault_message(&answered);
        assert_eq!(
            message, AUDIENCE_NOT_ACCEPTED,
            "case {case:?}: expected exactly {AUDIENCE_NOT_ACCEPTED:?}, got {message:?}"
        );
        let named: Vec<String> = answered["fieldList"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field["path"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            named,
            vec![format!("/{member}")],
            "case {case:?}: expected fieldList to name the member the token arrived in, got \
             {answered}"
        );
    }
}
