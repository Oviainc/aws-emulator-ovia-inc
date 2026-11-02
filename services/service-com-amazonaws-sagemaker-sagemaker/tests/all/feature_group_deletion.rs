// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker Feature Store's `DeleteFeatureGroup`, proved
//! through the readers a deletion has to show up in — `DescribeFeatureGroup`, `Search`
//! and `GetSearchSuggestions` — and through `CreateFeatureGroup`, which gets the name
//! back once the deletion has completed.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, on the
//! harness `inference_deployment_lifecycle` declares, so what these tests exercise is
//! what a real SDK client gets.
//!
//! The tests assert RULES over TABLES of inputs rather than one recorded call:
//!
//! * a delete answers 200 carrying nothing, and the group it names reports
//!   `FeatureGroupStatus`'s `Deleting` from that call until the clock completes the
//!   removal, after which every reader answers as if the group were gone and the name is
//!   free for another group — for groups differing in name shape, which stores they have,
//!   how many features they declare and whether their provisioning had settled;
//! * a name this account holds no group under — never created, or already on its way out,
//!   or gone — is refused with the modelled `ResourceNotFound` and the sentence SageMaker
//!   words it with, and the refusal changes nothing;
//! * a delete touches only the group it names: every other group is still described,
//!   searched and suggested afterwards, with its own status.

use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{
    error_code, f64_field, message, str_field, Emulator, ACCOUNT,
};

/// Longer than `state::FEATURE_GROUP_CREATED_AFTER` (300 ms), so the clock owes a created
/// group its `Created` status by the time the next call runs.
const PAST_CREATED_DUE: Duration = Duration::from_millis(600);

/// Longer than `state::FEATURE_GROUP_RELEASED_AFTER` (300 ms), so the clock owes a
/// requested deletion its completion by the time the next call runs.
const PAST_RELEASE_DUE: Duration = Duration::from_millis(600);

/// A role ARN matching `RoleArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`), required of a group
/// that has an OfflineStore.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/FeatureStoreRole";

/// How SageMaker words a feature group this account does not hold, as the handlers answer
/// it: the `Resource Not Found:` prefix, the resource type and the name the request
/// addressed, ending with NO period. Spelled out here rather than imported from the
/// crate, so the wording itself is under test.
fn not_found_sentence(name: &str) -> String {
    format!("Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name {name}")
}

/// A valid `CreateFeatureGroup` body for `name`, with `extra`'s members laid over it.
///
/// A member of `extra` whose value is `null` is REMOVED from the body, which is how a row
/// drops an otherwise-present member rather than sending a JSON null — the same shape
/// `feature_group_lifecycle` builds its bodies in.
fn create_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "FeatureGroupName": name,
        "RecordIdentifierFeatureName": "customer-id",
        "EventTimeFeatureName": "event-time",
        "FeatureDefinitions": [
            { "FeatureName": "customer-id", "FeatureType": "String" },
            { "FeatureName": "event-time", "FeatureType": "String" },
        ],
        "OnlineStoreConfig": { "EnableOnlineStore": true },
    });
    let object = body.as_object_mut().expect("a JSON object body");
    for (key, value) in extra.as_object().expect("a JSON object of extra members") {
        if value.is_null() {
            object.remove(key);
        } else {
            object.insert(key.clone(), value.clone());
        }
    }
    body
}

/// Create the feature group `body` describes, answering its minted `FeatureGroupArn`.
async fn create_feature_group(emulator: &Emulator, body: &Value) -> String {
    let created = emulator.ok("CreateFeatureGroup", body).await;
    str_field(&created, "FeatureGroupArn")
}

