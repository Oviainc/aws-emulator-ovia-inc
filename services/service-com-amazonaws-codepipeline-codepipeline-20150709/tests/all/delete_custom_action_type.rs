// SPDX-License-Identifier: CC0-1.0

//! `DeleteCustomActionType`, end to end over the wire the service actually speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test: a delete drops the `Custom`-owned action type whose
//! `(category, provider, version)` the request names, as registered in the Region the
//! call was signed for, and nothing else — so every later read of that resource
//! (`ListActionTypes`, `PollForJobs`, `ListTagsForResource`, `GetActionType`) answers
//! as it does for an action type the account never held, while every other
//! registration is untouched; and a delete of an identity the account holds no
//! registration of in that Region succeeds having changed nothing.
//!
//! Each row of the table differs along what the rule generalises over: the category,
//! the provider, the version (including one at `Version`'s 9-character maximum), how
//! much the registration carried (settings, configuration properties, tags, artifact
//! bounds) and the Region it was made in. Every assertion goes through the router, so
//! nothing here can pass by reading a handler's internals.

use serde_json::{json, Value};

use crate::action_type_definitions::identifier;
use crate::custom_action_types::{identity, listed_in, CUSTOM};
use crate::pipeline_lifecycle::{Emulator, ACCOUNT_ID};

/// The Region `Emulator::call` signs for.
const HOME_REGION: &str = "us-east-1";

/// Every `ActionCategory` the model declares
/// (`smithy_query(query="shape",shape="ActionCategory")` → these seven values).
const ACTION_CATEGORIES: [&str; 7] = [
    "Source", "Build", "Deploy", "Test", "Invoke", "Approval", "Compute",
];

/// One custom action type to register and then delete.
struct Row {
    case: &'static str,
    category: &'static str,
    provider: &'static str,
    version: &'static str,
    /// The members the create carries beyond the three identifying ones.
    extra: Value,
}

impl Row {
    /// The `(category, provider, version)` this row is listed under.
    fn identity(&self) -> String {
        format!("{}/{}/{}", self.category, self.provider, self.version)
    }

    /// This row's `CreateCustomActionType` request.
    fn create(&self) -> Value {
        let mut body = json!({
            "category": self.category,
            "provider": self.provider,
            "version": self.version,
            "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
        });
        for (member, value) in self.extra.as_object().expect("an object of members") {
            body[member] = value.clone();
        }
        body
    }

    /// This row's `DeleteCustomActionType` request: the three members the operation
    /// takes, and no owner — the owner of a custom action is always `Custom`.
    fn delete(&self) -> Value {
        json!({
            "category": self.category,
            "provider": self.provider,
            "version": self.version,
        })
    }
}

/// The table: one row per shape of registration the rule has to hold for.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "the smallest registration: required members only",
            category: "Build",
            provider: "Minimal",
            version: "1",
            extra: json!({}),
        },
        Row {
            case: "artifact counts at the modelled maximum of 5, version at its \
                   9-character maximum",
            category: "Deploy",
            provider: "ArtifactBounds",
            version: "9chars_v9",
            extra: json!({
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 5},
                "outputArtifactDetails": {"minimumCount": 5, "maximumCount": 5},
            }),
        },
        Row {
            case: "settings and configuration properties",
            category: "Invoke",
            provider: "Full-Provider_1",
            version: "v2",
            extra: json!({
                "settings": {
                    "entityUrlTemplate": "https://example.test/entity/{Config:needle}",
                    "executionUrlTemplate": "https://example.test/execution",
                },
                "configurationProperties": [{
                    "name": "needle",
                    "required": true,
                    "key": true,
                    "secret": false,
                    "queryable": false,
                    "type": "String",
                }],
            }),
        },
        Row {
            case: "tags, which go with the registration",
            category: "Test",
            provider: "Tagged-Tool",
            version: "1",
            extra: json!({"tags": [{"key": "team", "value": "build"}]}),
        },
    ];
    // The rule holds for every category the model declares, not only the four above.
    rows.extend(ACTION_CATEGORIES.iter().map(|category| Row {
        case: "one registration per declared category",
        category,
        provider: "Per-Category",
        version: "1",
        extra: json!({}),
    }));
    rows
}

