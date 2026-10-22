// SPDX-License-Identifier: CC0-1.0

//! This service's ERROR CONTRACT, over the wire: which class an unusable value falls
//! into, and how the resulting message is worded.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a role ARN that names an account other than the one the emulator serves is an
//!   AUTHORIZATION refusal (`AccessDeniedException`, with the pass-role message), not
//!   a malformed-input refusal — Elemental Inference does not resolve a passed role
//!   offline, it evaluates it by attempting to pass it, so that decision is reached
//!   before the member's own syntax rules;
//! * `ResourceNotFoundException` carries ONE fixed message, the same for every
//!   operation and every id, and never renders the id that was asked for;
//! * a constraint-violation message renders a `@pattern` WITHOUT its anchors, for
//!   every anchored pattern member of this service.

use crate::harness::{call, create_feed, minimal_feed, output_config, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    CROSS_ACCOUNT_PASS_ROLE_MESSAGE, NOT_FOUND_MESSAGE,
};

/// A role ARN in the account this emulator serves — the only account whose roles it
/// may pass.
fn own_account_role(name: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One row of the pass-role table: a role ARN, and whether its syntax also satisfies
/// `IamRoleArn`'s modelled `@length`/`@pattern` (which decides whether the decision is
/// taken in the handler, with the modelled HTTP 403, or at the request boundary, which
/// the framework pins at HTTP 400 for every pre-handler refusal).
struct RoleCase {
    label: &'static str,
    arn: String,
    well_formed: bool,
}

/// Role ARNs that name an account other than `engine::EMULATOR_ACCOUNT_ID`, differing
/// along what the rule generalises over: the partition, the account id, the role path,
/// and whether the rest of the ARN is well formed at all.
fn cross_account_roles() -> Vec<RoleCase> {
    vec![
        RoleCase {
            label: "a well-formed role in another account",
            arn: "arn:aws:iam::123456789012:role/elemental-inference".to_string(),
            well_formed: true,
        },
        RoleCase {
            label: "another partition, another account, a role path",
            arn: "arn:aws-us-gov:iam::210987654321:role/service-role/ei-feed".to_string(),
            well_formed: true,
        },
        RoleCase {
            label: "another account, but shorter than the member's @length allows",
            arn: "arn:aws:iam::999999999999:role".to_string(),
            // 30 characters, and no `role/` resource: inside `IamRoleArn`'s @length
            // min of 32 and outside its @pattern, so the boundary sees it first.
            well_formed: false,
        },
        RoleCase {
            label: "another account, and no `role/` resource at all",
            arn: "arn:aws:iam::123456789012:notarole".to_string(),
            well_formed: false,
        },
    ]
}

/// A cross-account `accessRoleArn` is denied as a pass-role refusal, wherever the
/// member appears.
#[tokio::test]
async fn a_role_in_another_account_is_denied_rather_than_called_malformed() {
    let router = router();
    // One live feed, so UpdateFeed's refusal is about the role and not about the id.
    let feed_id = create_feed(&router, "pass-role-target").await;

    for case in cross_account_roles() {
        let create = json!({
            "name": "cross-account-feed",
            "accessRoleArn": case.arn,
            "outputs": [
                {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
            ]
        });
        let update = json!({
            "name": "cross-account-feed",
            "accessRoleArn": case.arn,
            "outputs": [
                {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
            ]
        });
        for (method, uri, body) in [
            ("POST", "/v1/feed".to_string(), create),
            ("PUT", format!("/v1/feed/{feed_id}"), update),
        ] {
            let label = format!("{} on {method} {uri}", case.label);
            let (status, error) = call(&router, method, &uri, Some(body)).await;
            assert_eq!(
                error["__type"],
                json!("AccessDeniedException"),
                "case {label:?}: passing a role from another account is denied, got {status} \
                 with {error}"
            );
            assert_eq!(
                error["message"],
                json!(CROSS_ACCOUNT_PASS_ROLE_MESSAGE),
                "case {label:?}: the denial says why, got {error}"
            );
            // The modelled status for `AccessDeniedException` is 403, and the handler
            // answers exactly that wherever it is the handler that decides.
            //
            // A value the model's own `@pattern`/`@length` already refuses never
            // reaches a handler: the framework decides a pre-handler refusal's status
            // from the protocol (`pre_dispatch::json`) and lets a service choose only
            // the code and the message, so this emulator cannot answer 403 there. That
            // is a KNOWN DIVERGENCE, recorded in this issue's report — not the
            // behaviour being pinned here, so this row asserts only that the refusal is
            // the caller's fault and carries the pass-role classification above.
            if case.well_formed {
                assert_eq!(
                    status, 403,
                    "case {label:?}: the modelled AccessDeniedException is a 403, got \
                     {status} with {error}"
                );
            } else {
                assert!(
                    (400..500).contains(&status),
                    "case {label:?}: a denied pass role is the caller's fault, got \
                     {status} with {error}"
                );
            }
        }
    }
}

/// A role in the emulator's own account is passed, so the feed is created with it.
#[tokio::test]
async fn a_role_in_this_account_is_passed() {
    let router = router();
    let roles = [
        own_account_role("elemental-inference-feed"),
        own_account_role("service-role/ei"),
        own_account_role("a.role_with-every+allowed=char,@-"),
    ];

    for role in roles {
        let (status, created) = call(
            &router,
            "POST",
            "/v1/feed",
            Some(json!({
                "name": "own-account-feed",
                "accessRoleArn": role,
                "outputs": [
                    {"name": "crop1", "outputConfig": output_config("cropping"),
                     "status": "ENABLED"}
                ]
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {role:?}: a role in this account is accepted, got {status} with {created}"
        );
        assert_eq!(
            created["accessRoleArn"],
            json!(role),
            "case {role:?}: the stored feed reports the role it was given, got {created}"
        );
    }

    // And a feed with no access role at all is still accepted: the member is optional.
    let (status, created) = call(
        &router,
        "POST",
        "/v1/feed",
        Some(minimal_feed("no-role-feed")),
    )
    .await;
    assert_eq!(
        status, 202,
        "a feed with no accessRoleArn is accepted, got {status} with {created}"
    );
    assert_eq!(
        created["accessRoleArn"],
        Value::Null,
        "a feed with no accessRoleArn reports none, got {created}"
    );
}

/// Every operation that takes an id answers the SAME not-found message, for every id.
#[tokio::test]
async fn resource_not_found_carries_one_fixed_message_for_every_operation_and_id() {
    let router = router();
    // One live feed, so each 404 is about the id and not about an empty store.
    let live_id = create_feed(&router, "the-only-feed").await;

    // Every id below satisfies ^[a-z0-9]{19}$, so the boundary admits it and the
    // handler is what refuses it. They differ in length of digit runs and alphabet so
    // no two renderings could coincide.
    let unknown_ids = [
        "0000000000000000000",
        "zzzzzzzzzzzzzzzzzzz",
        "abcdefghij123456789",
        // Deliberately NOT the first id this service mints into a fresh store: these
        // ids must name no live feed.
        "a0000000000000000zz",
    ];
    let update = json!({
        "name": "whatever",
        "outputs": [
            {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
        ]
    });
    let claim = json!({
        "associatedResourceName": "some-source-media",
        "dryRun": false,
        "outputs": []
    });
    let release = json!({"associatedResourceName": "some-source-media", "dryRun": false});

    for id in unknown_ids {
        assert_ne!(id, live_id, "the unknown ids must not be the live feed's");
        for (method, path, body) in [
            ("GET", String::new(), None),
            ("PUT", String::new(), Some(update.clone())),
            ("DELETE", String::new(), None),
            ("POST", "/associate".to_string(), Some(claim.clone())),
            ("POST", "/disassociate".to_string(), Some(release.clone())),
        ] {
            let uri = format!("/v1/feed/{id}{path}");
            let label = format!("{method} {uri}");
            let (status, error) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 404,
                "case {label:?}: an unknown feed id is a 404, got {status} with {error}"
            );
            assert_eq!(
                error["__type"],
                json!("ResourceNotFoundException"),
                "case {label:?}: that 404 is a ResourceNotFoundException, got {error}"
            );
            assert_eq!(
                error["message"],
                json!(NOT_FOUND_MESSAGE),
                "case {label:?}: the not-found message is fixed and names no id, got {error}"
            );
        }
    }
}

/// A feed that has been deleted is not found either — with the same fixed message, and
/// without echoing the id the caller probed.
#[tokio::test]
async fn a_deleted_feed_is_not_found_with_the_same_message() {
    let router = router();
    let id = create_feed(&router, "to-be-deleted").await;
    let (status, _) = call(&router, "DELETE", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(status, 202, "DeleteFeed is accepted, got {status}");
    crate::harness::past_the_transition_span();

    let (status, error) = call(&router, "GET", &format!("/v1/feed/{id}"), None).await;
    assert_eq!(status, 404, "a deleted feed is gone, got {status} with {error}");
    assert_eq!(
        error["message"],
        json!(NOT_FOUND_MESSAGE),
        "the message does not render the deleted feed's id, got {error}"
    );
    assert!(
        !error["message"].as_str().unwrap().contains(&id),
        "the message must not leak the probed id {id}, got {error}"
    );
}

/// One row of the pattern table: a request that violates exactly one anchored
/// `@pattern` member, and the expression that member's refusal must name.
///
/// Every `pattern` below is the member's modelled `@pattern` with its `^`/`$` anchors
/// removed; the modelled values come from
/// `smithy_query(query="operation",operation="CreateFeed,CreateDictionary,AssociateFeed,SearchFixtures")`:
/// `ResourceName` is `^[a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?$`, `FeedId` is
/// `^[a-z0-9]{19}$`, `AssociatedResourceName` is `^[\w \-\.',@:;/]*$` and `FixtureDate`
/// is `^\d{4}-\d{2}-\d{2}$`.
struct PatternCase {
    label: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Option<Value>,
    member: &'static str,
    value: &'static str,
    pattern: &'static str,
}

fn pattern_cases() -> Vec<PatternCase> {
    const RESOURCE_NAME: &str = "[a-zA-Z0-9]([a-zA-Z0-9-_]{0,126}[a-zA-Z0-9])?";
    vec![
        PatternCase {
            label: "CreateDictionary.name",
            method: "POST",
            uri: "/v1/dictionary",
            body: Some(json!({
                "name": "my-dictionary bad!",
                "language": "eng",
                "entries": "[{\"content\": \"test\"}]"
            })),
            member: "name",
            value: "my-dictionary bad!",
            pattern: RESOURCE_NAME,
        },
        PatternCase {
            label: "CreateFeed.name",
            method: "POST",
            uri: "/v1/feed",
            body: Some(json!({
                "name": "-starts-with-a-dash",
                "outputs": []
            })),
            member: "name",
            value: "-starts-with-a-dash",
            pattern: RESOURCE_NAME,
        },
        PatternCase {
            label: "AssociateFeed.id, bound to the URI label",
            method: "POST",
            uri: "/v1/feed/NOT-A-FEED-ID/associate",
            body: Some(json!({
                "associatedResourceName": "source-media",
                "dryRun": false,
                "outputs": []
            })),
            member: "id",
            value: "NOT-A-FEED-ID",
            pattern: "[a-z0-9]{19}",
        },
        PatternCase {
            label: "DisassociateFeed.associatedResourceName",
            method: "POST",
            uri: "/v1/feed/0000000000000000000/disassociate",
            body: Some(json!({"associatedResourceName": "star*media", "dryRun": false})),
            member: "associatedResourceName",
            value: "star*media",
            pattern: r"[\w \-\.',@:;/]*",
        },
        PatternCase {
            label: "SearchFixtures.startDate",
            method: "POST",
            uri: "/v1/fixtures",
            body: Some(json!({"sport": "basketball", "startDate": "2026-3-14"})),
            member: "startDate",
            value: "2026-3-14",
            pattern: r"\d{4}-\d{2}-\d{2}",
        },
    ]
}

/// A constraint-violation message renders the pattern alone, with no `^` or `$`.
#[tokio::test]
async fn a_pattern_violation_renders_the_expression_without_its_anchors() {
    let router = router();

    for case in pattern_cases() {
        let label = case.label;
        let (status, error) = call(&router, case.method, case.uri, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a constraint violation is a 400, got {status} with {error}"
        );
        assert_eq!(
            error["__type"],
            json!("ValidationException"),
            "case {label:?}: that 400 is a ValidationException, got {error}"
        );
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {}",
            case.value, case.member, case.pattern
        );
        assert_eq!(
            error["message"],
            json!(expected),
            "case {label:?}: expected {expected:?}, got {error}"
        );
    }
}