/// `DeleteFeatureGroup` for `name`, requiring a 200 whose body carries no member: the
/// operation's output is the modelled `Unit`.
async fn delete_feature_group(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = emulator
        .call("DeleteFeatureGroup", &json!({ "FeatureGroupName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteFeatureGroup, got {status} with {body}"
    );
    let members = match &body {
        Value::Null => 0,
        Value::Object(members) => members.len(),
        other => panic!("case {case:?}: expected an empty output, got {other}"),
    };
    assert_eq!(
        members, 0,
        "case {case:?}: DeleteFeatureGroup's output is modelled Unit, so it must carry no \
         member, got {body}"
    );
}

/// `DescribeFeatureGroup` for `name_or_arn`: the whole response when it is served, or
/// `None` when the read is refused with `ResourceNotFound` carrying the sentence for the
/// name it resolved to. Any other answer panics: those are the only two outcomes a reader
/// of a deleted group may give.
async fn describe(emulator: &Emulator, case: &str, name_or_arn: &str) -> Option<Value> {
    let (status, body) = emulator
        .call(
            "DescribeFeatureGroup",
            &json!({ "FeatureGroupName": name_or_arn }),
        )
        .await;
    match status {
        200 => Some(body),
        400 => {
            assert_eq!(
                error_code(&body),
                "ResourceNotFound",
                "case {case:?}: a group that is gone must read as ResourceNotFound, got {body}"
            );
            None
        }
        other => {
            panic!("case {case:?}: unexpected HTTP {other} from DescribeFeatureGroup: {body}")
        }
    }
}

/// The `FeatureGroupStatus` `DescribeFeatureGroup` reports for `name_or_arn`, or `None`
/// when the group is gone.
async fn status_of(emulator: &Emulator, case: &str, name_or_arn: &str) -> Option<String> {
    describe(emulator, case, name_or_arn)
        .await
        .map(|body| str_field(&body, "FeatureGroupStatus"))
}

/// The `FeatureGroupName`s `Search` reports for the `FeatureGroup` resource type, sorted.
async fn searched_names(emulator: &Emulator) -> Vec<String> {
    let answered = emulator
        .ok("Search", &json!({ "Resource": "FeatureGroup" }))
        .await;
    let mut names: Vec<String> = answered
        .get("Results")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Results list, got {answered}"))
        .iter()
        .map(|result| {
            let record = result
                .get("FeatureGroup")
                .unwrap_or_else(|| panic!("expected a FeatureGroup record, got {result}"));
            str_field(record, "FeatureGroupName")
        })
        .collect();
    names.sort();
    names
}

/// The property names `GetSearchSuggestions` offers for the `FeatureGroup` resource type.
async fn suggested_names(emulator: &Emulator) -> Vec<String> {
    let answered = emulator
        .ok("GetSearchSuggestions", &json!({ "Resource": "FeatureGroup" }))
        .await;
    answered
        .get("PropertyNameSuggestions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a PropertyNameSuggestions list, got {answered}"))
        .iter()
        .map(|suggestion| str_field(suggestion, "PropertyName"))
        .collect()
}

/// One row of the delete round trip: a case name, the group's name and the members laid
/// over the default create body.
///
/// Every row lets the group's provisioning settle before the delete, because a group is
/// deletable only once it exists: the refusal while it is still `Creating` is pinned in
/// `service_resolved_members`.
struct DeleteCase {
    case: &'static str,
    name: String,
    extra: Value,
}

#[tokio::test]
async fn a_delete_moves_the_group_to_deleting_until_the_clock_removes_it() {
    // The rows differ along what the rule generalises over: the name's shape and length,
    // which stores the group has, how many features it declares, and whether it carries
    // tags and a description.
    let longest = format!("d{}", "x".repeat(63));
    let cases = [
        DeleteCase {
            case: "an online store only, deleted while nothing else changed",
            name: "online-only".to_string(),
            extra: json!({}),
        },
        DeleteCase {
            case: "an online store only, deleted once Created",
            name: "online_settled".to_string(),
            extra: json!({}),
        },
        DeleteCase {
            case: "an offline store only",
            name: "offline-only".to_string(),
            extra: json!({
                "OnlineStoreConfig": null,
                "OfflineStoreConfig": { "S3StorageConfig": { "S3Uri": "s3://feature-store/offline/" } },
                "RoleArn": ROLE_ARN,
            }),
        },
        DeleteCase {
            case: "both stores, provisioned throughput, tags and a description",
            name: "both-stores-2".to_string(),
            extra: json!({
                "OfflineStoreConfig": { "S3StorageConfig": { "S3Uri": "s3://feature-store/both/" } },
                "RoleArn": ROLE_ARN,
                "ThroughputConfig": {
                    "ThroughputMode": "Provisioned",
                    "ProvisionedReadCapacityUnits": 10,
                    "ProvisionedWriteCapacityUnits": 5,
                },
                "Description": "a group with every member set",
                "Tags": [{ "Key": "Project", "Value": "retention" }],
            }),
        },
        DeleteCase {
            case: "the longest name the pattern admits",
            name: longest.clone(),
            extra: json!({}),
        },
        DeleteCase {
            case: "a digits-only name and a third feature",
            name: "90210".to_string(),
            extra: json!({
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                    { "FeatureName": "balance", "FeatureType": "Fractional" },
                ],
            }),
        },
    ];

    for DeleteCase { case, name, extra } in &cases {
        let emulator = Emulator::new();
        let created = create_feature_group(&emulator, &create_body(name, extra.clone())).await;
        assert_eq!(
            created,
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:feature-group/{name}"),
            "case {case:?}: the create must mint the group's ARN from its own name"
        );
        tokio::time::sleep(PAST_CREATED_DUE).await;
        assert_eq!(
            status_of(&emulator, case, name).await.as_deref(),
            Some("Created"),
            "case {case:?}: expected the clock to have provisioned the group"
        );

        delete_feature_group(&emulator, case, name).await;

        // Asked for, not finished: the group reports `Deleting` straight after, under
        // both spellings `FeatureGroupNameOrArn` admits, and still replays its own
        // identity rather than a half-empty record.
        for (spelling, identifier) in [("by name", name.clone()), ("by ARN", created.clone())] {
            let reported = describe(&emulator, case, &identifier)
                .await
                .unwrap_or_else(|| panic!("case {case:?} {spelling}: expected the group to be readable while Deleting"));
            assert_eq!(
                str_field(&reported, "FeatureGroupStatus"),
                "Deleting",
                "case {case:?} {spelling}: expected Deleting straight after the delete, got \
                 {reported}"
            );
            assert_eq!(
                (
                    str_field(&reported, "FeatureGroupArn"),
                    str_field(&reported, "FeatureGroupName")
                ),
                (created.clone(), name.clone()),
                "case {case:?} {spelling}: the group must keep its identity while Deleting"
            );
            // "A timestamp indicating when the feature group was last updated": the
            // deletion is an update, so it is no earlier than the creation.
            assert!(
                f64_field(&reported, "LastModifiedTime") >= f64_field(&reported, "CreationTime"),
                "case {case:?} {spelling}: expected LastModifiedTime at or after \
                 CreationTime, got {reported}"
            );
        }

        // And once the clock has completed the removal, the group is gone for every
        // reader, under either spelling, with the sentence SageMaker words it with.
        tokio::time::sleep(PAST_RELEASE_DUE).await;
        for (spelling, identifier) in [("by name", name.clone()), ("by ARN", created.clone())] {
            let (status, body) = emulator
                .call(
                    "DescribeFeatureGroup",
                    &json!({ "FeatureGroupName": identifier }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFound"),
                "case {case:?} {spelling}: expected 400 ResourceNotFound once the removal \
                 completed, got {status} with {body}"
            );
            assert_eq!(
                message(&body),
                not_found_sentence(name),
                "case {case:?} {spelling}: expected the not-found sentence for the group's \
                 own name"
            );
        }
        assert_eq!(
            searched_names(&emulator).await,
            Vec::<String>::new(),
            "case {case:?}: expected no feature group left to search"
        );

        // The name and the quota slot are free again: the same name creates a new group,
        // which is `Creating` rather than still `Deleting`.
        let recreated = create_feature_group(&emulator, &create_body(name, extra.clone())).await;
        assert_eq!(
            recreated, created,
            "case {case:?}: a group recreated under the same name carries the same ARN"
        );
        assert_ne!(
            status_of(&emulator, case, name).await.as_deref(),
            Some("Deleting"),
            "case {case:?}: the recreated group must not inherit the deleted one's status"
        );
    }
}

#[tokio::test]
async fn a_name_this_account_holds_no_group_under_is_refused_as_resource_not_found() {
    let emulator = Emulator::new();
    let kept = create_feature_group(&emulator, &create_body("kept-group", json!({}))).await;

    // A group whose deletion was asked for, and one whose removal the clock completed:
    // both are names this account no longer holds a deletable group under. Each is left
    // to finish provisioning first, because a group is deletable only once it exists.
    create_feature_group(&emulator, &create_body("deleting-group", json!({}))).await;
    create_feature_group(&emulator, &create_body("gone-group", json!({}))).await;
    tokio::time::sleep(PAST_CREATED_DUE).await;
    delete_feature_group(&emulator, "the group that is on its way out", "deleting-group").await;
    delete_feature_group(&emulator, "the group whose removal completes", "gone-group").await;
    tokio::time::sleep(PAST_RELEASE_DUE).await;

    // Every row satisfies `FeatureGroupName`'s pattern and length, so the refusal comes
    // from the handler rather than from boundary validation.
    let cases = [
        ("a name never created", "nosuchgroup".to_string()),
        ("a prefix of a held name", "kept".to_string()),
        ("a held name with a suffix", "kept-group-2".to_string()),
        (
            "a held name in another case",
            "KEPT-GROUP".to_string(),
        ),
        ("a group already Deleting", "deleting-group".to_string()),
        ("a group whose removal completed", "gone-group".to_string()),
        ("the longest name the pattern admits", format!("z{}", "y".repeat(63))),
    ];
    for (case, name) in &cases {
        let (status, body) = emulator
            .call("DeleteFeatureGroup", &json!({ "FeatureGroupName": name }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} with {body}"
        );
        assert_eq!(
            message(&body),
            not_found_sentence(name),
            "case {case:?}: expected the not-found sentence naming the requested name"
        );
    }

    // None of the refusals touched the group this account does hold, and the one that was
    // already `Deleting` kept that status rather than being moved again.
    assert_eq!(
        status_of(&emulator, "the kept group", &kept).await.as_deref(),
        Some("Created"),
        "expected the untouched group to survive every refusal"
    );
    assert_eq!(
        searched_names(&emulator).await,
        vec!["kept-group".to_string()],
        "expected only the group that was never deleted to be searchable"
    );
}

#[tokio::test]
async fn a_delete_touches_only_the_group_it_names() {
    let emulator = Emulator::new();
    let names = ["first", "second", "third", "fourth"];
    let mut arns = Vec::new();
    for name in names {
        arns.push(
            create_feature_group(
                &emulator,
                &create_body(
                    name,
                    json!({ "Tags": [{ "Key": format!("owner-{name}"), "Value": name }] }),
                ),
            )
            .await,
        );
    }
    tokio::time::sleep(PAST_CREATED_DUE).await;

    // Delete them one at a time, in a different order than they were created: after each
    // removal exactly the remaining groups are described, searched and suggested.
    let order = [2usize, 0, 3, 1];
    let mut remaining: Vec<usize> = (0..names.len()).collect();
    for index in order {
        let case = names[index];
        delete_feature_group(&emulator, case, names[index]).await;
        remaining.retain(|held| *held != index);
        tokio::time::sleep(PAST_RELEASE_DUE).await;

        let mut expected: Vec<String> = remaining
            .iter()
            .map(|held| names[*held].to_string())
            .collect();
        expected.sort();
        assert_eq!(
            searched_names(&emulator).await,
            expected,
            "case {case:?}: expected exactly the undeleted groups to be searchable"
        );
        let suggested = suggested_names(&emulator).await;
        assert!(
            !suggested.contains(&format!("tags.owner-{case}")),
            "case {case:?}: a deleted group's tag key must not be suggested, got \
             {suggested:?}"
        );
        for held in &remaining {
            assert!(
                suggested.contains(&format!("tags.owner-{}", names[*held])),
                "case {case:?}: {:?} must keep its suggested tag key, got {suggested:?}",
                names[*held]
            );
            assert_eq!(
                status_of(&emulator, case, &arns[*held]).await.as_deref(),
                Some("Created"),
                "case {case:?}: {:?} must keep its own status",
                names[*held]
            );
        }
        assert_eq!(
            status_of(&emulator, case, &arns[index]).await,
            None,
            "case {case:?}: the deleted group must be gone for every reader"
        );
    }
}
