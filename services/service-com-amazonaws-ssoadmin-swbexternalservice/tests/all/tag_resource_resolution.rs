// SPDX-License-Identifier: CC0-1.0

//! The ONE resolution the whole tag surface shares, driven end to end over the
//! `awsJson1_1` wire through an operation of each kind that performs it.
//!
//! The rule under test, which `TagResource`, `UntagResource` and `ListTagsForResource` all
//! apply to the resource a request names:
//!
//! 1. when the optional `InstanceArn` is sent, it names the instance the resource is
//!    expected to live under, so an ARN this account holds no instance for can hold no
//!    resource either and the call is refused with the modelled
//!    `ResourceNotFoundException`;
//! 2. the resource is then whichever of the four kinds the model's `TaggableResourceArn`
//!    admits (an instance, a permission set, an application or a trusted token issuer) the
//!    ARN names, scoped by that `InstanceArn` — never created, already deleted, gone with
//!    its instance or held under another instance is the same answer either way, the
//!    modelled `ResourceNotFoundException`;
//! 3. and for the two WRITES only, a resource whose holding instance is still being
//!    created is refused with the modelled `ConflictException`, while the REPORT of its
//!    tags is served.
//!
//! Each refusal is asserted on its wire code AND on its message, over a table spanning the
//! four resource kinds and every way the resolution can fail, so the three operations are
//! pinned to one answer rather than to three that happen to agree today.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the tests run
//! against an ORGANIZATION instance so that all four taggable kinds — permission sets
//! among them — can be created.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::application_authentication_methods::create_application;
use crate::inline_policy::create_permission_set;
use crate::instance_create::hold_activation;
use crate::ssoadmin_permission_sets::{
    call, error_code, organization_engine, router_over, with_store, TEST_REGION,
    UNKNOWN_INSTANCE_ARN, UNKNOWN_PERMISSION_SET_ARN,
};

/// The only value the model's `TrustedTokenIssuerType` enum admits
/// (`smithy_query(query="shape",shape="TrustedTokenIssuerType")` → `["OIDC_JWT"]`).
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")`).
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// The tag every resource of the fixture is seeded with, so a report is a non-empty answer
/// and a refused write can be shown to have left it alone.
const SEED_TAG: (&str, &str) = ("suite", "resolution");

/// The instance id of the SECOND instance the scoping rows need: `ssoins-` + 16 of
/// `[a-zA-Z0-9-.]`, as the model's `InstanceArn` pattern requires.
const OTHER_INSTANCE_ID: &str = "ssoins-9a8b7c6d5e4f3021";

/// Its identity store: `d-` + 10 hex digits, per `InstanceIdentityStoreArn`.
const OTHER_IDENTITY_STORE_ID: &str = "d-9a8b7c6d5e";

