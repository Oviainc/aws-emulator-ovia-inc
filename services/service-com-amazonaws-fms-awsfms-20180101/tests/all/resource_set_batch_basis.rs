// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the step BOTH resource-set batch membership operations
//! take: re-reading the resource set the resolved identifier names, under the
//! lock the call already holds, and classifying the batch against that set's own
//! `ResourceTypeList` and the membership it currently holds.
//!
//! The rule under test is one sentence: a batch membership call classifies its
//! items against the stored resource set it resolved — that set's declared
//! `ResourceTypeList` and its current membership — or, when no such record is
//! there to read, answers the service's not-found error instead of classifying
//! the batch against nothing and writing nothing.
//!
//! `BatchAssociateResource` and `BatchDisassociateResource` both take that step,
//! so the two reach it through one shared path
//! (`crate::state::resource_set_batch_basis`, whose write half is
//! `crate::state::write_resource_set_resources`) and cannot disagree about which
//! items a given set can hold or about what a lookup miss answers. These tests
//! drive that shared path through BOTH operations over the same table: every row
//! is associated and then disassociated, and the per-item outcomes the two report
//! must be the same.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router, reusing the request harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response,
//! and the membership each call wrote is read back through the modelled reader
//! (`ListResourceSetResources`).

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::associate_admin;
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, ACCOUNT, REGION};
use crate::resource_set_membership_last_update_time::reported_membership;


/// A second Region, for an item whose ARN names another Region than the request.
const OTHER_REGION: &str = "eu-central-1";

/// An account that is in no organization this emulator knows.
const FOREIGN_ACCOUNT: &str = "999999999999";

/// A `Base62Id` (the model's `com.amazonaws.fms#Base62Id`: 22 characters
/// matching `^[a-z0-9A-Z]{22}$`) that names no resource set.
const UNKNOWN_RESOURCE_SET_ID: &str = "0123456789AbCdEfGhIjKl";

/// An EC2 instance ARN of this account and Region, one per index.
fn instance(index: usize) -> String {
    format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-{index:017x}")
}

/// A Network Firewall ARN of this account and Region.
fn firewall(name: &str) -> String {
    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/{name}")
}

/// A CloudFront distribution ARN: a GLOBAL service's ARN, so it names no Region
/// at all.
fn distribution(id: &str) -> String {
    format!("arn:aws:cloudfront::{ACCOUNT}:distribution/{id}")
}

/// An Elastic IP ARN, which Amazon EC2 addresses by its allocation.
fn elastic_ip(id: &str) -> String {
    format!("arn:aws:ec2:{REGION}:{ACCOUNT}:elastic-ip/eipalloc-{id}")
}

/// One item of a batch: the URI and the `FailedItemReason` both operations must
/// report it with, or `None` when the set can hold it.
struct Item {
    uri: String,
    reason: Option<&'static str>,
}

impl Item {
    fn admissible(uri: String) -> Self {
        Self { uri, reason: None }
    }

    fn refused(uri: String, reason: &'static str) -> Self {
        Self {
            uri,
            reason: Some(reason),
        }
    }
}

/// One row of the table: a name for the assertion messages, the set's
/// `ResourceTypeList`, and the items one batch call carries.
struct Case {
    name: &'static str,
    resource_types: Vec<&'static str>,
    items: Vec<Item>,
}

impl Case {
    /// The `Items` list a batch call sends, in the order the row wrote it.
    fn items(&self) -> Vec<String> {
        self.items.iter().map(|item| item.uri.clone()).collect()
    }

    /// The `(URI, Reason)` pairs both operations must report, in a total order.
    fn expected_failures(&self) -> Vec<(String, String)> {
        let mut expected: Vec<(String, String)> = self
            .items
            .iter()
            .filter_map(|item| {
                item.reason
                    .map(|reason| (item.uri.clone(), reason.to_string()))
            })
            .collect();
        expected.sort();
        expected
    }

    /// The membership the set holds once the row's batch is associated: every
    /// item the set can hold, deduplicated and in the stored total order.
    fn expected_membership(&self) -> Vec<String> {
        let mut admitted: Vec<String> = self
            .items
            .iter()
            .filter(|item| item.reason.is_none())
            .map(|item| item.uri.clone())
            .collect();
        admitted.sort();
        admitted.dedup();
        admitted
    }
}

