// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteResourceSet`.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and a
//! SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rule under test is one sentence: deleting a Firewall Manager resource set
//! removes the stored resource the request's identifier names — the set's
//! document, its tags and the resources associated with it — so no later read of
//! that identifier can see it, while every OTHER stored resource set is
//! untouched; an identifier that names no stored set is
//! `ResourceNotFoundException`, and the organisation-scoped administrator gate is
//! decided before the identifier is looked up, so a refused call deletes nothing.
//!
//! The table in [`cases`] is the class that rule ranges over: the rows differ in
//! which optional members the set carries, in how many resource types it holds,
//! in tag count and in how many resources are associated with it (none, one,
//! several).

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::ResourceSet;
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness lives in one place for this test target: `policy_lifecycle`
// owns it, and the administrator helpers (`associate_admin`, `with_store`) live
// with the gate suite — so no two suites drift in how a request is signed, an
// error envelope is read or the store is reached.
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, ACCOUNT, REGION};

/// Well-formed `Base62Id`s (the model's `com.amazonaws.fms#Base62Id`: 22
/// characters matching `^[a-z0-9A-Z]{22}$`) that name no resource set. The rows
/// differ in character class: all digits, all lower case, all upper case and
/// mixed.
const UNKNOWN_IDS: &[&str] = &[
    "0000000000000000000001",
    "abcdefghijklmnopqrstuv",
    "ABCDEFGHIJKLMNOPQRSTUV",
    "aB3dE6gH9jK2mN5pQ8sT1v",
];

/// One row of the resource-set table: a name for the assertion messages, the
/// `ResourceSet` document a client sends, its tags, and the resource ARNs
/// associated with it after it is created.
struct Case {
    name: &'static str,
    resource_set: Value,
    tags: Value,
    resources: Vec<String>,
}

/// The resource-set class the deletion rule is asserted over.
///
/// The `ResourceTypeList` entries and the resource ARNs are matched to each
/// other, because `ResourceSet$ResourceTypeList` "determines the resources that
/// can be associated to the resource set" — so every row's resources really do
/// become its membership before it is deleted.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The minimum a resource set can be, holding nothing at all.
            name: "minimal-with-no-resources",
            resource_set: json!({
                "Name": "minimal-set",
                "ResourceTypeList": ["AWS::NetworkFirewall::Firewall"],
            }),
            tags: json!([]),
            resources: Vec::new(),
        },
        Case {
            name: "described-three-types-one-resource",
            resource_set: json!({
                "Name": "set.with-three_types",
                "Description": "resources imported into a Firewall Manager policy",
                "ResourceTypeList": [
                    "AWS::EC2::Instance",
                    "AWS::EC2::VPC",
                    "AWS::NetworkFirewall::Firewall",
                ],
            }),
            tags: json!([{ "Key": "team", "Value": "security" }]),
            resources: vec![format!(
                "arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/edge"
            )],
        },
        Case {
            name: "many-tags-three-resources",
            resource_set: json!({
                "Name": "tagged-set",
                "ResourceTypeList": ["AWS::EC2::Instance"],
            }),
            tags: json!([
                { "Key": "a", "Value": "1" },
                { "Key": "b", "Value": "2" },
                { "Key": "c", "Value": "3" },
            ]),
            resources: vec![
                format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0123456789abcdef0"),
                format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0123456789abcdef1"),
                format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0123456789abcdef2"),
            ],
        },
    ]
}

