// SPDX-License-Identifier: CC0-1.0

//! What an `@idempotencyToken` create does with a token it has already seen:
//! the SAME parameters replay the first call's resource, and ANY different
//! parameters are the modelled 409 `ConflictException` — whatever characters
//! those parameters carry.
//!
//! The rendering a create compares the two calls by is a canonical one in which
//! every caller-supplied value is quoted and escaped, so a value carrying the
//! `=` or `;` characters that rendering uses as its own separators cannot make
//! two different requests look like one call. The rows below are exactly such
//! requests: pairs that differ only in WHERE a separator-carrying value's
//! boundaries fall, which a rendering that interpolated its values plainly would
//! collapse into one — answering the second request as a replay, so the resource
//! it asked for was never created.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! resource is only visible if the create really reached state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, error_code, ok, router, store_with_validation_off as store, user_pool_arn,
    SIGNING_REGION,
};

/// A Cognito user pool this emulator can resolve: one of its own account, in the
/// region the harness signs for, whose pool id names that same region.
fn pool_arn() -> String {
    user_pool_arn(SIGNING_REGION, &format!("{SIGNING_REGION}_1a2b3c4d5"))
}

/// A Cedar static policy every policy row below carries, so a row differs from
/// its partner only in the member it is about.
const STATEMENT: &str = "permit(principal, action, resource);";

/// A Cedar policy TEMPLATE statement, which has to carry a placeholder.
const TEMPLATE_STATEMENT: &str = "permit(principal == ?principal, action, resource);";