/// Create one trusted token issuer under `instance_arn` and return its ARN.
async fn create_trusted_token_issuer(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateTrustedTokenIssuer",
        &json!({
            "InstanceArn": instance_arn,
            "Name": name,
            "TrustedTokenIssuerType": ISSUER_TYPE,
            "TrustedTokenIssuerConfiguration": {
                "OidcJwtConfiguration": {
                    "IssuerUrl": "https://issuer.example.com/oidc",
                    "ClaimAttributePath": "email",
                    "IdentityStoreAttributePath": "emails.primary.value",
                    "JwksRetrievalOption": JWKS_RETRIEVAL_OPTION,
                }
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTrustedTokenIssuer {name:?}: expected 200, got {status} {body}"
    );
    body["TrustedTokenIssuerArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateTrustedTokenIssuer {name:?} minted no ARN: {body}"))
        .to_string()
}

/// `TagResource` for `resource_arn`, with the optional `InstanceArn` the caller chose.
async fn tag_resource(
    router: &MultiServiceRouter,
    instance_arn: Option<&str>,
    resource_arn: &str,
    tag: (&str, &str),
) -> (u16, Value) {
    let mut body = json!({
        "ResourceArn": resource_arn,
        "Tags": [{"Key": tag.0, "Value": tag.1}],
    });
    if let Some(instance_arn) = instance_arn {
        body["InstanceArn"] = json!(instance_arn);
    }
    call(router, "TagResource", &body).await
}

/// `UntagResource` for `resource_arn`, naming one key.
async fn untag_resource(
    router: &MultiServiceRouter,
    instance_arn: Option<&str>,
    resource_arn: &str,
    key: &str,
) -> (u16, Value) {
    let mut body = json!({"ResourceArn": resource_arn, "TagKeys": [key]});
    if let Some(instance_arn) = instance_arn {
        body["InstanceArn"] = json!(instance_arn);
    }
    call(router, "UntagResource", &body).await
}

/// `ListTagsForResource` for `resource_arn`.
async fn list_tags(
    router: &MultiServiceRouter,
    instance_arn: Option<&str>,
    resource_arn: &str,
) -> (u16, Value) {
    let mut body = json!({"ResourceArn": resource_arn});
    if let Some(instance_arn) = instance_arn {
        body["InstanceArn"] = json!(instance_arn);
    }
    call(router, "ListTagsForResource", &body).await
}

/// The `(Key, Value)` pairs a `ListTagsForResource` answer reported, in the order it
/// reported them.
fn reported(body: &Value) -> Vec<(String, String)> {
    body["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer carries no Tags list: {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The seeded tag set, as the ordered pairs a report must name.
fn seed_pairs() -> Vec<(String, String)> {
    vec![(SEED_TAG.0.to_string(), SEED_TAG.1.to_string())]
}

/// Assert one call was refused with `expected_code` AND with exactly `expected_message`:
/// the message is part of the answer, so a wording that drifted between two copies of the
/// rule is a failure here.
fn assert_refusal(
    case: &str,
    what: &str,
    expected_code: &str,
    expected_message: &str,
    status: u16,
    body: &Value,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: {what} must be refused 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        expected_code,
        "case {case:?}: {what} expected {expected_code}, got {body}"
    );
    assert_eq!(
        body["Message"].as_str().unwrap_or_default(),
        expected_message,
        "case {case:?}: {what} expected the message {expected_message:?}, got {body}"
    );
}

/// The fixture every test here runs against: an organization instance holding one resource
/// of each kind the model's `TaggableResourceArn` admits, each carrying [`SEED_TAG`], plus
/// a SECOND instance this account holds that holds none of them.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    instance_arn: String,
    other_instance_arn: String,
    /// `(the kind's label, the resource's ARN, an ARN of that kind nothing was created
    /// under)`. The instance is a taggable resource itself, so it is the first row.
    resources: Vec<(String, String, String)>,
}

/// Build that fixture, creating every resource over the wire.
async fn fixture(suffix: &str) -> Fixture {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));

    let permission_set_arn =
        create_permission_set(&router, &instance_arn, &format!("ps-{suffix}")).await;
    let application_arn =
        create_application(&router, &instance_arn, &format!("app {suffix}")).await;
    let issuer_arn =
        create_trusted_token_issuer(&router, &instance_arn, &format!("tti-{suffix}")).await;

    // The instance id and the account, read off ARNs the emulator minted rather than
    // restated here, so the never-created ARNs below are shaped exactly like the real ones.
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("the instance ARN carries no id: {instance_arn}"))
        .to_string();
    let account = application_arn
        .split(':')
        .nth(4)
        .unwrap_or_else(|| panic!("the application ARN carries no account: {application_arn}"))
        .to_string();

    let resources = vec![
        (
            "an instance".to_string(),
            instance_arn.clone(),
            UNKNOWN_INSTANCE_ARN.to_string(),
        ),
        (
            "a permission set".to_string(),
            permission_set_arn,
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
        (
            "an application".to_string(),
            application_arn.clone(),
            format!("arn:aws:sso::{account}:application/{instance_id}/apl-0000000000000000"),
        ),
        (
            "a trusted token issuer".to_string(),
            issuer_arn,
            format!(
                "arn:aws:sso::{account}:trustedTokenIssuer/{instance_id}/\
                 tti-00000000-0000-4000-8000-000000000000"
            ),
        ),
    ];

    // Each resource carries the seed tag, so a report is a non-empty answer and a refused
    // write can be shown to have left the stored set alone.
    for (kind, resource_arn, _) in &resources {
        let (status, body) =
            tag_resource(&router, Some(&instance_arn), resource_arn, SEED_TAG).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: seeding the tag must answer 200, got {status} {body}"
        );
    }

    // A SECOND instance this account holds, seeded through the same `state::Instance`
    // constructor the handlers use: no operation of this API enables an organization
    // instance, and the scoping rows need an instance that EXISTS but holds none of the
    // resources above.
    let other_instance_arn = with_store(&engine, |store| {
        let instance = state::Instance::active(
            OTHER_INSTANCE_ID,
            OTHER_IDENTITY_STORE_ID,
            state::InstanceType::Organization,
            Some("SecondOrganization".to_string()),
            aws_smithy_types::DateTime::from(SystemTime::now()),
            TEST_REGION,
        );
        let arn = instance.instance_arn.clone();
        store.instances.insert(arn.clone(), instance);
        arn
    });

    Fixture {
        router,
        engine,
        instance_arn,
        other_instance_arn,
        resources,
    }
}

