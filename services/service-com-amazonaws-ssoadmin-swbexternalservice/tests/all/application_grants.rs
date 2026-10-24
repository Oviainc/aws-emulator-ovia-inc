// SPDX-License-Identifier: CC0-1.0

//! The application / OAuth-grant slice end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. a grant of an application is KEYED BY ITS `GrantType`: a `PutApplicationGrant` adds
//!    the grant of that type when the application carries none and REPLACES it when it
//!    does, and `GetApplicationGrant` reports exactly the grant the last put of that type
//!    stored — for every `Grant` union variant and every list size the model admits;
//! 2. `DeleteApplicationGrant` removes exactly the grant its `GrantType` names: after it
//!    the grant no longer resolves (the get and a second delete are both
//!    `ResourceNotFoundException`), every other grant type of the same application is
//!    untouched, and the application itself survives;
//! 3. the put is `@idempotent` and the get is a REPORT: repeating either answers
//!    identically and changes nothing a later caller sees;
//! 4. a grant belongs to ONE application: a put or a delete on one application is
//!    invisible on every other;
//! 5. an `ApplicationArn` this account holds no application for, and a `GrantType` the
//!    application carries no grant for, are refused `ResourceNotFoundException` — never a
//!    200 — on the put, the get and the delete alike, and a refused call changes nothing;
//! 6. the `Grant` union's own rule — "Exactly one member of the union must be specified,
//!    and must match the grant type selected" — is enforced: a request whose `GrantType`
//!    and union variant disagree is refused `ValidationException`, in every mismatched
//!    pair the two enums admit;
//! 7. the member each grant documents as required under its own type
//!    (`AuthorizationCodeGrant$RedirectUris`, `JwtBearerGrant$AuthorizedTokenIssuers`) is
//!    refused `ValidationException` when the grant omits it, and the list bounds
//!    (`length 1..10`) are refused by the boundary;
//! 8. a request outside the modelled constraints — a `GrantType` the enum does not carry,
//!    a `Grant` union body the model does not declare — is refused `ValidationException`
//!    on the put, the get and the delete.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{assert_refused, call, router, seeded_instance_arn};

/// The OAuth 2.0 application provider every application below is created under.
///
/// SOURCE: `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue
/// `ListApplicationProviders` reports, whose `custom` entry is the "SSO Custom
/// Application Provider".
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// Every value the model's `GrantType` enum admits, with the `Grant` union variant each
/// one selects.
///
/// SOURCE: `smithy_query(query="shape",shape="GrantType,Grant")` — the enum carries
/// `authorization_code | refresh_token | urn:ietf:params:oauth:grant-type:jwt-bearer |
/// urn:ietf:params:oauth:grant-type:token-exchange`, and the union carries
/// `AuthorizationCode | RefreshToken | JwtBearer | TokenExchange`.
const GRANT_TYPES: &[(&str, &str)] = &[
    ("authorization_code", "AuthorizationCode"),
    ("refresh_token", "RefreshToken"),
    ("urn:ietf:params:oauth:grant-type:jwt-bearer", "JwtBearer"),
    (
        "urn:ietf:params:oauth:grant-type:token-exchange",
        "TokenExchange",
    ),
];

/// A `TrustedTokenIssuerArn` of the shape the model's pattern requires:
/// `arn:aws:sso::<12 digits>:trustedTokenIssuer/(sso)?ins-<16>/tti-<uuid>`.
const TRUSTED_TOKEN_ISSUER_ARN: &str = "arn:aws:sso::123456789012:trustedTokenIssuer/\
     ssoins-7223b1d1ac5e1f0e/tti-11111111-2222-3333-4444-555555555555";

/// One row of the grant table: a `GrantType` and the `Grant` union body it selects.
#[derive(Debug, Clone)]
struct GrantCase {
    label: &'static str,
    grant_type: &'static str,
    grant: Value,
}

/// The `n`th trusted-token-issuer ARN, so a row can carry a list of DISTINCT issuers up
/// to the modelled bound.
fn issuer_arn(index: usize) -> String {
    format!(
        "arn:aws:sso::123456789012:trustedTokenIssuer/ssoins-7223b1d1ac5e1f0e/\
         tti-{index:08x}-2222-3333-4444-555555555555"
    )
}