/// Create one row's resource set and associate its resources, returning the
/// minted `(Id, ResourceSetArn)`.
async fn create_set(router: &MultiServiceRouter, case: &Case) -> (String, String) {
    let (status, put) = call(
        router,
        "PutResourceSet",
        &json!({ "ResourceSet": case.resource_set, "TagList": case.tags }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected PutResourceSet to answer 200, got {status} with {put}",
        case.name
    );
    let id = put["ResourceSet"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let arn = put["ResourceSetArn"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "case {:?}: expected a minted Id and ARN, got {put}",
        case.name
    );

    if !case.resources.is_empty() {
        let (status, body) = call(
            router,
            "BatchAssociateResource",
            &json!({ "ResourceSetIdentifier": id, "Items": case.resources }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected BatchAssociateResource to answer 200, got {status} with {body}",
            case.name
        );
        assert_eq!(
            body["FailedItems"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: every seeded resource must associate, got {body}",
            case.name
        );
    }
    (id, arn)
}

/// Whether an awsJson1_1 success body carries no members at all, which is what
/// an operation whose output shape is `Unit` answers.
fn is_empty_body(body: &Value) -> bool {
    body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty)
}

/// `DeleteResourceSet` removes the set and everything the stored record holds —
/// its document, its tags and its associated resources — for every row of the
/// table: the delete answers 200 with no members, `GetResourceSet` can no longer
/// find the identifier, and a second delete of the same identifier finds nothing
/// to delete.
#[tokio::test]
async fn delete_resource_set_removes_the_set_and_everything_it_held() {
    for case in cases() {
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        let (id, arn) = create_set(&router, &case).await;

        // The precondition, read through implemented operations and the store the
        // handlers share rather than assumed.
        let (status, before) = call(&router, "GetResourceSet", &json!({ "Identifier": id })).await;
        assert_eq!(
            status, 200,
            "case {:?}: the set must exist before it is deleted, got {status} with {before}",
            case.name
        );
        with_store(&engine, |store| {
            let record = store
                .resource_sets
                .get(&id)
                .unwrap_or_else(|| panic!("case {:?}: the created set is stored", case.name));
            assert_eq!(
                record.resources.len(),
                case.resources.len(),
                "case {:?}: expected {} associated resources, got {:?}",
                case.name,
                case.resources.len(),
                record.resources
            );
            assert!(
                store.tags_by_arn.contains_key(&arn),
                "case {:?}: the created set must be in the ARN-keyed tag index, got {:?}",
                case.name,
                store.tags_by_arn.keys().collect::<Vec<_>>()
            );
        });

        let (status, deleted) = call(&router, "DeleteResourceSet", &json!({ "Identifier": id })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DeleteResourceSet to answer 200, got {status} with {deleted}",
            case.name
        );
        assert!(
            is_empty_body(&deleted),
            "case {:?}: DeleteResourceSet models no output members, got {deleted}",
            case.name
        );

        let (status, body) = call(&router, "GetResourceSet", &json!({ "Identifier": id })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {:?}: a deleted set must not be readable, got {status} with {body}",
            case.name
        );

        with_store(&engine, |store| {
            assert!(
                !store.resource_sets.contains_key(&id),
                "case {:?}: the deleted set must be gone from the store, got {:?}",
                case.name,
                store.resource_sets.keys().collect::<Vec<_>>()
            );
            assert!(
                !store.tags_by_arn.contains_key(&arn),
                "case {:?}: the deleted set's tags must not outlive it, got {:?}",
                case.name,
                store.tags_by_arn.keys().collect::<Vec<_>>()
            );
        });

        // A second delete has nothing left to delete.
        let (status, body) = call(&router, "DeleteResourceSet", &json!({ "Identifier": id })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {:?}: deleting the same set twice must find nothing, got {status} with {body}",
            case.name
        );
    }
}

/// Deleting ONE resource set leaves every other one exactly as it was: the ones
/// that remain still read back byte for byte what they did before the delete,
/// and the store holds precisely the identifiers that were not deleted.
#[tokio::test]
async fn deleting_one_resource_set_leaves_the_others_intact() {
    let table = cases();
    // Each row gets a turn as the deleted one, so the rule is asserted on the
    // first, a middle and the last key of the store's total order.
    for deleted_row in 0..table.len() {
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;

        let mut created = Vec::new();
        for case in &table {
            let (id, arn) = create_set(&router, case).await;
            let (_, got) = call(&router, "GetResourceSet", &json!({ "Identifier": id })).await;
            created.push((case.name, id, arn, got));
        }

        let (_, target_id, target_arn, _) = &created[deleted_row];
        let (status, body) = call(
            &router,
            "DeleteResourceSet",
            &json!({ "Identifier": target_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DeleteResourceSet to answer 200, got {status} with {body}",
            table[deleted_row].name
        );

        for (index, (name, id, arn, before)) in created.iter().enumerate() {
            let (status, now) = call(&router, "GetResourceSet", &json!({ "Identifier": id })).await;
            if index == deleted_row {
                assert_eq!(
                    (status, error_code(&now).as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {name:?}: the deleted set must not be readable, got {status} with {now}"
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {name:?}: deleting {:?} must not affect it, got {status} with {now}",
                table[deleted_row].name
            );
            assert_eq!(
                now, *before,
                "case {name:?}: a surviving set must read back unchanged, got {now}"
            );
            assert_ne!(
                arn, target_arn,
                "case {name:?}: the table's rows must have distinct ARNs"
            );
        }

        let surviving: Vec<String> = created
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != deleted_row)
            .map(|(_, (_, id, _, _))| id.clone())
            .collect();
        with_store(&engine, |store| {
            let mut wanted = surviving.clone();
            wanted.sort();
            let stored: Vec<String> = store.resource_sets.keys().cloned().collect();
            assert_eq!(
                stored, wanted,
                "case {:?}: the store must hold exactly the sets that were not deleted, got \
                 {stored:?}",
                table[deleted_row].name
            );
            let mut indexed: Vec<String> = store.tags_by_arn.keys().cloned().collect();
            indexed.sort();
            assert!(
                !indexed.contains(target_arn),
                "case {:?}: the deleted set's ARN must leave the tag index, got {indexed:?}",
                table[deleted_row].name
            );
        });
    }
}

/// An identifier that is well formed but names no stored resource set is
/// `ResourceNotFoundException`, whichever character class the `Base62Id` is drawn
/// from — and an identifier the model's `Base62Id` constraint refuses never
/// reaches the handler at all.
#[tokio::test]
async fn an_unknown_identifier_is_resource_not_found() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    // One real set exists, so "not found" is about the identifier rather than
    // about an empty store.
    let case = &cases()[1];
    let (id, _) = create_set(&router, case).await;

    for unknown in UNKNOWN_IDS {
        assert_ne!(
            *unknown, id,
            "case {unknown:?}: the unknown ids must not collide with the minted one"
        );
        let (status, body) = call(
            &router,
            "DeleteResourceSet",
            &json!({ "Identifier": unknown }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {unknown:?}: an unknown identifier must be ResourceNotFoundException, got \
             {status} with {body}"
        );
    }

    // Malformed identifiers: the model constrains `Identifier` to
    // `com.amazonaws.fms#Base62Id` (length 22..22, `^[a-z0-9A-Z]{22}$`), so a
    // value outside it is refused before the handler runs — one past each bound
    // and one with a character the pattern excludes.
    for malformed in [
        "0123456789AbCdEfGhIjK",   // 21 characters: one under the length
        "0123456789AbCdEfGhIjKlM", // 23 characters: one over the length
        "0123456789AbCdEfGhIj-l",  // 22 characters, one outside [a-z0-9A-Z]
    ] {
        let (status, body) = call(
            &router,
            "DeleteResourceSet",
            &json!({ "Identifier": malformed }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {malformed:?}: a value outside the Base62Id constraint must be refused, got \
             {status} with {body}"
        );
    }

    // None of the refusals touched the set that does exist.
    let (status, body) = call(&router, "GetResourceSet", &json!({ "Identifier": id })).await;
    assert_eq!(
        status, 200,
        "a refused delete must leave the existing set alone, got {status} with {body}"
    );
    with_store(&engine, |store| {
        assert_eq!(
            store.resource_sets.len(),
            1,
            "a refused delete must not change the store, got {:?}",
            store.resource_sets.keys().collect::<Vec<_>>()
        );
    });
}

/// With no administrator association, `DeleteResourceSet` is refused with HTTP
/// 400 `AccessDeniedException` BEFORE the identifier is looked up — so a stored
/// set is still there afterwards, and a nonexistent identifier gets the
/// authorization refusal rather than `ResourceNotFoundException`.
#[tokio::test]
async fn delete_resource_set_is_administrator_gated_and_deletes_nothing_when_refused() {
    let (router, engine) = emulator_with_engine();

    // Seeded through the store the handlers share, because with no administrator
    // association `PutResourceSet` is refused too: the point of the test is that
    // the gate is decided without the record being consulted.
    let seeded = "zZ9yY8xX7wW6vV5uU4tT3s";
    let seeded_arn = state::resource_set_arn(REGION, seeded);
    with_store(&engine, |store| {
        let now = std::time::SystemTime::now();
        store.resource_sets.insert(
            seeded.to_string(),
            state::ResourceSetRecord {
                id: seeded.to_string(),
                arn: seeded_arn.clone(),
                update_token: state::mint_identifier(),
                resource_set: ResourceSet {
                    name: "seeded-set".to_string(),
                    resource_type_list: vec!["AWS::EC2::Instance".to_string()],
                    ..ResourceSet::default()
                },
                tags: Vec::new(),
                resources: std::collections::BTreeSet::new(),
                created_at: now,
                updated_at: now,
            },
        );
    });

    for identifier in [seeded, UNKNOWN_IDS[0]] {
        let (status, body) = call(
            &router,
            "DeleteResourceSet",
            &json!({ "Identifier": identifier }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {identifier:?}: with no administrator the delete must be refused before the \
             lookup, got {status} with {body}"
        );
    }

    with_store(&engine, |store| {
        assert!(
            store.resource_sets.contains_key(seeded),
            "a refused delete must delete nothing, got {:?}",
            store.resource_sets.keys().collect::<Vec<_>>()
        );
    });

    // With an administrator associated the same call acts on the store.
    associate_admin(&router).await;
    let (status, body) = call(
        &router,
        "DeleteResourceSet",
        &json!({ "Identifier": seeded }),
    )
    .await;
    assert_eq!(
        status, 200,
        "with an administrator the seeded set must delete, got {status} with {body}"
    );
    with_store(&engine, |store| {
        assert!(
            store.resource_sets.is_empty(),
            "the seeded set must be gone, got {:?}",
            store.resource_sets.keys().collect::<Vec<_>>()
        );
    });
}