/// The class the shared step ranges over.
///
/// The rows differ in how many resource types the set declares (one, two,
/// three), in how many items one call carries (one to four), in which ARN field
/// of an item disagrees with the set, in whether the item's ARN carries a Region
/// at all (a global service's does not), in whether the owning service spells
/// the type differently from the resource-type name (`elastic-ip` for
/// `AWS::EC2::EIP`), in whether the batch repeats an item, and in whether the
/// set admits every item, some of them or none.
///
/// The `AWS::<Service>::<Type>` names are `ResourceSet$ResourceTypeList`
/// entries, and every expected reason is a value of the model's
/// `FailedItemReason`
/// (`smithy_query(query="shape",shape="FailedItemReason")`, mirrored in
/// `crate::ops::types::FAILED_ITEM_REASON_VALUES`, which the first test checks
/// each row against).
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The smallest batch there is, every item admissible.
            name: "one-type-one-admissible-item",
            resource_types: vec!["AWS::EC2::Instance"],
            items: vec![Item::admissible(instance(0))],
        },
        Case {
            name: "two-types-admissible-items-of-both",
            resource_types: vec!["AWS::EC2::Instance", "AWS::NetworkFirewall::Firewall"],
            items: vec![
                Item::admissible(instance(1)),
                Item::admissible(firewall("edge")),
            ],
        },
        Case {
            // A type the owning service spells its own way, and a global
            // service's ARN, which names no Region to disagree with.
            name: "three-types-including-a-global-service-and-a-respelled-type",
            resource_types: vec![
                "AWS::EC2::Instance",
                "AWS::CloudFront::Distribution",
                "AWS::EC2::EIP",
            ],
            items: vec![
                Item::admissible(distribution("E1EXAMPLE1EXAM")),
                Item::admissible(elastic_ip("0123456789abcdef0")),
                Item::refused(firewall("unknown-type"), "NOT_VALID_RESOURCE_TYPE"),
            ],
        },
        Case {
            // One admissible item beside one refused on each ARN field this
            // service judges.
            name: "one-type-admissible-beside-every-kind-of-refusal",
            resource_types: vec!["AWS::EC2::Instance"],
            items: vec![
                Item::admissible(instance(2)),
                Item::refused("this-is-not-an-arn-at-all".to_string(), "NOT_VALID_ARN"),
                Item::refused(
                    format!("arn:aws-cn:ec2:{REGION}:{ACCOUNT}:instance/i-1"),
                    "NOT_VALID_PARTITION",
                ),
                Item::refused(
                    format!("arn:aws:ec2:{OTHER_REGION}:{ACCOUNT}:instance/i-1"),
                    "NOT_VALID_REGION",
                ),
                Item::refused(
                    format!("arn:aws:ec2:{REGION}:{FOREIGN_ACCOUNT}:instance/i-1"),
                    "NOT_VALID_ACCOUNT_ID",
                ),
            ],
        },
        Case {
            // Nothing the set can hold: the membership stays empty, and the
            // refusals are per-item rather than a refusal of the call.
            name: "one-type-no-admissible-item",
            resource_types: vec!["AWS::NetworkFirewall::Firewall"],
            items: vec![
                Item::refused(instance(3), "NOT_VALID_RESOURCE_TYPE"),
                Item::refused(
                    format!("arn:aws:network-firewall:{OTHER_REGION}:{ACCOUNT}:firewall/far"),
                    "NOT_VALID_REGION",
                ),
            ],
        },
        Case {
            // A repeated item is one item: the membership is a SET.
            name: "two-types-with-a-repeated-item",
            resource_types: vec!["AWS::EC2::Instance", "AWS::CloudFront::Distribution"],
            items: vec![
                Item::admissible(instance(4)),
                Item::admissible(instance(4)),
                Item::admissible(distribution("E2EXAMPLE2EXAM")),
            ],
        },
    ]
}