/// The grants rules 1-3 are asserted over.
///
/// The rows span the whole class the rule generalises over: every one of the four `Grant`
/// union variants, the two grants that carry no member at all, and — for the two that
/// carry a list — one entry, the modelled upper bound of ten entries, and an entry with
/// its own optional members present and absent. The list bounds come from
/// `smithy_query(query="shape",shape="RedirectUris,AuthorizedTokenIssuers")`:
/// `length 1..10` on both.
fn grant_cases() -> Vec<GrantCase> {
    vec![
        GrantCase {
            label: "authorization_code with a single redirect URI",
            grant_type: "authorization_code",
            grant: json!({"AuthorizationCode": {"RedirectUris": ["https://app.example.com/cb"]}}),
        },
        GrantCase {
            label: "authorization_code with redirect URIs of several forms",
            grant_type: "authorization_code",
            grant: json!({"AuthorizationCode": {"RedirectUris": [
                "https://app.example.com/oauth/callback?tenant=a",
                "http://localhost:8080/cb",
                "com.example.app:/oauth2redirect",
            ]}}),
        },
        GrantCase {
            label: "authorization_code at the modelled ten-URI bound",
            grant_type: "authorization_code",
            grant: json!({"AuthorizationCode": {"RedirectUris": (0..10)
                .map(|index| json!(format!("https://app{index}.example.com/cb")))
                .collect::<Vec<Value>>()}}),
        },
        GrantCase {
            label: "refresh_token, a grant with no configuration member",
            grant_type: "refresh_token",
            grant: json!({"RefreshToken": {}}),
        },
        GrantCase {
            label: "token-exchange, the other grant with no configuration member",
            grant_type: "urn:ietf:params:oauth:grant-type:token-exchange",
            grant: json!({"TokenExchange": {}}),
        },
        GrantCase {
            label: "jwt-bearer with one issuer and one audience",
            grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
            grant: json!({"JwtBearer": {"AuthorizedTokenIssuers": [{
                "TrustedTokenIssuerArn": TRUSTED_TOKEN_ISSUER_ARN,
                "AuthorizedAudiences": ["https://audience.example.com"],
            }]}}),
        },
        GrantCase {
            label: "jwt-bearer with an issuer that carries no audience list",
            grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
            grant: json!({"JwtBearer": {"AuthorizedTokenIssuers": [{
                "TrustedTokenIssuerArn": TRUSTED_TOKEN_ISSUER_ARN,
            }]}}),
        },
        GrantCase {
            label: "jwt-bearer with one issuer at the ten-audience bound",
            grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
            grant: json!({"JwtBearer": {"AuthorizedTokenIssuers": [{
                "TrustedTokenIssuerArn": TRUSTED_TOKEN_ISSUER_ARN,
                "AuthorizedAudiences": (0..10)
                    .map(|index| json!(format!("https://audience{index}.example.com")))
                    .collect::<Vec<Value>>(),
            }]}}),
        },
        GrantCase {
            label: "jwt-bearer at the modelled ten-issuer bound",
            grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
            grant: json!({"JwtBearer": {"AuthorizedTokenIssuers": (0..10)
                .map(|index| json!({
                    "TrustedTokenIssuerArn": issuer_arn(index),
                    "AuthorizedAudiences": [format!("https://audience{index}.example.com")],
                }))
                .collect::<Vec<Value>>()}}),
        },
    ]
}

/// One grant of each `GrantType`, so a test can put the whole set on one application and
/// watch a delete touch exactly one of them.
fn one_grant_per_type() -> Vec<GrantCase> {
    let mut rows: Vec<GrantCase> = Vec::new();
    for (grant_type, _) in GRANT_TYPES {
        let row = grant_cases()
            .into_iter()
            .find(|case| case.grant_type == *grant_type)
            .expect("every GrantType has a row in the grant table");
        rows.push(row);
    }
    rows
}

