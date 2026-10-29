// SPDX-License-Identifier: CC0-1.0

//! Which refusal a request earns when it is faulty in TWO ways at once: it
//! references a resource this service does not hold AND it carries a payload the
//! operation itself refuses.
//!
//! The rule under test: an operation resolves every resource the request
//! REFERENCES before it reads the request's own payload, so a reference that
//! names nothing answers the 404 `ResourceNotFoundException` whatever else the
//! request got wrong — the same answer whichever operation the reference arrived
//! on. Against a resource that DOES exist the very same payload earns its 400
//! `ValidationException`, so what is pinned here is the PRECEDENCE and not a
//! blanket 404.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so
//! each answer is the one a client would really receive.

use serde_json::{json, Value};

use super::harness::{
    call, error_code, ok, router, store_with_validation_off, user_pool_arn, SIGNING_REGION,
};

/// A policy store id no store in a fresh engine carries.
const UNKNOWN_STORE: &str = "PSdoesnotexist11111111";

/// The prefix a `policyStoreId` carries when it references a store by ALIAS,
/// from `CreatePolicyStoreAliasInput$aliasName`'s model documentation: "The alias
/// name must always be prefixed with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The alias name no alias in a fresh engine carries.
const UNKNOWN_ALIAS_NAME: &str = "not-an-alias";

/// An identity source id no store holds.
const UNKNOWN_IDENTITY_SOURCE: &str = "ISdoesnotexist11111111";

/// A value shaped like a compact JWS: three `.`-separated base64url segments, so
/// it satisfies `Token`'s modelled pattern
/// (`^[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+.[A-Za-z0-9-_=]+$`) and the boundary
/// validator admits it — what the call then answers is the handler's decision,
/// which is what this suite is about. Nothing signed it, so no store would ever
/// verify it.
const TOKEN_SHAPED: &str = "aaaa.bbbb.cccc";

/// The `principal` member every well-formed row below carries.
fn principal() -> Value {
    json!({ "entityType": "User", "entityId": "dana" })
}

/// The `action` member every well-formed row below carries.
fn action() -> Value {
    json!({ "actionType": "Action", "actionId": "view" })
}

/// The `resource` member every well-formed row below carries.
fn resource() -> Value {
    json!({ "entityType": "Photo", "entityId": "p" })
}

/// A Cognito user pool ARN that satisfies `UserPoolArn`'s modelled pattern but
/// names no pool this emulator can reach: the pool id names the region the pool
/// was created in, and this one disagrees with the ARN's own region, which
/// `src/state.rs`'s `resolve_user_pool_arn` refuses.
fn unreachable_pool_arn() -> String {
    user_pool_arn(SIGNING_REGION, "eu-west-1_1a2b3c4d5")
}

/// A Cognito user pool ARN this service resolves: the emulator's one account and
/// the region the harness signs for, with a pool id naming that same region.
fn reachable_pool_arn() -> String {
    user_pool_arn(SIGNING_REGION, &format!("{SIGNING_REGION}_1a2b3c4d5"))
}

/// One request whose PAYLOAD this service refuses on its own checks.
struct Fault {
    /// The operation to call.
    operation: &'static str,
    /// What is wrong with the payload, for the case name.
    fault: &'static str,
    /// The member `ValidationException$fieldList` must name when the referenced
    /// resources all exist.
    field: &'static str,
    /// The request body, against the `policyStoreId` and `identitySourceId` the
    /// row supplies.
    body: fn(&str, &str) -> Value,
}

