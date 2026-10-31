// SPDX-License-Identifier: CC0-1.0

//! Who may hold the service's own default `principalEntityType`.
//!
//! The rule under test, one sentence: an identity source's stored
//! `principalEntityType` is only ever a value the CALLER named or the default of
//! the provider kind the source actually points at, so a write — by either
//! writer of that member — that would leave the service's Cognito-only default on
//! a provider kind with no default of its own is refused with a 400
//! `ValidationException` naming `principalEntityType`, and persists nothing.
//!
//! `CreateIdentitySource` states half of it already: it resolves an omitted
//! member to [`DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE`] for a Cognito user pool
//! source and refuses an OIDC source that names none, because an OIDC source's
//! principals are of a type only the caller's Cedar schema supplies.
//! `UpdateIdentitySource` writes the same member, so the rule is the same there:
//! a value the caller once named carries over, a value the SERVICE defaulted does
//! not follow the source onto another provider kind.
//!
//! Every row drives a source through a SEQUENCE of updates, because the member's
//! provenance — caller-named or service-defaulted — is what each write decides,
//! and only a sequence can tell the two apart. Every assertion goes over the wire
//! through the shared [`crate::harness`], so a refusal only counts when nothing
//! reached state behind it.

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::state::DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, error_code, field_list, ok, router, store_with_validation_off, user_pool_arn,
};

/// The region every request in this suite is signed for, so a pool ARN and the
/// emulator agree on the region.
const REGION: &str = crate::harness::SIGNING_REGION;

/// An OIDC issuer: a provider OUTSIDE the emulator, accepted as given (the
/// model's `^https://.*$`).
const OIDC_ISSUER: &str = "https://auth.example.com";

/// A second issuer, for a row that replaces one OIDC provider with another.
const OTHER_OIDC_ISSUER: &str = "https://accounts.example.org/tenant";

/// The member a refusal of this rule addresses: `principalEntityType`, which both
/// writers declare at the top level of their input
/// (`smithy_query(query="operation", operation="CreateIdentitySource,UpdateIdentitySource")`).
const PRINCIPAL_ENTITY_TYPE_POINTER: &str = "/principalEntityType";

/// What one write of an identity source answers, and what every read of it must
/// report afterwards.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// Accepted, leaving this `principalEntityType` stored.
    Stores(&'static str),
    /// Refused with the 400 `ValidationException` naming `principalEntityType`,
    /// leaving the source exactly as it was.
    Refused,
}

/// One update in a row's sequence: the request members beside the ids, and what
/// the write must answer.
#[derive(Debug, Clone)]
struct Step {
    update: Value,
    expect: Expect,
}

/// One row: the identity source a create puts in the store, and the updates it is
/// then driven through, in order.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    create: Value,
    steps: Vec<Step>,
}

/// An OIDC provider description, as either writer's configuration union carries
/// it.
fn oidc(issuer: &str) -> Value {
    json!({ "openIdConnectConfiguration": {
        "issuer": issuer,
        "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
    } })
}

/// A Cognito user pool provider description, naming a pool the user-pool service
/// really holds.
fn cognito(arn: &str) -> Value {
    json!({ "cognitoUserPoolConfiguration": { "userPoolArn": arn } })
}