/// Create one application and return its `ApplicationArn`.
async fn create_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateApplication",
        &json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
            "Name": name,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateApplication must answer 200, got {status} {body}"
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateApplication returned no ARN: {body}"))
        .to_string()
}

async fn put_grant(
    router: &MultiServiceRouter,
    application_arn: &str,
    grant_type: &str,
    grant: &Value,
) -> (u16, Value) {
    call(
        router,
        "PutApplicationGrant",
        &json!({
            "ApplicationArn": application_arn,
            "GrantType": grant_type,
            "Grant": grant,
        }),
    )
    .await
}

async fn get_grant(
    router: &MultiServiceRouter,
    application_arn: &str,
    grant_type: &str,
) -> (u16, Value) {
    call(
        router,
        "GetApplicationGrant",
        &json!({"ApplicationArn": application_arn, "GrantType": grant_type}),
    )
    .await
}

async fn delete_grant(
    router: &MultiServiceRouter,
    application_arn: &str,
    grant_type: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteApplicationGrant",
        &json!({"ApplicationArn": application_arn, "GrantType": grant_type}),
    )
    .await
}

/// Rules 1 and 3: every row is stored and read back verbatim, a repeated put stores the
/// same grant, and a repeated get reports it unchanged.
#[tokio::test]
async fn every_put_grant_reads_back_exactly_what_it_stored() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for case in grant_cases() {
        let label = case.label;
        // One application per row, so each row observes a FIRST put as well as a repeat.
        let application_arn = create_application(&router, &instance_arn, "grant-roundtrip").await;

        // Rule 5: before the put there is no grant of this type to read.
        let (status, body) = get_grant(&router, &application_arn, case.grant_type).await;
        assert_refused(
            label,
            "a get before any put",
            "ResourceNotFoundException",
            status,
            &body,
        );

        let (status, body) = put_grant(&router, &application_arn, case.grant_type, &case.grant).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the put must answer 200, got {status} {body}"
        );
        assert!(
            body["Grant"].is_null(),
            "case {label:?}: the put's output is modelled as Unit, got {body}"
        );

        let (status, read) = get_grant(&router, &application_arn, case.grant_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the get after the put must answer 200, got {status} {read}"
        );
        assert_eq!(
            read,
            json!({"Grant": case.grant}),
            "case {label:?}: the get must report exactly the grant the put stored, got {read}"
        );

        // Rule 3: the put is idempotent and the get is a report, so repeating both
        // answers identically.
        let (status, body) = put_grant(&router, &application_arn, case.grant_type, &case.grant).await;
        assert_eq!(
            status, 200,
            "case {label:?}: repeating the put must answer 200, got {status} {body}"
        );
        let (status, again) = get_grant(&router, &application_arn, case.grant_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the get after the repeated put must answer 200, got {status} {again}"
        );
        assert_eq!(
            again, read,
            "case {label:?}: repeating the put must leave the same grant stored, got {again}"
        );
        let (status, third) = get_grant(&router, &application_arn, case.grant_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a third get must answer 200, got {status} {third}"
        );
        assert_eq!(
            third, read,
            "case {label:?}: a get must change nothing a later caller sees, got {third}"
        );
    }
}

/// Rule 1: a put of a type the application already carries REPLACES the stored grant,
/// in every direction the configuration can change.
#[tokio::test]
async fn a_second_put_replaces_the_grant_of_the_same_type() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "replaced-grant").await;

    // Only the rows of ONE grant type can replace each other: the transitions span
    // growing the list, shrinking it, and swapping its entries for others.
    let rows: Vec<GrantCase> = grant_cases()
        .into_iter()
        .filter(|case| case.grant_type == "authorization_code")
        .collect();
    let mut previous: Option<Value> = None;
    for case in rows.iter().chain(rows.iter().rev()) {
        let label = case.label;
        let (status, body) =
            put_grant(&router, &application_arn, case.grant_type, &case.grant).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the replacing put must answer 200, got {status} {body}"
        );
        let (status, read) = get_grant(&router, &application_arn, case.grant_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the get after the replacing put must answer 200, got {status} {read}"
        );
        assert_eq!(
            read,
            json!({"Grant": case.grant}),
            "case {label:?}: the second put must REPLACE the stored grant, got {read}"
        );
        if let Some(previous) = &previous {
            if previous != &case.grant {
                assert_ne!(
                    read,
                    json!({"Grant": previous}),
                    "case {label:?}: the replaced grant must no longer be readable, got {read}"
                );
            }
        }
        previous = Some(case.grant.clone());
    }
}