/// One row of the resolution table: the ARNs a tag request names, and the refusal EVERY
/// tag operation must answer it with.
struct Unresolvable {
    label: String,
    instance_arn: Option<String>,
    resource_arn: String,
    message: String,
}

/// The rows, spanning the four resource kinds and every way the resolution can fail: an
/// `InstanceArn` this account holds no instance for, an `InstanceArn` it does hold that
/// holds none of these resources, and a `ResourceArn` nothing was created under — the last
/// one both with and without the optional `InstanceArn`, since the member is optional.
fn unresolvable_rows(fixture: &Fixture) -> Vec<Unresolvable> {
    let mut rows = Vec::new();
    for (kind, resource_arn, never_created_arn) in &fixture.resources {
        rows.push(Unresolvable {
            label: format!("{kind} under an InstanceArn this account holds no instance for"),
            instance_arn: Some(UNKNOWN_INSTANCE_ARN.to_string()),
            resource_arn: resource_arn.clone(),
            message: state::instance_not_found_message(UNKNOWN_INSTANCE_ARN),
        });
        rows.push(Unresolvable {
            label: format!("{kind} under an InstanceArn that holds another instance's resources"),
            instance_arn: Some(fixture.other_instance_arn.clone()),
            resource_arn: resource_arn.clone(),
            message: state::taggable_resource_not_found_message(resource_arn),
        });
        rows.push(Unresolvable {
            label: format!("a {kind} ARN nothing was created under, scoped to the instance"),
            instance_arn: Some(fixture.instance_arn.clone()),
            resource_arn: never_created_arn.clone(),
            message: state::taggable_resource_not_found_message(never_created_arn),
        });
        rows.push(Unresolvable {
            label: format!("a {kind} ARN nothing was created under, with no InstanceArn"),
            instance_arn: None,
            resource_arn: never_created_arn.clone(),
            message: state::taggable_resource_not_found_message(never_created_arn),
        });
    }
    rows
}

/// Steps 1 and 2: the write, the removal and the report resolve the resource a request
/// names by the SAME rule, so an unresolvable request earns the same refusal — code and
/// message — from all three, and none of them writes.
#[tokio::test]
async fn every_tag_operation_refuses_an_unresolvable_resource_identically() {
    let fixture = fixture("unresolvable").await;

    for row in unresolvable_rows(&fixture) {
        let case = row.label.as_str();
        let instance_arn = row.instance_arn.as_deref();

        let (status, body) = tag_resource(
            &fixture.router,
            instance_arn,
            &row.resource_arn,
            ("env", "prod"),
        )
        .await;
        assert_refusal(
            case,
            "TagResource",
            "ResourceNotFoundException",
            &row.message,
            status,
            &body,
        );

        let (status, body) =
            untag_resource(&fixture.router, instance_arn, &row.resource_arn, SEED_TAG.0).await;
        assert_refusal(
            case,
            "UntagResource",
            "ResourceNotFoundException",
            &row.message,
            status,
            &body,
        );

        let (status, body) = list_tags(&fixture.router, instance_arn, &row.resource_arn).await;
        assert_refusal(
            case,
            "ListTagsForResource",
            "ResourceNotFoundException",
            &row.message,
            status,
            &body,
        );
    }

    // The refusals stored nothing and removed nothing: every held resource still carries
    // exactly the set it was seeded with.
    for (kind, resource_arn, _) in &fixture.resources {
        let (status, listed) =
            list_tags(&fixture.router, Some(&fixture.instance_arn), resource_arn).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: the held resource must still list, got {status} {listed}"
        );
        assert_eq!(
            reported(&listed),
            seed_pairs(),
            "case {kind:?}: a refused call must leave the stored set alone, got {listed}"
        );
    }
}