/// The `(URI, Reason)` pairs a `FailedItems` list reports, in a total order so
/// two lists compare whatever order the handler built them in.
fn reported_failures(body: &Value) -> Vec<(String, String)> {
    let mut reported: Vec<(String, String)> = body["FailedItems"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item["URI"].as_str().unwrap_or_default().to_string(),
                        item["Reason"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    reported.sort();
    reported
}

/// Create a resource set declaring `resource_types` and hand back its
/// `(Id, ResourceSetArn)`.
async fn create_set(
    router: &MultiServiceRouter,
    case_name: &str,
    resource_types: &[&str],
) -> (String, String) {
    let (status, created) = call(
        router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Name": format!("set-{case_name}"),
                "ResourceTypeList": resource_types,
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case_name:?}: expected PutResourceSet to answer 200, got {status} with {created}"
    );
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let arn = created["ResourceSetArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "case {case_name:?}: expected a minted Id and ARN, got {created}"
    );
    (id, arn)
}

/// Both batch membership operations classify the SAME batch against the SAME
/// stored resource set identically: the items the set can hold are the ones
/// associated and then disassociated, and the items it cannot hold are reported
/// with the same `FailedItemReason` by both calls.
#[tokio::test]
async fn both_batch_operations_classify_a_batch_against_the_stored_set() {
    let reasons = service_com_amazonaws_fms_awsfms_20180101::ops::types::FAILED_ITEM_REASON_VALUES;
    for case in cases() {
        let (router, _engine) = emulator_with_engine();
        associate_admin(&router).await;
        let (id, _) = create_set(&router, case.name, &case.resource_types).await;

        for item in &case.items {
            if let Some(reason) = item.reason {
                assert!(
                    reasons.contains(&reason),
                    "case {:?}: {reason:?} is not in the model's FailedItemReason catalog \
                     {reasons:?}",
                    case.name
                );
            }
        }

        // The associating half of the shared step.
        let (status, associated) = call(
            &router,
            "BatchAssociateResource",
            &json!({ "ResourceSetIdentifier": id, "Items": case.items() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected BatchAssociateResource to answer 200, got {status} with \
             {associated}",
            case.name
        );
        assert_eq!(
            associated["ResourceSetIdentifier"].as_str(),
            Some(id.as_str()),
            "case {:?}: the response must name the set that was changed, got {associated}",
            case.name
        );
        assert_eq!(
            reported_failures(&associated),
            case.expected_failures(),
            "case {:?}: expected BatchAssociateResource to report exactly {:?}, got {associated}",
            case.name,
            case.expected_failures()
        );

        // The write really landed: the modelled reader reports it.
        let held = reported_membership(&router, case.name, &id).await;
        assert_eq!(
            held,
            case.expected_membership(),
            "case {:?}: expected the membership {:?}, got {held:?}",
            case.name,
            case.expected_membership()
        );

        // The disassociating half, over the SAME items: the members go, and the
        // items the set cannot hold are reported with the same reasons.
        let (status, disassociated) = call(
            &router,
            "BatchDisassociateResource",
            &json!({ "ResourceSetIdentifier": id, "Items": case.items() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected BatchDisassociateResource to answer 200, got {status} with \
             {disassociated}",
            case.name
        );
        assert_eq!(
            reported_failures(&disassociated),
            case.expected_failures(),
            "case {:?}: expected BatchDisassociateResource to report the same failures {:?}, got \
             {disassociated}",
            case.name,
            case.expected_failures()
        );
        let left = reported_membership(&router, case.name, &id).await;
        assert_eq!(
            left,
            Vec::<String>::new(),
            "case {:?}: disassociating the whole batch must leave the set empty, got {left:?}",
            case.name
        );
    }
}

/// Neither operation classifies a batch against a resource set it did not find:
/// an identifier that names no stored set is the service's
/// `ResourceNotFoundException` from both, with no per-item result at all — not a
/// 200 that blames every item on an empty `ResourceTypeList`.
#[tokio::test]
async fn neither_batch_operation_classifies_against_a_set_it_did_not_find() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;
    // A set that DOES exist, so the refusals below are about the identifier
    // rather than about an empty store.
    let (id, _) = create_set(&router, "not-found", &["AWS::EC2::Instance"]).await;

    let unknown_arn = state::resource_set_arn(REGION, UNKNOWN_RESOURCE_SET_ID);
    for operation in ["BatchAssociateResource", "BatchDisassociateResource"] {
        for identifier in [
            UNKNOWN_RESOURCE_SET_ID,
            "abcdefghijklmnopqrstuv",
            unknown_arn.as_str(),
        ] {
            let (status, body) = call(
                &router,
                operation,
                &json!({ "ResourceSetIdentifier": identifier, "Items": [instance(0)] }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {operation:?} with {identifier:?}: an unknown identifier must be \
                 ResourceNotFoundException, got {status} with {body}"
            );
            assert!(
                body["FailedItems"].as_array().is_none(),
                "case {operation:?} with {identifier:?}: a not-found call must report no per-item \
                 result, got {body}"
            );
        }
    }

    // The real set is untouched by any of it.
    let held = reported_membership(&router, "not-found", &id).await;
    assert_eq!(
        held,
        Vec::<String>::new(),
        "a not-found call must leave the stored set alone, got {held:?}"
    );
}

/// The shared membership writer is reached by both operations, so a call that
/// changes nothing writes nothing and a call that changes something writes
/// exactly that: re-associating a member adds nothing, and disassociating a URI
/// the set does not hold removes nothing.
#[tokio::test]
async fn both_membership_writers_write_only_what_changed() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;
    let (id, arn) = create_set(&router, "idempotence", &["AWS::EC2::Instance"]).await;

    let members: Vec<String> = (0..3).map(instance).collect();
    for round in ["first", "second"] {
        // The set is named by its ARN on the second round: the identifier
        // resolves to the same stored record either way.
        let identifier = if round == "first" { &id } else { &arn };
        let (status, body) = call(
            &router,
            "BatchAssociateResource",
            &json!({ "ResourceSetIdentifier": identifier, "Items": members }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {round:?}: expected BatchAssociateResource to answer 200, got {status} with \
             {body}"
        );
        let held = reported_membership(&router, round, &id).await;
        assert_eq!(
            held, members,
            "case {round:?}: expected the membership {members:?}, got {held:?}"
        );
    }

    // Removing one the set never held removes nothing, and the one it holds goes.
    let (status, body) = call(
        &router,
        "BatchDisassociateResource",
        &json!({
            "ResourceSetIdentifier": arn,
            "Items": [instance(0), instance(99)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchDisassociateResource to answer 200, got {status} with {body}"
    );
    assert_eq!(
        reported_failures(&body),
        Vec::<(String, String)>::new(),
        "a URI the set could hold but does not is no failure, got {body}"
    );
    let held = reported_membership(&router, "removal", &id).await;
    assert_eq!(
        held,
        vec![instance(1), instance(2)],
        "expected exactly the unnamed members to remain, got {held:?}"
    );
}