/// Rule 2: the delete removes exactly the grant its `GrantType` names — the get and a
/// second delete then both refuse — while every other grant type of the same application
/// is untouched and the application itself survives.
#[tokio::test]
async fn delete_removes_only_the_grant_its_type_names() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // Deleting in several ORDERS, so the rule is not asserted on one walk of the set:
    // front to back, back to front, and from the middle out.
    let all = one_grant_per_type();
    let orders: Vec<(&str, Vec<usize>)> = vec![
        ("front to back", vec![0, 1, 2, 3]),
        ("back to front", vec![3, 2, 1, 0]),
        ("from the middle out", vec![1, 2, 0, 3]),
    ];

    for (order_label, order) in orders {
        let application_arn =
            create_application(&router, &instance_arn, "grant-delete-order").await;
        for case in &all {
            let (status, body) =
                put_grant(&router, &application_arn, case.grant_type, &case.grant).await;
            assert_eq!(
                status, 200,
                "case {order_label:?}/{:?}: the seed put must answer 200, got {status} {body}",
                case.grant_type
            );
        }

        let mut remaining: Vec<&GrantCase> = all.iter().collect();
        for index in order {
            let doomed = &all[index];
            let case = format!("{order_label} deleting {}", doomed.grant_type);

            let (status, body) = delete_grant(&router, &application_arn, doomed.grant_type).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the delete must answer 200, got {status} {body}"
            );
            assert!(
                body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
                "case {case:?}: the delete's output is modelled as Unit, got {body}"
            );

            // The deleted grant no longer resolves, for the get and for a second delete
            // alike.
            let (status, body) = get_grant(&router, &application_arn, doomed.grant_type).await;
            assert_refused(
                &case,
                "a get of the deleted grant",
                "ResourceNotFoundException",
                status,
                &body,
            );
            let (status, body) = delete_grant(&router, &application_arn, doomed.grant_type).await;
            assert_refused(
                &case,
                "a second delete of the same grant",
                "ResourceNotFoundException",
                status,
                &body,
            );

            remaining.retain(|row| row.grant_type != doomed.grant_type);
            // Every grant type the delete did not name still reports its OWN grant.
            for row in &remaining {
                let (status, read) = get_grant(&router, &application_arn, row.grant_type).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: {} must survive the delete, got {status} {read}",
                    row.grant_type
                );
                assert_eq!(
                    read,
                    json!({"Grant": row.grant}),
                    "case {case:?}: {} must still report its own grant, got {read}",
                    row.grant_type
                );
            }
        }

        // The application outlives its grants: deleting every grant deletes no
        // application.
        let (status, body) = call(
            &router,
            "DescribeApplication",
            &json!({"ApplicationArn": application_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {order_label:?}: the application must survive its grants, got {status} {body}"
        );

        // And a grant put again after the delete is stored again.
        let revived = &all[0];
        let (status, body) =
            put_grant(&router, &application_arn, revived.grant_type, &revived.grant).await;
        assert_eq!(
            status, 200,
            "case {order_label:?}: a put after the delete must answer 200, got {status} {body}"
        );
        let (status, read) = get_grant(&router, &application_arn, revived.grant_type).await;
        assert_eq!(
            status, 200,
            "case {order_label:?}: the re-put grant must read back, got {status} {read}"
        );
        assert_eq!(
            read,
            json!({"Grant": revived.grant}),
            "case {order_label:?}: the re-put grant must read back verbatim, got {read}"
        );
    }
}

