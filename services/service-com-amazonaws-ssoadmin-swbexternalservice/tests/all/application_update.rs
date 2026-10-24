// SPDX-License-Identifier: CC0-1.0

//! `UpdateApplication` end to end over the `awsJson1_1` wire — the operation the stub
//! issue reported as an unimplemented scaffold.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over a single
//! example:
//!
//! 1. an update is PARTIAL: every optional member the request CARRIES replaces that
//!    property of the application the `ApplicationArn` names, and every member the
//!    request OMITS leaves the stored value exactly as it was — asserted by comparing
//!    the WHOLE `DescribeApplication` body before and after against the stored body
//!    overlaid with just the members sent, over a cross product of created applications
//!    and update requests;
//! 2. `UpdateApplicationPortalOptions` models `SignInOptions` ONLY
//!    (`smithy_query(query="shape",shape="UpdateApplicationPortalOptions")`), so an
//!    update of the portal options replaces the sign-in options and leaves the stored
//!    `Visibility` alone — this operation cannot change visibility at all;
//! 3. an update touches ONE application: every other stored application still reports
//!    exactly what it reported before, and the updated application keeps the identity its
//!    create decided (`ApplicationArn`, `InstanceArn`, `IdentityStoreArn`,
//!    `ApplicationProviderArn`, `ApplicationAccount`, `CreatedDate`, `CreatedFrom`);
//! 4. the update is repeatable: sending the same request again answers the same way and
//!    leaves the same state, so a client retry is safe;
//! 5. an update that cannot be served is refused with the modelled CLIENT error for the
//!    reason it cannot be served — never the scaffold's HTTP 501, never a 200 — and
//!    writes NOTHING.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    assert_refused, call, router, router_over, seeded_instance_arn, with_store,
};