/// The `principalEntityType` `ListIdentitySources` reports for one source of
/// `store_id` — the other read of the same stored member, so a row proves both
/// reads agree.
async fn listed_principal_entity_type(
    router: &MultiServiceRouter,
    store_id: &str,
    source_id: &str,
) -> Value {
    let listed = ok(
        router,
        "ListIdentitySources",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    listed["identitySources"]
        .as_array()
        .and_then(|sources| {
            sources
                .iter()
                .find(|source| source["identitySourceId"] == json!(source_id))
        })
        .map(|source| source["principalEntityType"].clone())
        .unwrap_or(Value::Null)
}

/// The union arm a configuration carries, as the member name the request spelled
/// it with — what a read must report back for a replacement to have landed.
fn arm(configuration: &Value) -> String {
    configuration
        .as_object()
        .and_then(|members| members.keys().next().cloned())
        .unwrap_or_default()
}

/// RULE: the stored `principalEntityType` is the caller's value where the caller
/// named one and the default of the provider kind the source points at otherwise,
/// so an update that would leave the Cognito-only default on an OpenID Connect
/// source is refused and changes nothing.
///
/// The rows span the provider kind the source starts on (Cognito, OIDC) and the
/// one it is updated to (the same kind, the other kind), whether the member was
/// named on the create, on a later update or never, the case where the caller
/// names the service's own default value ITSELF — which is a choice, not a default
/// — and the length of the sequence, so a defaulted value is followed across
/// several writes.
#[tokio::test]
async fn a_defaulted_principal_entity_type_never_crosses_to_another_provider_kind() {
    let router = router();
    let pool = crate::harness::user_pool(&router, "entity-type-pool").await;
    let arn = user_pool_arn(REGION, &pool);
    let other_pool = crate::harness::user_pool(&router, "entity-type-other-pool").await;
    let other_arn = user_pool_arn(REGION, &other_pool);

    let rows: Vec<Row> = vec![
        Row {
            // The default follows the source while it stays on the kind the
            // default belongs to.
            case: "the Cognito default, updated to another Cognito pool",
            create: json!({ "configuration": cognito(&arn) }),
            steps: vec![Step {
                update: json!({ "updateConfiguration": cognito(&other_arn) }),
                expect: Expect::Stores(DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE),
            }],
        },
        Row {
            // The defect this suite was written for: the update crosses to OIDC
            // carrying nothing, so there is no entity type left to report.
            case: "the Cognito default, updated to OIDC naming no entity type",
            create: json!({ "configuration": cognito(&arn) }),
            steps: vec![
                Step {
                    update: json!({ "updateConfiguration": oidc(OIDC_ISSUER) }),
                    expect: Expect::Refused,
                },
                // The same update, with the member the refusal asked for.
                Step {
                    update: json!({
                        "principalEntityType": "MyCorp::User",
                        "updateConfiguration": oidc(OIDC_ISSUER),
                    }),
                    expect: Expect::Stores("MyCorp::User"),
                },
            ],
        },
        Row {
            case: "an entity type the create named, carried onto OIDC",
            create: json!({
                "principalEntityType": "MyCorp::User",
                "configuration": cognito(&arn),
            }),
            steps: vec![Step {
                update: json!({ "updateConfiguration": oidc(OIDC_ISSUER) }),
                expect: Expect::Stores("MyCorp::User"),
            }],
        },
        Row {
            // The caller NAMED the value the service would also have defaulted
            // to, which makes it the caller's choice and not a default, so it
            // crosses like any other chosen value.
            case: "the caller names the service's own default itself",
            create: json!({
                "principalEntityType": DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE,
                "configuration": cognito(&arn),
            }),
            steps: vec![Step {
                update: json!({ "updateConfiguration": oidc(OIDC_ISSUER) }),
                expect: Expect::Stores(DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE),
            }],
        },
        Row {
            // An UPDATE names the member, so from then on the stored value is
            // the caller's and the next update carries it over.
            case: "an update names the entity type, a later update keeps it",
            create: json!({ "configuration": cognito(&arn) }),
            steps: vec![
                Step {
                    update: json!({
                        "principalEntityType": "MyCorp::Staff",
                        "updateConfiguration": cognito(&other_arn),
                    }),
                    expect: Expect::Stores("MyCorp::Staff"),
                },
                Step {
                    update: json!({ "updateConfiguration": oidc(OTHER_OIDC_ISSUER) }),
                    expect: Expect::Stores("MyCorp::Staff"),
                },
            ],
        },
        Row {
            // The other direction: an OIDC source's entity type is always the
            // caller's, so it carries onto a Cognito pool rather than being
            // replaced by the default.
            case: "an OIDC source's entity type, updated to a Cognito pool",
            create: json!({
                "principalEntityType": "MyCorp::Person",
                "configuration": oidc(OIDC_ISSUER),
            }),
            steps: vec![Step {
                update: json!({ "updateConfiguration": cognito(&arn) }),
                expect: Expect::Stores("MyCorp::Person"),
            }],
        },
        Row {
            case: "an OIDC source, updated to another OIDC provider",
            create: json!({
                "principalEntityType": "MyCorp::Person",
                "configuration": oidc(OIDC_ISSUER),
            }),
            steps: vec![Step {
                update: json!({ "updateConfiguration": oidc(OTHER_OIDC_ISSUER) }),
                expect: Expect::Stores("MyCorp::Person"),
            }],
        },
        Row {
            // A longer sequence: the default stays a default across updates that
            // keep the source on a Cognito pool, so the crossing is still
            // refused several writes later.
            case: "the default survives Cognito updates and still cannot cross",
            create: json!({ "configuration": cognito(&arn) }),
            steps: vec![
                Step {
                    update: json!({ "updateConfiguration": cognito(&other_arn) }),
                    expect: Expect::Stores(DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE),
                },
                Step {
                    update: json!({ "updateConfiguration": cognito(&arn) }),
                    expect: Expect::Stores(DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE),
                },
                Step {
                    update: json!({ "updateConfiguration": oidc(OIDC_ISSUER) }),
                    expect: Expect::Refused,
                },
            ],
        },
    ];

    let store_id = store_with_validation_off(&router).await;
    for row in rows {
        let case = row.case;
        let mut create = row.create.clone();
        create["policyStoreId"] = json!(store_id);
        let created = ok(&router, "CreateIdentitySource", &create).await;
        let source_id = created["identitySourceId"]
            .as_str()
            .expect("identitySourceId is required on the CreateIdentitySource response")
            .to_string();
        let read = json!({ "policyStoreId": store_id, "identitySourceId": source_id });

        for (index, step) in row.steps.iter().enumerate() {
            let before = ok(&router, "GetIdentitySource", &read).await;
            let mut update = step.update.clone();
            update["policyStoreId"] = json!(store_id);
            update["identitySourceId"] = json!(source_id);
            let (status, body) = call(&router, "UpdateIdentitySource", &update).await;

            match step.expect {
                Expect::Refused => {
                    assert_eq!(
                        status, 400,
                        "case {case:?} step {index}: expected HTTP 400, got {status} {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        "ValidationException",
                        "case {case:?} step {index}: expected a ValidationException, got {body}"
                    );
                    let refused = field_list(&body);
                    assert_eq!(
                        refused.iter().map(|field| field.0.as_str()).collect::<Vec<_>>(),
                        vec![PRINCIPAL_ENTITY_TYPE_POINTER],
                        "case {case:?} step {index}: expected the refusal to name \
                         {PRINCIPAL_ENTITY_TYPE_POINTER}, got {refused:?}"
                    );
                    let after = ok(&router, "GetIdentitySource", &read).await;
                    assert_eq!(
                        after, before,
                        "case {case:?} step {index}: a refused update must change nothing, \
                         expected {before}, got {after}"
                    );
                }
                Expect::Stores(expected) => {
                    assert_eq!(
                        status, 200,
                        "case {case:?} step {index}: expected HTTP 200, got {status} {body}"
                    );
                    let after = ok(&router, "GetIdentitySource", &read).await;
                    assert_eq!(
                        after["principalEntityType"], json!(expected),
                        "case {case:?} step {index}: expected principalEntityType {expected:?}, \
                         got {:?}",
                        after["principalEntityType"]
                    );
                    // The replacement really landed, so the entity type above is
                    // the one stored BESIDE the provider the update named.
                    let expected_arm = arm(&step.update["updateConfiguration"]);
                    assert!(
                        after["configuration"].get(&expected_arm).is_some(),
                        "case {case:?} step {index}: expected the read to report a \
                         {expected_arm:?} configuration, got {:?}",
                        after["configuration"]
                    );
                    let listed =
                        listed_principal_entity_type(&router, &store_id, &source_id).await;
                    assert_eq!(
                        listed, json!(expected),
                        "case {case:?} step {index}: expected the listing to report \
                         principalEntityType {expected:?}, got {listed:?}"
                    );
                }
            }
        }
    }
}

/// RULE (the same one, on the wording): the two writers of the stored member
/// refuse an OIDC provider with no entity type behind it the SAME way — the same
/// status, the same error code, and the same `fieldList` entry naming
/// `principalEntityType` for the same reason — because one rule has one wording
/// wherever it is applied.
#[tokio::test]
async fn both_writers_refuse_an_oidc_source_without_an_entity_type_identically() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;
    let arn = user_pool_arn(
        REGION,
        &crate::harness::user_pool(&router, "both-writers-pool").await,
    );
    let provider = oidc(OIDC_ISSUER);

    // A source holding the service default, for the update half: it took the
    // default because its create named no entity type.
    let created = ok(
        &router,
        "CreateIdentitySource",
        &json!({ "policyStoreId": store_id, "configuration": cognito(&arn) }),
    )
    .await;
    let source_id = created["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();

    let (create_status, create_body) = call(
        &router,
        "CreateIdentitySource",
        &json!({ "policyStoreId": store_id, "configuration": provider }),
    )
    .await;
    let (update_status, update_body) = call(
        &router,
        "UpdateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "identitySourceId": source_id,
            "updateConfiguration": provider,
        }),
    )
    .await;

    assert_eq!(
        create_status, 400,
        "expected the create to refuse an OIDC provider with no entity type, got \
         {create_status} {create_body}"
    );
    assert_eq!(
        update_status, create_status,
        "expected the update to answer HTTP {create_status} as the create does, got \
         {update_status} {update_body}"
    );
    assert_eq!(
        error_code(&create_body),
        "ValidationException",
        "expected a ValidationException from the create, got {create_body}"
    );
    assert_eq!(
        error_code(&update_body),
        error_code(&create_body),
        "expected the update to answer a {} as the create does, got {update_body}",
        error_code(&create_body)
    );
    assert_eq!(
        field_list(&update_body),
        field_list(&create_body),
        "expected both writers to refuse the same member for the same reason, got {:?} from the \
         create and {:?} from the update",
        field_list(&create_body),
        field_list(&update_body)
    );
    assert_eq!(
        field_list(&create_body)
            .iter()
            .map(|field| field.0.as_str())
            .collect::<Vec<_>>(),
        vec![PRINCIPAL_ENTITY_TYPE_POINTER],
        "expected the refusal to name {PRINCIPAL_ENTITY_TYPE_POINTER}, got {:?}",
        field_list(&create_body)
    );
}