/// Rule 4: a grant belongs to one application, so a put and a delete on one are invisible
/// on every other — including on applications created before and after them.
#[tokio::test]
async fn a_grant_belongs_to_one_application() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    let names = ["app-one", "app-two", "app-three", "app-four"];
    let mut arns: Vec<String> = Vec::new();
    for name in names {
        arns.push(create_application(&router, &instance_arn, name).await);
    }

    // Each application gets its OWN redirect URI, so a leak between two of them shows up
    // as the wrong grant and not only as a 200.
    let grant_of = |arn: &str| json!({"AuthorizationCode": {"RedirectUris": [format!("https://{arn}/cb")]}});
    for (index, arn) in arns.iter().enumerate() {
        let (status, body) = put_grant(&router, arn, "authorization_code", &grant_of(arn)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put must answer 200, got {status} {body}",
            names[index]
        );
    }

    // Deleting the grant of ONE application leaves every other application's grant.
    let (status, body) = delete_grant(&router, &arns[1], "authorization_code").await;
    assert_eq!(
        status, 200,
        "case \"app-two\": the delete must answer 200, got {status} {body}"
    );
    for (index, arn) in arns.iter().enumerate() {
        let (status, read) = get_grant(&router, arn, "authorization_code").await;
        if index == 1 {
            assert_refused(
                names[index],
                "a get of the deleted grant",
                "ResourceNotFoundException",
                status,
                &read,
            );
        } else {
            assert_eq!(
                status, 200,
                "case {:?}: the get must answer 200, got {status} {read}",
                names[index]
            );
            assert_eq!(
                read,
                json!({"Grant": grant_of(arn)}),
                "case {:?}: each application must report its OWN grant, got {read}",
                names[index]
            );
        }
    }

    // An application created after all those calls carries no grant at all, and deleting
    // from it is refused.
    let latecomer = create_application(&router, &instance_arn, "app-five").await;
    for (grant_type, _) in GRANT_TYPES {
        let (status, body) = get_grant(&router, &latecomer, grant_type).await;
        assert_refused(
            grant_type,
            "a get on an application nobody put a grant on",
            "ResourceNotFoundException",
            status,
            &body,
        );
        let (status, body) = delete_grant(&router, &latecomer, grant_type).await;
        assert_refused(
            grant_type,
            "a delete on an application nobody put a grant on",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }
}

/// Rule 5: an `ApplicationArn` no create minted is refused by the put, the get and the
/// delete, and the refused calls change nothing anywhere.
#[tokio::test]
async fn an_application_arn_no_create_minted_is_refused_not_found() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let instance_id = instance_arn.rsplit('/').next().unwrap_or_default().to_string();
    // One application with one stored grant exists, so the not-found is about the ARN
    // asked for and not about an empty store.
    let stored = create_application(&router, &instance_arn, "stored-app").await;
    let grant = json!({"AuthorizationCode": {"RedirectUris": ["https://stored.example.com/cb"]}});
    let (status, body) = put_grant(&router, &stored, "authorization_code", &grant).await;
    assert_eq!(status, 200, "the seed put must answer 200, got {status} {body}");

    let account = stored.split(':').nth(4).unwrap_or_default().to_string();
    let suffix = stored.rsplit("/apl-").next().unwrap_or_default().to_string();
    let application_arn = |partition: &str, account: &str, instance: &str, suffix: &str| {
        format!("arn:{partition}:sso::{account}:application/{instance}/apl-{suffix}")
    };

    // The rows span the parts of the ARN that identify the application: the `apl-`
    // suffix, the instance segment, the account segment and the partition.
    let cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid apl- suffix that was never minted",
            application_arn("aws", &account, &instance_id, "0000000000000000"),
        ),
        (
            "the stored suffix under another instance",
            application_arn("aws", &account, "ssoins-0000000000000000", &suffix),
        ),
        (
            "the stored application under another account",
            application_arn("aws", "210987654321", &instance_id, &suffix),
        ),
        (
            "the stored application in another partition",
            application_arn("aws-us-gov", &account, &instance_id, &suffix),
        ),
    ];

    for (case, arn) in cases {
        assert_ne!(
            arn, stored,
            "case {case:?}: the row must name a DIFFERENT ARN than the stored one"
        );
        let (status, body) = put_grant(&router, &arn, "authorization_code", &grant).await;
        assert_refused(
            case,
            "a put under an unminted ARN",
            "ResourceNotFoundException",
            status,
            &body,
        );
        let (status, body) = get_grant(&router, &arn, "authorization_code").await;
        assert_refused(
            case,
            "a get under an unminted ARN",
            "ResourceNotFoundException",
            status,
            &body,
        );
        let (status, body) = delete_grant(&router, &arn, "authorization_code").await;
        assert_refused(
            case,
            "a delete under an unminted ARN",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }

    // None of those refusals touched the stored application's grant.
    let (status, read) = get_grant(&router, &stored, "authorization_code").await;
    assert_eq!(
        status, 200,
        "the stored grant must survive the refusals, got {status} {read}"
    );
    assert_eq!(
        read,
        json!({"Grant": grant}),
        "a refused call must not change the stored grant, got {read}"
    );
}