/// The `identitySourceId`s a store holds, in the order they are listed.
async fn listed_source_ids(router: &MultiServiceRouter, store_id: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListIdentitySources",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    listed["identitySources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["identitySourceId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Two `CreateIdentitySource` requests that are genuinely different, and what a
/// read of the SECOND one's own identity source must report once it is created
/// under a token of its own.
struct SourcePair {
    case: &'static str,
    /// Request members beside `policyStoreId` and `clientToken`.
    first: Value,
    second: Value,
    /// `(JSON pointer into the GetIdentitySource response, expected value)` for
    /// the second request's identity source.
    second_reads: Vec<(&'static str, Value)>,
}

// ---------------------------------------------------------------------------
// CreateIdentitySource
// ---------------------------------------------------------------------------

/// Two different `CreateIdentitySource` requests under one `clientToken`
/// conflict, the first one's identity source is untouched, and each request
/// creates its own identity source under a token of its own.
///
/// The rows differ along WHICH member carries the separators and across both
/// arms of the `Configuration` union and both arms of
/// `OpenIdConnectTokenSelection`. The members used are the ones whose modelled
/// types admit those characters — `PrincipalEntityType` (`^.*$`), `Issuer`
/// (`^https://.*$`), `Claim`, `EntityIdPrefix` (no pattern) and `ClientId`
/// (`^.*$`), from smithy_query(query="shape",
/// shape="PrincipalEntityType,Issuer,Claim,EntityIdPrefix,ClientId") — so every
/// row is legal input.
#[tokio::test]
async fn two_different_identity_source_requests_under_one_client_token_conflict() {
    let arn = pool_arn();
    let pairs: Vec<SourcePair> = vec![
        SourcePair {
            // The separators move the boundary between `principalEntityType`
            // and the configuration that follows it.
            case: "oidc-issuer-and-principal-entity-type-swap-the-boundary",
            first: json!({
                "principalEntityType": "MyCorp::User",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://first.example.com;configuration=oidc:issuer=https://second.example.com",
                        "tokenSelection": { "identityTokenOnly": { "principalIdClaim": "sub" } },
                    }
                },
            }),
            second: json!({
                "principalEntityType": "MyCorp::User;configuration=oidc:issuer=https://first.example.com",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://second.example.com",
                        "tokenSelection": { "identityTokenOnly": { "principalIdClaim": "sub" } },
                    }
                },
            }),
            second_reads: vec![
                (
                    "/principalEntityType",
                    json!("MyCorp::User;configuration=oidc:issuer=https://first.example.com"),
                ),
                (
                    "/configuration/openIdConnectConfiguration/issuer",
                    json!("https://second.example.com"),
                ),
            ],
        },
        SourcePair {
            // The separators move the boundary between the issuer and the
            // claim the token selection names.
            case: "oidc-issuer-and-principal-id-claim-swap-the-boundary",
            first: json!({
                "principalEntityType": "MyCorp::Person",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://idp.example.org;principalIdClaim=sub",
                        "tokenSelection": { "identityTokenOnly": { "principalIdClaim": "email" } },
                    }
                },
            }),
            second: json!({
                "principalEntityType": "MyCorp::Person",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://idp.example.org",
                        "tokenSelection": {
                            "identityTokenOnly": { "principalIdClaim": "sub;principalIdClaim=email" }
                        },
                    }
                },
            }),
            second_reads: vec![
                (
                    "/configuration/openIdConnectConfiguration/issuer",
                    json!("https://idp.example.org"),
                ),
                (
                    "/configuration/openIdConnectConfiguration/tokenSelection/identityTokenOnly/principalIdClaim",
                    json!("sub;principalIdClaim=email"),
                ),
            ],
        },
        SourcePair {
            // The separators move the boundary between the issuer and the
            // optional entity-id prefix: one request sets the prefix, the other
            // spells it inside its issuer and sets none.
            case: "oidc-entity-id-prefix-present-or-spelled-in-the-issuer",
            first: json!({
                "principalEntityType": "MyCorp::Person",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://prefix.example.org",
                        "entityIdPrefix": "MyOIDCProvider",
                        "tokenSelection": { "identityTokenOnly": { "principalIdClaim": "sub" } },
                    }
                },
            }),
            second: json!({
                "principalEntityType": "MyCorp::Person",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://prefix.example.org;entityIdPrefix=MyOIDCProvider",
                        "tokenSelection": { "identityTokenOnly": { "principalIdClaim": "sub" } },
                    }
                },
            }),
            second_reads: vec![
                (
                    "/configuration/openIdConnectConfiguration/issuer",
                    json!("https://prefix.example.org;entityIdPrefix=MyOIDCProvider"),
                ),
                (
                    "/configuration/openIdConnectConfiguration/tokenSelection/identityTokenOnly/principalIdClaim",
                    json!("sub"),
                ),
            ],
        },
        SourcePair {
            // A LIST member: the same characters, split across one element or
            // two, is a different set of accepted client ids.
            case: "oidc-client-ids-split-across-one-element-or-two",
            first: json!({
                "principalEntityType": "MyCorp::Person",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://clients.example.org",
                        "tokenSelection": {
                            "identityTokenOnly": {
                                "principalIdClaim": "sub",
                                "clientIds": ["first;second"],
                            }
                        },
                    }
                },
            }),
            second: json!({
                "principalEntityType": "MyCorp::Person",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://clients.example.org",
                        "tokenSelection": {
                            "identityTokenOnly": {
                                "principalIdClaim": "sub",
                                "clientIds": ["first", "second"],
                            }
                        },
                    }
                },
            }),
            second_reads: vec![(
                "/configuration/openIdConnectConfiguration/tokenSelection/identityTokenOnly/clientIds",
                json!(["first", "second"]),
            )],
        },
        SourcePair {
            // The other arm of the token selection, with its own list member.
            case: "oidc-audiences-split-across-one-element-or-two",
            first: json!({
                "principalEntityType": "MyCorp::Person",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://audiences.example.org",
                        "tokenSelection": {
                            "accessTokenOnly": {
                                "principalIdClaim": "sub",
                                "audiences": ["https://a.example.com;https://b.example.com"],
                            }
                        },
                    }
                },
            }),
            second: json!({
                "principalEntityType": "MyCorp::Person",
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://audiences.example.org",
                        "tokenSelection": {
                            "accessTokenOnly": {
                                "principalIdClaim": "sub",
                                "audiences": ["https://a.example.com", "https://b.example.com"],
                            }
                        },
                    }
                },
            }),
            second_reads: vec![(
                "/configuration/openIdConnectConfiguration/tokenSelection/accessTokenOnly/audiences",
                json!(["https://a.example.com", "https://b.example.com"]),
            )],
        },
        SourcePair {
            // The other arm of the `Configuration` union, whose client ids are a
            // list too.
            case: "cognito-client-ids-split-across-one-element-or-two",
            first: json!({
                "principalEntityType": "MyCorp::User",
                "configuration": {
                    "cognitoUserPoolConfiguration": {
                        "userPoolArn": arn,
                        "clientIds": ["first;second"],
                    }
                },
            }),
            second: json!({
                "principalEntityType": "MyCorp::User",
                "configuration": {
                    "cognitoUserPoolConfiguration": {
                        "userPoolArn": arn,
                        "clientIds": ["first", "second"],
                    }
                },
            }),
            second_reads: vec![(
                "/configuration/cognitoUserPoolConfiguration/clientIds",
                json!(["first", "second"]),
            )],
        },
        SourcePair {
            // A Cognito source whose `principalEntityType` spells out the
            // configuration that follows it.
            case: "cognito-principal-entity-type-spells-the-configuration",
            first: json!({
                "principalEntityType": "MyCorp::User",
                "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } },
            }),
            second: json!({
                "principalEntityType": format!("MyCorp::User;configuration=cognito:userPoolArn={arn}"),
                "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": arn } },
            }),
            second_reads: vec![(
                "/principalEntityType",
                json!(format!("MyCorp::User;configuration=cognito:userPoolArn={arn}")),
            )],
        },
    ];

    let router = router();
    for (index, pair) in pairs.iter().enumerate() {
        let case = pair.case;
        // One store per row, so what a row's store holds is only what that row
        // created.
        let store_id = store(&router).await;
        let token = format!("token-{index}");

        let mut first = pair.first.clone();
        first["policyStoreId"] = json!(store_id);
        first["clientToken"] = json!(token);
        let created = ok(&router, "CreateIdentitySource", &first).await;
        let created_id = created["identitySourceId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The same request under the same token is a replay, not a second create.
        let replay = ok(&router, "CreateIdentitySource", &first).await;
        assert_eq!(
            replay, created,
            "case {case:?}: the same parameters must replay the first result {created}, got {replay}"
        );

        // The partner request under that token is a different request.
        let mut second = pair.second.clone();
        second["policyStoreId"] = json!(store_id);
        second["clientToken"] = json!(token);
        let (status, body) = call(&router, "CreateIdentitySource", &second).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 for a reused token with different parameters, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected a ConflictException, got {body}"
        );
        assert_eq!(
            body["resources"],
            json!([{ "resourceId": created_id, "resourceType": "IDENTITY_SOURCE" }]),
            "case {case:?}: expected the conflict to name {created_id}, got {:?}",
            body["resources"]
        );
        assert_eq!(
            listed_source_ids(&router, &store_id).await,
            vec![created_id.clone()],
            "case {case:?}: the refused create must have persisted nothing, got {:?}",
            listed_source_ids(&router, &store_id).await
        );

        // Under a token of its own the second request is served, and what it
        // asked for is what state holds.
        second["clientToken"] = json!(format!("other-{index}"));
        let other = ok(&router, "CreateIdentitySource", &second).await;
        let other_id = other["identitySourceId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            other_id, created_id,
            "case {case:?}: expected a distinct identity source, got {other}"
        );
        let got = ok(
            &router,
            "GetIdentitySource",
            &json!({ "policyStoreId": store_id, "identitySourceId": other_id }),
        )
        .await;
        for (pointer, expected) in &pair.second_reads {
            assert_eq!(
                got.pointer(pointer),
                Some(expected),
                "case {case:?}: expected {pointer} to read back as {expected}, got {got}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The other creates that carry an @idempotencyToken
// ---------------------------------------------------------------------------

/// The same rule holds for every create in this service that carries an
/// `@idempotencyToken`: the rows are one pair per create, differing only in a
/// value that carries the rendering's own separators.
#[tokio::test]
async fn two_different_requests_under_one_client_token_conflict_in_every_create() {
    let router = router();
    let store_id = store(&router).await;

    // (case, operation, the member of the response that names the created
    //  resource, first request, second request)
    let pairs: Vec<(&str, &str, &str, Value, Value)> = vec![
        (
            // `PolicyStoreDescription` carries no pattern, so a description may
            // spell out the members that follow it.
            "policy-store-description-spells-the-following-members",
            "CreatePolicyStore",
            "policyStoreId",
            json!({
                "validationSettings": { "mode": "OFF" },
                "description": "shared;deletionProtection=\"ENABLED\"",
            }),
            json!({
                "validationSettings": { "mode": "OFF" },
                "description": "shared",
                "deletionProtection": "ENABLED",
            }),
        ),
        (
            // `TagKey` and `TagValue` carry no pattern either, so the same
            // characters split across a key or a value is a different tag set.
            "policy-store-tags-split-across-the-key-or-the-value",
            "CreatePolicyStore",
            "policyStoreId",
            json!({
                "validationSettings": { "mode": "OFF" },
                "tags": { "team;env": "prod" },
            }),
            json!({
                "validationSettings": { "mode": "OFF" },
                "tags": { "team": "env;prod" },
            }),
        ),
        (
            "policy-description-spells-the-following-members",
            "CreatePolicy",
            "policyId",
            json!({
                "policyStoreId": store_id,
                "definition": {
                    "static": {
                        "statement": STATEMENT,
                        "description": "shared;statement=",
                    }
                },
            }),
            json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": STATEMENT, "description": "shared" } },
            }),
        ),
        (
            "policy-template-description-spells-the-following-members",
            "CreatePolicyTemplate",
            "policyTemplateId",
            json!({
                "policyStoreId": store_id,
                "statement": TEMPLATE_STATEMENT,
                "name": "template-a",
                "description": "shared;name=template-b",
            }),
            json!({
                "policyStoreId": store_id,
                "statement": TEMPLATE_STATEMENT,
                "name": "template-b",
                "description": "shared",
            }),
        ),
    ];

    for (index, (case, operation, id_member, first, second)) in pairs.iter().enumerate() {
        let token = format!("shared-token-{index}");
        let mut first = first.clone();
        first["clientToken"] = json!(token);
        let created = ok(&router, operation, &first).await;
        let created_id = created[*id_member]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !created_id.is_empty(),
            "case {case:?}: expected {operation} to report a {id_member}, got {created}"
        );

        let replay = ok(&router, operation, &first).await;
        assert_eq!(
            replay, created,
            "case {case:?}: the same parameters must replay the first result {created}, got {replay}"
        );

        let mut second = second.clone();
        second["clientToken"] = json!(token);
        let (status, body) = call(&router, operation, &second).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 for a reused token with different parameters, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected a ConflictException, got {body}"
        );
        assert_eq!(
            body["resources"][0]["resourceId"],
            json!(created_id),
            "case {case:?}: expected the conflict to name {created_id}, got {body}"
        );
    }
}