/// Every payload refusal this suite pairs with a reference that names nothing.
///
/// The rows differ along the OPERATION (single, token-based and batch
/// authorization, and the two identity-source writers) and along the KIND of
/// payload fault (an absent member of the Cedar triple, an absent token, a
/// payload that is no Cedar JSON, a provider reference that resolves to nothing,
/// a member whose value cannot be defaulted).
const FAULTS: &[Fault] = &[
    Fault {
        operation: "IsAuthorized",
        fault: "an absent action",
        field: "/action",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "principal": principal(),
                "resource": resource(),
            })
        },
    },
    Fault {
        operation: "IsAuthorized",
        fault: "an absent principal",
        field: "/principal",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "action": action(),
                "resource": resource(),
            })
        },
    },
    Fault {
        operation: "IsAuthorized",
        fault: "a context that is no Cedar JSON",
        field: "/context/cedarJson",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "principal": principal(),
                "action": action(),
                "resource": resource(),
                "context": { "cedarJson": "not-json" },
            })
        },
    },
    Fault {
        operation: "IsAuthorized",
        fault: "entities that are no Cedar JSON",
        field: "/entities/cedarJson",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "principal": principal(),
                "action": action(),
                "resource": resource(),
                "entities": { "cedarJson": "not-json" },
            })
        },
    },
    Fault {
        operation: "IsAuthorizedWithToken",
        fault: "no token presented",
        field: "/accessToken",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "action": action(),
                "resource": resource(),
            })
        },
    },
    Fault {
        operation: "IsAuthorizedWithToken",
        fault: "an absent action beside a token",
        field: "/action",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "identityToken": TOKEN_SHAPED,
                "resource": resource(),
            })
        },
    },
    Fault {
        operation: "IsAuthorizedWithToken",
        fault: "a context that is no Cedar JSON",
        field: "/context/cedarJson",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "identityToken": TOKEN_SHAPED,
                "action": action(),
                "resource": resource(),
                "context": { "cedarJson": "not-json" },
            })
        },
    },
    Fault {
        operation: "BatchIsAuthorized",
        fault: "a request with an absent principal",
        field: "/requests/0/principal",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "requests": [{ "action": action(), "resource": resource() }],
            })
        },
    },
    Fault {
        operation: "BatchIsAuthorized",
        fault: "a request whose context is no Cedar JSON",
        field: "/requests/0/context/cedarJson",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "requests": [{
                    "principal": principal(),
                    "action": action(),
                    "resource": resource(),
                    "context": { "cedarJson": "not-json" },
                }],
            })
        },
    },
    Fault {
        operation: "CreateIdentitySource",
        fault: "a user pool ARN naming no reachable pool",
        field: "/configuration/cognitoUserPoolConfiguration/userPoolArn",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "configuration": {
                    "cognitoUserPoolConfiguration": { "userPoolArn": unreachable_pool_arn() }
                },
            })
        },
    },
    Fault {
        operation: "CreateIdentitySource",
        fault: "an OpenID Connect source with no principalEntityType",
        field: "/principalEntityType",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://idp.example.com",
                        "tokenSelection": {
                            "identityTokenOnly": { "clientIds": ["a1b2c3"] }
                        },
                    }
                },
            })
        },
    },
    Fault {
        operation: "UpdateIdentitySource",
        fault: "a user pool ARN naming no reachable pool",
        // The update carries its provider in `updateConfiguration`, so the
        // pointer this operation's refusal names is rooted there.
        field: "/updateConfiguration/cognitoUserPoolConfiguration/userPoolArn",
        body: |store, source| {
            json!({
                "policyStoreId": store,
                "identitySourceId": source,
                "updateConfiguration": {
                    "cognitoUserPoolConfiguration": { "userPoolArn": unreachable_pool_arn() }
                },
            })
        },
    },
];