/// The `(category, provider, version)` of every `Custom`-owned action type the
/// account holds in `region`.
async fn held_in(emulator: &Emulator, region: &str) -> Vec<String> {
    listed_in(emulator, CUSTOM, region)
        .await
        .iter()
        .map(identity)
        .collect()
}

/// Delete `body` signed for `region`, asserting the operation's modelled success: a
/// 200 whose document carries no error and no member.
async fn delete_in(emulator: &Emulator, region: &str, case: &str, body: &Value) {
    let (status, answer) = emulator
        .call_in(region, "DeleteCustomActionType", body)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteCustomActionType in {region} to answer HTTP 200, \
         got {status} with {answer}"
    );
    assert!(
        answer.get("__type").is_none(),
        "case {case:?}: expected a success document, got {answer}"
    );
    assert!(
        answer.as_object().is_none_or(serde_json::Map::is_empty),
        "case {case:?}: DeleteCustomActionType models no output member, got {answer}"
    );
}

/// RULE: a delete drops exactly the registration whose `(category, provider,
/// version)` it names, so the listing loses that one and keeps every other.
#[tokio::test]
async fn a_delete_drops_the_registration_it_names_and_no_other() {
    let emulator = Emulator::new();
    let rows = rows();

    for row in &rows {
        let answer = emulator.ok("CreateCustomActionType", &row.create()).await;
        assert_eq!(
            identity(&answer["actionType"]),
            row.identity(),
            "case {:?}: expected the create to answer the identity it named, got {answer}",
            row.case
        );
    }
    let held = held_in(&emulator, HOME_REGION).await;
    assert_eq!(
        held.len(),
        rows.len(),
        "expected every created action type to be held, got {held:?}"
    );

    // Deleted in the reverse of the order they were created in: a delete is selected
    // by the identity it names, not by how recent a registration is.
    let mut remaining: Vec<String> = rows.iter().map(Row::identity).collect();
    for row in rows.iter().rev() {
        delete_in(&emulator, HOME_REGION, row.case, &row.delete()).await;
        remaining.retain(|held| held != &row.identity());

        let held = held_in(&emulator, HOME_REGION).await;
        assert!(
            !held.contains(&row.identity()),
            "case {:?}: expected {} to be gone from the listing, got {held:?}",
            row.case,
            row.identity()
        );
        for kept in &remaining {
            assert!(
                held.contains(kept),
                "case {:?}: expected {kept} to be untouched by the delete, got {held:?}",
                row.case
            );
        }
        assert_eq!(
            held.len(),
            remaining.len(),
            "case {:?}: expected {} registrations left, got {held:?}",
            row.case,
            remaining.len()
        );
    }
}

/// RULE: the identity is `(category, provider, version)` together, so a delete naming
/// an identity that differs in any one of the three is a different resource.
#[tokio::test]
async fn a_delete_matches_on_the_category_provider_and_version_together() {
    let emulator = Emulator::new();

    let row = |category: &'static str, provider: &'static str, version: &'static str| Row {
        case: "one of four identities differing in one member",
        category,
        provider,
        version,
        extra: json!({}),
    };
    // One base identity, and one differing in each single component.
    let base = row("Build", "Prov", "1");
    let others = [
        row("Test", "Prov", "1"),
        row("Build", "Other", "1"),
        row("Build", "Prov", "2"),
    ];
    for created in std::iter::once(&base).chain(others.iter()) {
        emulator.ok("CreateCustomActionType", &created.create()).await;
    }

    delete_in(&emulator, HOME_REGION, "the base identity", &base.delete()).await;

    let held = held_in(&emulator, HOME_REGION).await;
    assert!(
        !held.contains(&base.identity()),
        "expected {} to be deleted, got {held:?}",
        base.identity()
    );
    for kept in &others {
        assert!(
            held.contains(&kept.identity()),
            "expected {} — differing from the deleted identity in one member — to be \
             untouched, got {held:?}",
            kept.identity()
        );
    }
}