/// The one published provider an application can be CREATED under: `CreateApplication`
/// makes an OAuth 2.0 CUSTOMER MANAGED application, which is what this provider backs.
///
/// SOURCE: `state::BuiltInApplicationProvider::accepts_customer_managed_application` over
/// `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue `ListApplicationProviders`
/// publishes over the wire.
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// A pattern-valid `ApplicationArn` no `CreateApplication` ever minted, used to pin the
/// not-found rule.
///
/// SOURCE: `smithy_query(query="shape",shape="ApplicationArn")` —
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`.
const UNKNOWN_APPLICATION_ARN: &str =
    "arn:aws:sso::123456789012:application/ssoins-0000000000000000/apl-0000000000000000";

/// Another unminted ARN, in a non-`aws` partition the same pattern admits
/// (`arn:aws(-[a-z]{1,5}){0,3}`), so the refusal is not keyed to one partition.
const UNKNOWN_APPLICATION_ARN_GOV: &str =
    "arn:aws-us-gov:sso::210987654321:application/ins-abcdefghij012345/apl-ZYXWVUTSRQ987654";

/// One `CreateApplication` request the update rules are asserted against: the shapes of
/// STORED application an update has to start from.
///
/// They differ along what an update can touch — whether a `Description` was ever stored,
/// which `ApplicationStatus` the application carries (`ENABLED | DISABLED`, from
/// `smithy_query(query="shape",shape="ApplicationStatus")`), which `SignInOrigin` its
/// portal options carry (or none at all) and which `ApplicationVisibility` they carry.
#[derive(Debug, Clone)]
struct Base {
    label: &'static str,
    description: Option<&'static str>,
    status: Option<&'static str>,
    portal_options: Option<Value>,
}

impl Base {
    fn request(&self, instance_arn: &str, name: &str) -> Value {
        let mut body = json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
            "Name": name,
        });
        if let Some(description) = self.description {
            body["Description"] = json!(description);
        }
        if let Some(status) = self.status {
            body["Status"] = json!(status);
        }
        if let Some(portal_options) = &self.portal_options {
            body["PortalOptions"] = portal_options.clone();
        }
        body
    }
}

/// The created applications the updates are applied to.
fn bases() -> Vec<Base> {
    vec![
        Base {
            label: "only the required members",
            description: None,
            status: None,
            portal_options: None,
        },
        Base {
            label: "a DISABLED application hidden from the portal, signing in at its own URL",
            description: Some("disabled, hidden from the portal, signing in at its own URL"),
            status: Some("DISABLED"),
            portal_options: Some(json!({
                "SignInOptions": {
                    "Origin": "APPLICATION",
                    "ApplicationUrl": "https://lake.example.com/sso/start?tenant=one",
                },
                "Visibility": "DISABLED",
            })),
        },
        Base {
            label: "an ENABLED application signing in through Identity Center",
            description: Some("d"),
            status: Some("ENABLED"),
            portal_options: Some(json!({
                "SignInOptions": {"Origin": "IDENTITY_CENTER"},
                "Visibility": "ENABLED",
            })),
        },
    ]
}

/// One `UpdateApplication` request: a row of the table.
///
/// The rows differ along which SUBSET of the operation's optional members the request
/// carries (none, each one alone, all of them) and along the values those members take —
/// `Name` at both published length bounds (`len 1..100`, `^[\S\s]*$`, so spaces and
/// punctuation are admitted), `Description` at both of its (`len 1..128`), each
/// `ApplicationStatus` and each `SignInOrigin`. Every constraint is from
/// `smithy_query(query="operation",operation="UpdateApplication")` and
/// `smithy_query(query="shape",shape="UpdateApplicationPortalOptions,SignInOptions,ApplicationStatus")`.
fn updates() -> Vec<(&'static str, Value)> {
    vec![
        ("no updatable member at all", json!({})),
        (
            "Name alone, at its lower length bound",
            json!({"Name": "a"}),
        ),
        (
            "Name alone, at its upper length bound, with spaces and punctuation",
            json!({"Name": format!("Data lake (prod) - {}", "x".repeat(81))}),
        ),
        (
            "Description alone, at its lower length bound",
            json!({"Description": "d"}),
        ),
        (
            "Description alone, at its upper length bound",
            json!({"Description": "d".repeat(128)}),
        ),
        ("Status alone, DISABLED", json!({"Status": "DISABLED"})),
        ("Status alone, ENABLED", json!({"Status": "ENABLED"})),
        (
            "portal options alone, the IDENTITY_CENTER origin",
            json!({"PortalOptions": {"SignInOptions": {"Origin": "IDENTITY_CENTER"}}}),
        ),
        (
            "portal options alone, the APPLICATION origin and its URL",
            json!({
                "PortalOptions": {
                    "SignInOptions": {
                        "Origin": "APPLICATION",
                        "ApplicationUrl": "https://updated.example.com/start",
                    },
                },
            }),
        ),
        (
            "portal options carrying no sign-in options",
            json!({"PortalOptions": {}}),
        ),
        (
            "every updatable member at once",
            json!({
                "Name": "renamed and reconfigured",
                "Description": "every member of the update sent at once",
                "Status": "DISABLED",
                "PortalOptions": {
                    "SignInOptions": {
                        "Origin": "APPLICATION",
                        "ApplicationUrl": "https://all.example.com/sso?x=1",
                    },
                },
            }),
        ),
    ]
}

/// The `DescribeApplication` body an application must answer with AFTER `update` is
/// applied to the application that answered `before`: rule 1 and rule 2 as a function.
///
/// Only the members `update` CARRIES are overlaid; `Visibility` is carried over from
/// `before` because `UpdateApplicationPortalOptions` has no member that could change it.
fn expected_after(before: &Value, update: &Value) -> Value {
    let mut expected = before.clone();
    for member in ["Name", "Description", "Status"] {
        if let Some(value) = update.get(member) {
            expected[member] = value.clone();
        }
    }
    if let Some(sign_in_options) = update
        .get("PortalOptions")
        .and_then(|options| options.get("SignInOptions"))
    {
        expected["PortalOptions"]["SignInOptions"] = sign_in_options.clone();
    }
    expected
}

/// `DescribeApplication` for `arn`.
async fn describe(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(
        router,
        "DescribeApplication",
        &json!({"ApplicationArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeApplication of {arn} expected 200, got {status} {body}"
    );
    body
}

/// `CreateApplication` for `base` under `instance_arn`, returning the minted ARN.
async fn create(router: &MultiServiceRouter, instance_arn: &str, base: &Base, name: &str) -> String {
    let (status, created) = call(router, "CreateApplication", &base.request(instance_arn, name)).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateApplication must answer 200, got {status} {created}",
        base.label
    );
    created["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: no ApplicationArn in {created}", base.label))
        .to_string()
}

/// `UpdateApplication` of `arn` with `update`, asserting the modelled EMPTY response.
async fn update_application(router: &MultiServiceRouter, case: &str, arn: &str, update: &Value) {
    let mut request = update.clone();
    request["ApplicationArn"] = json!(arn);
    let (status, body) = call(router, "UpdateApplication", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateApplication must answer 200, got {status} {body}"
    );
    // `UpdateApplicationResponse` models no members, so the answer carries none.
    assert!(
        body.as_object().is_none_or(|members| members.is_empty()),
        "case {case:?}: UpdateApplication models an empty response, got {body}"
    );
}

/// Rules 1, 2 and 3: every row writes exactly the members it carries, leaves the rest of
/// the record alone, and disturbs no other application.
#[tokio::test]
async fn an_update_writes_only_the_members_the_request_carries() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // One application that is never updated, so "an update touches one application" is a
    // claim about a store that holds others.
    let bystander = create(
        &router,
        &instance_arn,
        &bases()[1],
        "the application nothing updates",
    )
    .await;
    let bystander_before = describe(&router, &bystander).await;

    for (base_index, base) in bases().into_iter().enumerate() {
        for (update_index, (label, update)) in updates().into_iter().enumerate() {
            let case = format!("{}: {label}", base.label);
            // `ApplicationNameType` is `len 1..100`, so the name is an index rather than
            // the (much longer) case label.
            let name = format!("app-{base_index}-{update_index}");
            let arn = create(&router, &instance_arn, &base, &name).await;
            let before = describe(&router, &arn).await;

            update_application(&router, &case, &arn, &update).await;

            let after = describe(&router, &arn).await;
            assert_eq!(
                after,
                expected_after(&before, &update),
                "case {case:?}: the update must write the members it carries and leave every \
                 other member of {before} alone, got {after}"
            );

            // Rule 2, stated on its own: whatever the update did, the stored visibility is
            // the one the create decided.
            assert_eq!(
                after["PortalOptions"]["Visibility"], before["PortalOptions"]["Visibility"],
                "case {case:?}: UpdateApplication carries no Visibility member, so the stored \
                 one must survive, got {after}"
            );

            // Rule 3: the identity the create decided is not an update's to change.
            for member in [
                "ApplicationArn",
                "InstanceArn",
                "IdentityStoreArn",
                "ApplicationProviderArn",
                "ApplicationAccount",
                "CreatedDate",
                "CreatedFrom",
            ] {
                assert_eq!(
                    after[member], before[member],
                    "case {case:?}: {member} belongs to the create, so the update must leave \
                     it as {:?}, got {after}",
                    before[member]
                );
            }

            // Rule 3: nothing else moved.
            let bystander_after = describe(&router, &bystander).await;
            assert_eq!(
                bystander_after, bystander_before,
                "case {case:?}: updating {arn} must leave every other application as it was, \
                 got {bystander_after}"
            );
        }
    }
}

/// Rule 4: the same update sent twice answers the same way and leaves the same state.
#[tokio::test]
async fn repeating_an_update_changes_nothing_further() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for (base_index, base) in bases().into_iter().enumerate() {
        for (update_index, (label, update)) in updates().into_iter().enumerate() {
            let case = format!("{}: {label} twice", base.label);
            let name = format!("twice-{base_index}-{update_index}");
            let arn = create(&router, &instance_arn, &base, &name).await;

            update_application(&router, &case, &arn, &update).await;
            let once = describe(&router, &arn).await;
            update_application(&router, &case, &arn, &update).await;
            let twice = describe(&router, &arn).await;

            assert_eq!(
                twice, once,
                "case {case:?}: sending the same update again must leave the record as the \
                 first one left it ({once}), got {twice}"
            );
        }
    }
}

/// Rule 5: an update that cannot be served is refused and writes nothing.
#[tokio::test]
async fn a_refused_update_changes_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;

    let arn = create(
        &router,
        &instance_arn,
        &bases()[1],
        "the application a refused update must not touch",
    )
    .await;
    let before = describe(&router, &arn).await;
    let stored_before = with_store(&engine, |store| {
        store.applications.keys().cloned().collect::<Vec<String>>()
    });

    // (case, the request, the modelled error it is refused with) — the rows span the
    // reasons an update whose members all satisfy the model cannot be served.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "an ApplicationArn this account holds no application for",
            json!({"ApplicationArn": UNKNOWN_APPLICATION_ARN, "Name": "unheld"}),
            "ResourceNotFoundException",
        ),
        (
            "an unminted ApplicationArn in another partition",
            json!({
                "ApplicationArn": UNKNOWN_APPLICATION_ARN_GOV,
                "Description": "an application that does not exist",
                "Status": "DISABLED",
            }),
            "ResourceNotFoundException",
        ),
        (
            "an unminted ApplicationArn carrying no other member",
            json!({"ApplicationArn": UNKNOWN_APPLICATION_ARN}),
            "ResourceNotFoundException",
        ),
        (
            "an APPLICATION sign-in origin with no ApplicationUrl",
            json!({
                "ApplicationArn": arn,
                "PortalOptions": {"SignInOptions": {"Origin": "APPLICATION"}},
            }),
            "ValidationException",
        ),
        (
            "an APPLICATION sign-in origin on an otherwise complete update",
            json!({
                "ApplicationArn": arn,
                "Name": "would-be-renamed",
                "Description": "a complete update but for the sign-in URL",
                "Status": "ENABLED",
                "PortalOptions": {"SignInOptions": {"Origin": "APPLICATION"}},
            }),
            "ValidationException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = call(&router, "UpdateApplication", &request).await;
        // 400 carrying the modelled code, so neither a 200 nor the scaffold's 501.
        assert_refused(case, "UpdateApplication", expected, status, &body);

        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update writes nothing, got {after}"
        );
        let stored_after = with_store(&engine, |store| {
            store.applications.keys().cloned().collect::<Vec<String>>()
        });
        assert_eq!(
            stored_after, stored_before,
            "case {case:?}: a refused update neither adds nor removes an application, the \
             store now holds {stored_after:?}"
        );
    }
}