/// What a row expects of the call.
struct Expect {
    status: u16,
    code: &'static str,
    /// For a 404, the `(resourceType, resourceId)` the exception must name; for a
    /// 400, `None`.
    resource: Option<(&'static str, String)>,
}

/// A reference to a policy store that does not exist outranks every refusal of
/// the request's own payload, on every operation that takes one — and the same
/// payload against the store that DOES exist still earns its 400 naming the
/// member at fault.
#[tokio::test]
async fn an_unresolvable_policy_store_outranks_a_payload_refusal() {
    let router = router();
    let live_store = store_with_validation_off(&router).await;
    // One identity source to update: the `UpdateIdentitySource` rows reference a
    // source that really exists, so the only resource missing on a 404 row is the
    // store.
    let created = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": live_store,
            "configuration": {
                "cognitoUserPoolConfiguration": { "userPoolArn": reachable_pool_arn() }
            },
        }),
    )
    .await;
    let source_id = created["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();

    for fault in FAULTS {
        // The rows differ along WHAT the `policyStoreId` names: nothing at all, an
        // alias naming nothing, and the store that does exist.
        let references: [(&str, String, Expect); 3] = [
            (
                "a store id no store carries",
                UNKNOWN_STORE.to_string(),
                Expect {
                    status: 404,
                    code: "ResourceNotFoundException",
                    resource: Some(("POLICY_STORE", UNKNOWN_STORE.to_string())),
                },
            ),
            (
                "an alias naming nothing",
                format!("{ALIAS_PREFIX}{UNKNOWN_ALIAS_NAME}"),
                Expect {
                    status: 404,
                    code: "ResourceNotFoundException",
                    // The reference named an ALIAS, so the alias is the resource
                    // reported missing, by its bare name.
                    resource: Some(("POLICY_STORE_ALIAS", UNKNOWN_ALIAS_NAME.to_string())),
                },
            ),
            (
                "the store that does exist",
                live_store.clone(),
                Expect {
                    status: 400,
                    code: "ValidationException",
                    resource: None,
                },
            ),
        ];

        for (reference_case, reference, expected) in references {
            let case = format!(
                "{} with {} against {reference_case}",
                fault.operation, fault.fault
            );
            let body = (fault.body)(&reference, &source_id);
            let (status, response) = call(&router, fault.operation, &body).await;
            assert_eq!(
                status, expected.status,
                "case {case:?}: expected HTTP {}, got {status} {response}",
                expected.status
            );
            assert_eq!(
                error_code(&response),
                expected.code,
                "case {case:?}: expected {}, got {response}",
                expected.code
            );
            match &expected.resource {
                Some((resource_type, resource_id)) => {
                    assert_eq!(
                        response["resourceType"].as_str(),
                        Some(*resource_type),
                        "case {case:?}: expected resourceType {resource_type}, got {response}"
                    );
                    assert_eq!(
                        response["resourceId"].as_str(),
                        Some(resource_id.as_str()),
                        "case {case:?}: expected resourceId {resource_id}, got {response}"
                    );
                }
                None => {
                    let paths: Vec<&str> = response["fieldList"]
                        .as_array()
                        .map(|fields| {
                            fields
                                .iter()
                                .filter_map(|field| field["path"].as_str())
                                .collect()
                        })
                        .unwrap_or_default();
                    assert!(
                        paths.contains(&fault.field),
                        "case {case:?}: expected fieldList to name {}, got {paths:?} in {response}",
                        fault.field
                    );
                }
            }
        }
    }
}

/// The same precedence for a SECOND resource a request references: an
/// `identitySourceId` naming nothing outranks a configuration the operation would
/// refuse, and neither answer leaves the stored source changed.
#[tokio::test]
async fn an_unresolvable_identity_source_outranks_a_payload_refusal() {
    let router = router();
    let store = store_with_validation_off(&router).await;
    let created = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store,
            "configuration": {
                "cognitoUserPoolConfiguration": { "userPoolArn": reachable_pool_arn() }
            },
        }),
    )
    .await;
    let source_id = created["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();

    // (case, identitySourceId, expected status, expected code)
    let cases: [(&str, String, u16, &str); 2] = [
        (
            "an identity source id no store holds",
            UNKNOWN_IDENTITY_SOURCE.to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the identity source that does exist",
            source_id.clone(),
            400,
            "ValidationException",
        ),
    ];
    for (case, identity_source_id, status_expected, code) in cases {
        let (status, response) = call(
            &router,
            "UpdateIdentitySource",
            &json!({
                "policyStoreId": store,
                "identitySourceId": identity_source_id,
                "updateConfiguration": {
                    "cognitoUserPoolConfiguration": { "userPoolArn": unreachable_pool_arn() }
                },
            }),
        )
        .await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            code,
            "case {case:?}: expected {code}, got {response}"
        );
    }

    // Neither refusal may have reached state: the source still carries the pool
    // the create resolved.
    let read_back = ok(
        &router,
        "GetIdentitySource",
        &json!({ "policyStoreId": store, "identitySourceId": source_id }),
    )
    .await;
    assert_eq!(
        read_back["configuration"]["cognitoUserPoolConfiguration"]["userPoolArn"].as_str(),
        Some(reachable_pool_arn().as_str()),
        "case \"a refused update changes nothing\": the stored pool must be unchanged, got \
         {read_back}"
    );
}