/// RULE: a registration belongs to the Region it was made in, so a delete addresses
/// the registration of the Region IT was signed for and leaves another Region's alone.
#[tokio::test]
async fn a_delete_addresses_the_region_it_was_signed_for() {
    let emulator = Emulator::new();

    // Rows differ in the Region the create is signed for, including a Region named
    // nowhere else and two registrations in one Region.
    let registrations = [
        ("us-east-1", "EastOne"),
        ("us-east-1", "EastTwo"),
        ("eu-west-2", "WestOne"),
        ("ap-southeast-4", "SouthOne"),
    ];
    let row = |provider: &'static str| Row {
        case: "a registration of one Region",
        category: "Build",
        provider,
        version: "1",
        extra: json!({}),
    };
    for (region, provider) in registrations {
        let (status, answer) = emulator
            .call_in(region, "CreateCustomActionType", &row(provider).create())
            .await;
        assert_eq!(
            status, 200,
            "creating {provider} in {region} must succeed, got {status} with {answer}"
        );
    }

    // A delete signed for a Region the registration does not belong to deletes
    // nothing, whichever Region it is.
    for (region, provider) in registrations {
        for elsewhere in ["us-west-1", "eu-west-2", "us-east-1"] {
            if elsewhere == region {
                continue;
            }
            let case = format!("{provider} of {region} deleted from {elsewhere}");
            delete_in(&emulator, elsewhere, &case, &row(provider).delete()).await;
            let held = held_in(&emulator, region).await;
            assert!(
                held.contains(&row(provider).identity()),
                "case {case:?}: a delete signed for another Region must delete nothing, \
                 got {held:?} in {region}"
            );
        }
    }

    // And the delete signed for the owning Region does delete it.
    for (region, provider) in registrations {
        let case = format!("{provider} deleted from its own Region {region}");
        delete_in(&emulator, region, &case, &row(provider).delete()).await;
        let held = held_in(&emulator, region).await;
        assert!(
            !held.contains(&row(provider).identity()),
            "case {case:?}: expected the registration to be gone, got {held:?}"
        );
    }
}

/// RULE: everything the service held about the action type goes with it — after the
/// delete, `PollForJobs` for it is not found and its ARN names no taggable resource;
/// re-registering the identity starts from the new request alone.
#[tokio::test]
async fn a_deleted_action_type_is_no_longer_pollable_or_taggable() {
    let emulator = Emulator::new();

    let row = Row {
        case: "an action type read through every operation that addresses one",
        category: "Build",
        provider: "My-Tool",
        version: "1",
        extra: json!({"tags": [{"key": "team", "value": "build"}]}),
    };
    emulator.ok("CreateCustomActionType", &row.create()).await;

    // `GetActionType` is not in this table: a stored definition is only ever reached
    // by an `AWS`- or `ThirdParty`-owned identity (`ActionTypeIdentifier$owner` is
    // `@pattern("AWS|ThirdParty")`), so a custom action type has none to lose.
    let id = identifier(row.category, CUSTOM, row.provider, row.version);

    // The ARN form AWS documents for an action type: the four `ActionTypeId` members,
    // owner first (pinned by `resource_tags.rs`).
    let arn = format!(
        "arn:aws:codepipeline:{HOME_REGION}:{ACCOUNT_ID}:actiontype:{CUSTOM}/{}/{}/{}",
        row.category, row.provider, row.version
    );
    let poll = json!({"actionTypeId": id, "maxBatchSize": 1});

    // Every reader answers for the resource while it is held.
    for (operation, body) in [
        ("PollForJobs", poll.clone()),
        ("ListTagsForResource", json!({"resourceArn": arn})),
    ] {
        let (status, answer) = emulator.call(operation, &body).await;
        assert_eq!(
            status, 200,
            "{operation} must answer for the held action type, got {status} with {answer}"
        );
    }
    let tagged = emulator
        .ok("ListTagsForResource", &json!({"resourceArn": arn}))
        .await;
    assert_eq!(
        tagged["tags"],
        json!([{"key": "team", "value": "build"}]),
        "the create's tags must be readable by the ARN, got {tagged}"
    );

    delete_in(&emulator, HOME_REGION, row.case, &row.delete()).await;

    // And none of them does afterwards, each with the error the operation models for
    // an action type the account does not hold.
    for (operation, body, expected) in [
        ("PollForJobs", poll.clone(), "ActionTypeNotFoundException"),
        (
            "ListTagsForResource",
            json!({"resourceArn": arn}),
            "ResourceNotFoundException",
        ),
    ] {
        let (status, code) = emulator.expect_error(operation, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "{operation} after the delete: expected a 400 {expected}, got {status} {code}"
        );
    }
    let held = held_in(&emulator, HOME_REGION).await;
    assert!(
        held.is_empty(),
        "expected the deleted action type to be gone from the listing, got {held:?}"
    );

    // Re-registering the identity brings back only what the new request carries: the
    // deleted registration's tags and definition are not restored with it.
    let recreated = Row {
        case: "the identity registered again after its delete",
        category: row.category,
        provider: row.provider,
        version: row.version,
        extra: json!({
            "inputArtifactDetails": {"minimumCount": 1, "maximumCount": 4},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 2},
        }),
    };
    emulator
        .ok("CreateCustomActionType", &recreated.create())
        .await;
    let listed = listed_in(&emulator, CUSTOM, HOME_REGION).await;
    assert_eq!(
        listed.len(),
        1,
        "expected the re-registered identity listed exactly once, got {listed:?}"
    );
    assert_eq!(
        listed[0]["inputArtifactDetails"],
        json!({"minimumCount": 1, "maximumCount": 4}),
        "expected the listing to report the new registration, got {}",
        listed[0]
    );
    let tags = emulator
        .ok("ListTagsForResource", &json!({"resourceArn": arn}))
        .await;
    assert!(
        tags.get("tags").is_none_or(|tags| tags == &json!([])),
        "the deleted registration's tags must not come back with it, got {tags}"
    );
    // And it is pollable again, because the registration it needs is back.
    let (status, answer) = emulator.call("PollForJobs", &poll).await;
    assert_eq!(
        status, 200,
        "the re-registered action type must be pollable again, got {status} with {answer}"
    );
}