/// Rule 6: the `Grant` union variant must match the `GrantType` the request declares, in
/// every mismatched pair the two enums admit, and a refused put stores nothing.
#[tokio::test]
async fn a_grant_that_does_not_match_its_grant_type_is_refused() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "mismatched-grant").await;

    // A well-formed body for each union variant, so the ONLY thing wrong in a mismatched
    // row is the pairing.
    let body_of = |variant: &str| match variant {
        "AuthorizationCode" => {
            json!({"AuthorizationCode": {"RedirectUris": ["https://app.example.com/cb"]}})
        }
        "JwtBearer" => json!({"JwtBearer": {"AuthorizedTokenIssuers": [{
            "TrustedTokenIssuerArn": TRUSTED_TOKEN_ISSUER_ARN,
        }]}}),
        "RefreshToken" => json!({"RefreshToken": {}}),
        other => json!({other: {}}),
    };

    for (grant_type, _) in GRANT_TYPES {
        for (other_type, other_variant) in GRANT_TYPES {
            if other_type == grant_type {
                continue;
            }
            let case = format!("GrantType {grant_type} carrying a {other_variant} grant");
            let (status, body) =
                put_grant(&router, &application_arn, grant_type, &body_of(other_variant)).await;
            assert_refused(
                &case,
                "a put whose union variant does not match its GrantType",
                "ValidationException",
                status,
                &body,
            );
        }
    }

    // None of the refusals stored a grant under either half of the pair.
    for (grant_type, _) in GRANT_TYPES {
        let (status, body) = get_grant(&router, &application_arn, grant_type).await;
        assert_refused(
            grant_type,
            "a get after only refused puts",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }
}