/// The other half of steps 1 and 2: a request the rule DOES resolve is served by all three
/// operations, whether or not it carries the optional `InstanceArn`, on every resource kind
/// — so the refusals above are about the ARNs asked for and nothing else.
#[tokio::test]
async fn every_tag_operation_resolves_a_resource_the_account_holds() {
    let fixture = fixture("resolved").await;

    for (kind, resource_arn, _) in &fixture.resources {
        for scope in [Some(fixture.instance_arn.clone()), None] {
            let case = match &scope {
                Some(_) => format!("{kind} scoped to its holding instance"),
                None => format!("{kind} with no InstanceArn"),
            };
            let instance_arn = scope.as_deref();

            let (status, body) = tag_resource(
                &fixture.router,
                instance_arn,
                resource_arn,
                ("stage", "beta"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: TagResource must answer 200, got {status} {body}"
            );

            let (status, listed) = list_tags(&fixture.router, instance_arn, resource_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListTagsForResource must answer 200, got {status} {listed}"
            );
            assert_eq!(
                reported(&listed),
                vec![
                    ("stage".to_string(), "beta".to_string()),
                    (SEED_TAG.0.to_string(), SEED_TAG.1.to_string()),
                ],
                "case {case:?}: the report must name the stored set in key order, got {listed}"
            );

            let (status, body) =
                untag_resource(&fixture.router, instance_arn, resource_arn, "stage").await;
            assert_eq!(
                status, 200,
                "case {case:?}: UntagResource must answer 200, got {status} {body}"
            );

            let (status, listed) = list_tags(&fixture.router, instance_arn, resource_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListTagsForResource must answer 200, got {status} {listed}"
            );
            assert_eq!(
                reported(&listed),
                seed_pairs(),
                "case {case:?}: the removal must leave the seeded set, got {listed}"
            );
        }
    }
}

/// Put the account's instance back into the window its `CreateInstance` opens: the
/// `CREATE_IN_PROGRESS` status the record carries, with the activation the clock still owes
/// held open at `due_at` so both sides of that instant can be observed without the test
/// sleeping.
///
/// The resources under test are created while the instance is ACTIVE, because a create is
/// itself one of the calls that window withholds; moving the stored status and the stored
/// instant is the record's and the clock's business — exactly as
/// `instance_create::hold_activation` moves an activation instant — and the transition out
/// of the window is still applied by the service's own `materialize_due`. Everything
/// asserted afterwards goes over the wire.
fn hold_creation_window(engine: &Arc<engine::InMemoryEngine>, due_at: SystemTime) {
    with_store(engine, |store| {
        for instance in store.instances.values_mut() {
            instance.status = state::STATUS_CREATE_IN_PROGRESS.to_string();
            instance.active_due = Some(due_at);
        }
    });
}

/// Step 3, which only the two WRITES take: while the instance holding the resource is
/// still being created, both writes are refused with the modelled `ConflictException` and
/// store nothing, while the REPORT of the same resource's tags is served — and once the
/// activation falls due the same writes are served.
#[tokio::test]
async fn only_the_writes_withhold_a_resource_whose_instance_is_still_being_created() {
    let fixture = fixture("creating").await;
    hold_creation_window(&fixture.engine, SystemTime::now() + Duration::from_secs(3_600));
    let conflict = state::instance_creation_in_progress_message(&fixture.instance_arn);

    for (kind, resource_arn, _) in &fixture.resources {
        let case = format!("{kind} held under an instance still being created");

        let (status, body) = tag_resource(
            &fixture.router,
            Some(&fixture.instance_arn),
            resource_arn,
            ("stage", "beta"),
        )
        .await;
        assert_refusal(
            &case,
            "TagResource",
            "ConflictException",
            &conflict,
            status,
            &body,
        );

        let (status, body) = untag_resource(
            &fixture.router,
            Some(&fixture.instance_arn),
            resource_arn,
            SEED_TAG.0,
        )
        .await;
        assert_refusal(
            &case,
            "UntagResource",
            "ConflictException",
            &conflict,
            status,
            &body,
        );

        // The report is served throughout, and it shows both writes stored nothing.
        let (status, listed) =
            list_tags(&fixture.router, Some(&fixture.instance_arn), resource_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must still answer 200, got {status} {listed}"
        );
        assert_eq!(
            reported(&listed),
            seed_pairs(),
            "case {case:?}: a refused write must leave the stored set alone, got {listed}"
        );
    }

    // Once the clock makes the activation due, the SAME writes are served: the refusals
    // above came from the instance's status and from nothing else.
    hold_activation(&fixture.engine, SystemTime::now() - Duration::from_secs(60));
    for (kind, resource_arn, _) in &fixture.resources {
        let case = format!("{kind} once the activation has fallen due");

        let (status, body) = tag_resource(
            &fixture.router,
            Some(&fixture.instance_arn),
            resource_arn,
            ("stage", "beta"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource must be served, got {status} {body}"
        );

        let (status, body) = untag_resource(
            &fixture.router,
            Some(&fixture.instance_arn),
            resource_arn,
            "stage",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource must be served, got {status} {body}"
        );

        let (status, listed) =
            list_tags(&fixture.router, Some(&fixture.instance_arn), resource_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must answer 200, got {status} {listed}"
        );
        assert_eq!(
            reported(&listed),
            seed_pairs(),
            "case {case:?}: the served write and removal must leave the seeded set, got {listed}"
        );
    }
}