/// RULE: a delete of an identity the account holds no `Custom` registration of is
/// already in the state it asks for, so it succeeds and changes nothing — the
/// operation models no "not found" error, and the action types the SERVICE provides
/// are not addressable by it at all.
#[tokio::test]
async fn a_delete_of_an_identity_the_account_does_not_hold_changes_nothing() {
    let emulator = Emulator::new();

    let kept = Row {
        case: "the one registration these deletes must not touch",
        category: "Build",
        provider: "Kept",
        version: "1",
        extra: json!({}),
    };
    emulator.ok("CreateCustomActionType", &kept.create()).await;

    // Rows: an identity nothing ever registered; the identity just deleted, deleted
    // again; and the `(category, provider, version)` of three action types the
    // service provides itself — one `AWS`-owned provider published in two categories
    // and one `ThirdParty`-owned (`state::BUILT_IN_ACTION_TYPES`), none of which is
    // `Custom`-owned and so none of which this operation can reach.
    let gone = Row {
        case: "the identity deleted twice",
        category: "Deploy",
        provider: "Twice",
        version: "1",
        extra: json!({}),
    };
    emulator.ok("CreateCustomActionType", &gone.create()).await;
    delete_in(&emulator, HOME_REGION, gone.case, &gone.delete()).await;

    let cases = [
        (
            "an identity nothing ever registered",
            json!({"category": "Approval", "provider": "Never", "version": "1"}),
        ),
        ("the identity deleted twice", gone.delete()),
        (
            "an AWS-owned provider the service publishes",
            json!({"category": "Deploy", "provider": "CodeDeploy", "version": "1"}),
        ),
        (
            "another category of an AWS-owned provider the service publishes",
            json!({"category": "Source", "provider": "S3", "version": "1"}),
        ),
        (
            "a ThirdParty-owned provider the service publishes",
            json!({"category": "Source", "provider": "GitHub", "version": "1"}),
        ),
    ];
    for (case, body) in &cases {
        delete_in(&emulator, HOME_REGION, case, body).await;
    }

    // The account's own registration is still there, and so is every action type the
    // service provides.
    let held = held_in(&emulator, HOME_REGION).await;
    assert_eq!(
        held,
        vec![kept.identity()],
        "expected only the untouched registration, got {held:?}"
    );
    for (owner, expected) in [
        ("AWS", ["Deploy/CodeDeploy/1", "Source/S3/1"].as_slice()),
        ("ThirdParty", ["Source/GitHub/1"].as_slice()),
    ] {
        let provided = listed_in(&emulator, owner, HOME_REGION)
            .await
            .iter()
            .map(identity)
            .collect::<Vec<String>>();
        for wanted in expected {
            assert!(
                provided.contains(&wanted.to_string()),
                "case {owner:?}: the service's own {wanted} must still be listed, \
                 got {provided:?}"
            );
        }
    }
}