/// Rule 7: the member each grant documents as required under its own type is refused when
/// the grant omits it, and the modelled list bounds are refused by the boundary.
#[tokio::test]
async fn a_grant_missing_its_required_member_is_refused() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "incomplete-grant").await;

    // Rows 1-2: the documented requirement the model marks OPTIONAL
    // (`AuthorizationCodeGrant$RedirectUris` "is required when the grant type is
    // authorization_code"; `JwtBearerGrant$AuthorizedTokenIssuers` "is required when the
    // grant type is JwtBearerGrant"). Rows 3-4: the `length 1..10` bound of each list,
    // below it (an empty list) and above it (eleven entries).
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "authorization_code with no RedirectUris",
            "authorization_code",
            json!({"AuthorizationCode": {}}),
        ),
        (
            "jwt-bearer with no AuthorizedTokenIssuers",
            "urn:ietf:params:oauth:grant-type:jwt-bearer",
            json!({"JwtBearer": {}}),
        ),
        (
            "authorization_code with an empty RedirectUris list",
            "authorization_code",
            json!({"AuthorizationCode": {"RedirectUris": []}}),
        ),
        (
            "jwt-bearer with an empty AuthorizedTokenIssuers list",
            "urn:ietf:params:oauth:grant-type:jwt-bearer",
            json!({"JwtBearer": {"AuthorizedTokenIssuers": []}}),
        ),
        (
            "authorization_code one URI past the ten-URI bound",
            "authorization_code",
            json!({"AuthorizationCode": {"RedirectUris": (0..11)
                .map(|index| json!(format!("https://app{index}.example.com/cb")))
                .collect::<Vec<Value>>()}}),
        ),
        (
            "jwt-bearer one issuer past the ten-issuer bound",
            "urn:ietf:params:oauth:grant-type:jwt-bearer",
            json!({"JwtBearer": {"AuthorizedTokenIssuers": (0..11)
                .map(|index| json!({"TrustedTokenIssuerArn": issuer_arn(index)}))
                .collect::<Vec<Value>>()}}),
        ),
    ];

    for (case, grant_type, grant) in cases {
        let (status, body) = put_grant(&router, &application_arn, grant_type, &grant).await;
        assert_refused(
            case,
            "a put of an incomplete grant",
            "ValidationException",
            status,
            &body,
        );
    }

    // Nothing was stored, so nothing can be deleted either.
    for (grant_type, _) in GRANT_TYPES {
        let (status, body) = get_grant(&router, &application_arn, grant_type).await;
        assert_refused(
            grant_type,
            "a get after only refused puts",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }

    // The grants the model requires nothing of are accepted as they stand.
    for (grant_type, variant) in [
        ("refresh_token", "RefreshToken"),
        (
            "urn:ietf:params:oauth:grant-type:token-exchange",
            "TokenExchange",
        ),
    ] {
        let grant = json!({variant: {}});
        let (status, body) = put_grant(&router, &application_arn, grant_type, &grant).await;
        assert_eq!(
            status, 200,
            "case {grant_type:?}: a grant with no required member must be accepted, got \
             {status} {body}"
        );
        let (status, read) = get_grant(&router, &application_arn, grant_type).await;
        assert_eq!(
            status, 200,
            "case {grant_type:?}: the get must answer 200, got {status} {read}"
        );
        assert_eq!(
            read,
            json!({"Grant": grant}),
            "case {grant_type:?}: the get must report the stored grant, got {read}"
        );
    }
}

/// Rule 8: a request outside the modelled constraints is refused `ValidationException` on
/// the put, the get and the delete, and stores nothing.
#[tokio::test]
async fn requests_outside_the_modeled_constraints_are_refused() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "constrained-grant").await;
    let grant = json!({"AuthorizationCode": {"RedirectUris": ["https://app.example.com/cb"]}});

    // `GrantType` values the model's enum does not carry: the rows span the case, a
    // near-miss, the bare URN tail, a value of another enum of this service and the empty
    // string.
    let unknown_types = [
        ("a value in another case", "AUTHORIZATION_CODE"),
        ("a near-miss with a suffix", "authorization_code_v2"),
        ("the tail of a URN value", "jwt-bearer"),
        ("a value of another enum", "IAM"),
        ("the empty string", ""),
    ];
    for (case, grant_type) in unknown_types {
        let (status, body) = put_grant(&router, &application_arn, grant_type, &grant).await;
        assert_refused(
            case,
            "a put with an unmodelled GrantType",
            "ValidationException",
            status,
            &body,
        );
        let (status, body) = get_grant(&router, &application_arn, grant_type).await;
        assert_refused(
            case,
            "a get with an unmodelled GrantType",
            "ValidationException",
            status,
            &body,
        );
        let (status, body) = delete_grant(&router, &application_arn, grant_type).await;
        assert_refused(
            case,
            "a delete with an unmodelled GrantType",
            "ValidationException",
            status,
            &body,
        );
    }

    // `Grant` bodies the model's union does not admit: a variant it does not declare, and
    // no variant at all.
    let unknown_grants: Vec<(&str, Value)> = vec![
        (
            "a union variant the model does not declare",
            json!({"ClientCredentials": {}}),
        ),
        ("a union with no variant set", json!({})),
    ];
    for (case, body) in unknown_grants {
        let (status, answer) = put_grant(&router, &application_arn, "refresh_token", &body).await;
        assert_refused(
            case,
            "a put with an unmodelled Grant",
            "ValidationException",
            status,
            &answer,
        );
    }

    // None of the refusals stored anything.
    for (grant_type, _) in GRANT_TYPES {
        let (status, body) = get_grant(&router, &application_arn, grant_type).await;
        assert_refused(
            grant_type,
            "a get after only refused requests",
            "ResourceNotFoundException",
            status,
            &body,
        );
    }
}
