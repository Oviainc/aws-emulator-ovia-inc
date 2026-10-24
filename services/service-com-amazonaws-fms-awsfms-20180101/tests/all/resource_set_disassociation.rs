// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchDisassociateResource`.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and a
//! SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rule under test is one sentence: disassociating resources from a Firewall
//! Manager resource set removes from that set's membership every item the request
//! names that the set holds, leaving the members it does not name alone — the
//! membership is a SET, so naming a URI the set does not hold removes nothing and
//! is not a failure, and naming one twice is naming it once — while an item that
//! could not be a member of that set at all is reported in `FailedItems` with the
//! `FailedItemReason` of the first ARN field that disagrees, and the rest of the
//! batch is disassociated anyway.
//!
//! The table in [`cases`] is the class that rule ranges over: the rows differ in
//! how many resource types the set declares, in how many resources it holds, in
//! how many of them the request names (none, one of several, all of them), in
//! whether the request repeats an item or names one the set never held, and in
//! whether the batch carries items that cannot be members.
//!
//! The membership itself is read back through the store the handlers share,
//! because the operation that reports a resource set's members on the wire
//! (`ListResourceSetResources`) is still a scaffold tracked by its own issue —
//! the same seam the `BatchAssociateResource` suite asserts membership through.

use std::collections::BTreeSet;

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

/// A second Region, for an item whose ARN names another Region than the request.
const OTHER_REGION: &str = "eu-central-1";

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

/// One row of the disassociation table: a name for the assertion messages, the
/// set's `ResourceTypeList`, the resources associated with it before the call,
/// the `Items` the disassociate call names, and the `(URI, FailedItemReason)`
/// pairs that call must report.
struct Case {
    name: &'static str,
    resource_types: Vec<&'static str>,
    members: Vec<String>,
    remove: Vec<String>,
    expected_failures: Vec<(String, &'static str)>,
}

impl Case {
    /// The membership the rule says the set holds after the call: every member
    /// the request did not name.
    fn expected_membership(&self) -> Vec<String> {
        let named: BTreeSet<&String> = self.remove.iter().collect();
        let mut remaining: Vec<String> = self
            .members
            .iter()
            .filter(|uri| !named.contains(uri))
            .cloned()
            .collect();
        remaining.sort();
        remaining
    }
}

/// The class the disassociation rule is asserted over.
///
/// The `ResourceTypeList` entries and the resource ARNs are matched to each
/// other, because `ResourceSet$ResourceTypeList` "determines the resources that
/// can be associated to the resource set" — so every row's members really are
/// its membership before the disassociate call runs.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // Naming nothing removes nothing: the smallest batch there is.
            name: "names-no-item",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0), instance(1)],
            remove: Vec::new(),
            expected_failures: Vec::new(),
        },
        Case {
            name: "removes-one-of-three",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0), instance(1), instance(2)],
            remove: vec![instance(1)],
            expected_failures: Vec::new(),
        },
        Case {
            name: "removes-every-member-of-two-types",
            resource_types: vec!["AWS::EC2::Instance", "AWS::NetworkFirewall::Firewall"],
            members: vec![instance(0), firewall("edge")],
            remove: vec![firewall("edge"), instance(0)],
            expected_failures: Vec::new(),
        },
        Case {
            // A repeated item is one item, and one the set never held is a
            // no-op: neither is a failure.
            name: "repeats-an-item-and-names-one-never-held",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0), instance(1)],
            remove: vec![instance(0), instance(0), instance(7)],
            expected_failures: Vec::new(),
        },
        Case {
            // The failures do not stop the rest of the batch.
            name: "removes-a-member-beside-items-that-cannot-be-members",
            resource_types: vec!["AWS::EC2::Instance"],
            members: vec![instance(0), instance(1)],
            remove: vec![
                instance(0),
                "this-is-not-an-arn-at-all".to_string(),
                format!("arn:aws:ec2:{OTHER_REGION}:{ACCOUNT}:instance/i-0"),
            ],
            expected_failures: vec![
                ("this-is-not-an-arn-at-all".to_string(), "NOT_VALID_ARN"),
                (
                    format!("arn:aws:ec2:{OTHER_REGION}:{ACCOUNT}:instance/i-0"),
                    "NOT_VALID_REGION",
                ),
            ],
        },
    ]
}

/// Create a resource set holding `resource_types` and associate `members` with
/// it, returning the minted `(Id, ResourceSetArn)`.
async fn create_set_with_members(
    router: &MultiServiceRouter,
    case_name: &str,
    resource_types: &[&str],
    members: &[String],
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

    if !members.is_empty() {
        let (status, body) = call(
            router,
            "BatchAssociateResource",
            &json!({ "ResourceSetIdentifier": id, "Items": members }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: expected BatchAssociateResource to answer 200, got {status} with \
             {body}"
        );
        assert_eq!(
            body["FailedItems"].as_array().map(Vec::len),
            Some(0),
            "case {case_name:?}: every seeded member must associate, got {body}"
        );
    }
    (id, arn)
}

/// The `(URI, Reason)` pairs a `FailedItems` list reports, in a total order so
/// two lists can be compared whatever order the handler built them in.
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

/// The pairs a row expects, in the same total order.
fn expected_failures(case: &Case) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = case
        .expected_failures
        .iter()
        .map(|(uri, reason)| (uri.clone(), (*reason).to_string()))
        .collect();
    expected.sort();
    expected
}

/// The resource set's stored membership, in its own total order.
fn membership(engine: &std::sync::Arc<engine::InMemoryEngine>, id: &str) -> Vec<String> {
    with_store(engine, |store| {
        store
            .resource_sets
            .get(id)
            .map(|record| record.resources.iter().cloned().collect())
            .unwrap_or_default()
    })
}

/// `BatchDisassociateResource` removes exactly the members the request names,
/// leaves every other member alone, reports the items that cannot be members,
/// and is idempotent — for every row of the table.
#[tokio::test]
async fn disassociating_removes_exactly_the_named_members() {
    for case in cases() {
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        let (id, _) = create_set_with_members(
            &router,
            case.name,
            &case.resource_types,
            &case.members,
        )
        .await;

        // The precondition, read rather than assumed.
        let mut seeded = case.members.clone();
        seeded.sort();
        seeded.dedup();
        assert_eq!(
            membership(&engine, &id),
            seeded,
            "case {:?}: the seeded members must be the set's membership",
            case.name
        );

        let (status, body) = call(
            &router,
            "BatchDisassociateResource",
            &json!({ "ResourceSetIdentifier": id, "Items": case.remove }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected BatchDisassociateResource to answer 200, got {status} with {body}",
            case.name
        );
        assert_eq!(
            body["ResourceSetIdentifier"].as_str(),
            Some(id.as_str()),
            "case {:?}: the response must name the resource set that was changed, got {body}",
            case.name
        );
        assert_eq!(
            reported_failures(&body),
            expected_failures(&case),
            "case {:?}: expected exactly {:?} to fail, got {body}",
            case.name,
            case.expected_failures
        );
        let remaining = membership(&engine, &id);
        assert_eq!(
            remaining,
            case.expected_membership(),
            "case {:?}: expected the membership {:?}, got {remaining:?}",
            case.name,
            case.expected_membership()
        );

        // Idempotent: the same call again has nothing left to remove, answers the
        // same failures, and leaves the membership where it is.
        let (status, again) = call(
            &router,
            "BatchDisassociateResource",
            &json!({ "ResourceSetIdentifier": id, "Items": case.remove }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the second disassociate must answer 200, got {status} with {again}",
            case.name
        );
        assert_eq!(
            reported_failures(&again),
            expected_failures(&case),
            "case {:?}: the second disassociate must report the same failures, got {again}",
            case.name
        );
        let after = membership(&engine, &id);
        assert_eq!(
            after, remaining,
            "case {:?}: the second disassociate must change nothing, got {after:?}",
            case.name
        );
    }
}

/// One row of the item-classification table: a URI the set does not hold, and
/// the `FailedItemReason` it must be reported with, or `None` when it could be a
/// member and so leaves nothing to remove.
struct ItemCase {
    uri: String,
    reason: Option<&'static str>,
}

/// An item the resource set does not hold is classified against the FIRST field
/// of its ARN that disagrees with the set — the same classification
/// `BatchAssociateResource` admits items by — and a well-formed item the set
/// simply does not hold is no failure at all.
#[tokio::test]
async fn an_unheld_item_is_classified_by_the_first_field_that_disagrees() {
    // The catalog is the model's own `FailedItemReason`
    // (`com.amazonaws.fms#FailedItemReason`), so a reason this suite expects
    // cannot drift from the enum.
    let reasons = service_com_amazonaws_fms_awsfms_20180101::ops::types::FAILED_ITEM_REASON_VALUES;
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let held = instance(0);
    let (id, _) = create_set_with_members(
        &router,
        "classification",
        &["AWS::EC2::Instance"],
        std::slice::from_ref(&held),
    )
    .await;

    // Rows differ in WHICH field of the ARN disagrees, one per reason this
    // service's store can judge.
    let cases = vec![
        ItemCase {
            // Well formed, a type the set declares, and simply not a member.
            uri: instance(42),
            reason: None,
        },
        ItemCase {
            uri: "this-is-not-an-arn-at-all".to_string(),
            reason: Some("NOT_VALID_ARN"),
        },
        ItemCase {
            uri: format!("arn:aws-cn:ec2:{REGION}:{ACCOUNT}:instance/i-1"),
            reason: Some("NOT_VALID_PARTITION"),
        },
        ItemCase {
            uri: format!("arn:aws:ec2:{OTHER_REGION}:{ACCOUNT}:instance/i-1"),
            reason: Some("NOT_VALID_REGION"),
        },
        ItemCase {
            uri: format!("arn:aws:ec2:{REGION}:999999999999:instance/i-1"),
            reason: Some("NOT_VALID_ACCOUNT_ID"),
        },
        ItemCase {
            // A type no entry of the set's `ResourceTypeList` names.
            uri: firewall("edge"),
            reason: Some("NOT_VALID_RESOURCE_TYPE"),
        },
    ];
    for case in &cases {
        if let Some(reason) = case.reason {
            assert!(
                reasons.contains(&reason),
                "case {:?}: {reason:?} is not in the model's FailedItemReason catalog {reasons:?}",
                case.uri
            );
        }
    }

    let items: Vec<&str> = cases.iter().map(|case| case.uri.as_str()).collect();
    let (status, body) = call(
        &router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": items }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchDisassociateResource to answer 200, got {status} with {body}"
    );
    for case in &cases {
        let reported = body["FailedItems"]
            .as_array()
            .expect("FailedItems is a list")
            .iter()
            .find(|item| item["URI"].as_str() == Some(case.uri.as_str()))
            .map(|item| item["Reason"].as_str().unwrap_or_default().to_string());
        assert_eq!(
            reported.as_deref(),
            case.reason,
            "case {:?}: expected reason {:?}, got {reported:?} in {body}",
            case.uri,
            case.reason
        );
    }

    // None of it touched the one resource the set does hold.
    assert_eq!(
        membership(&engine, &id),
        vec![held],
        "a batch of items the set does not hold must leave its membership alone"
    );
}

/// A resource the set HOLDS is removable whatever the set's `ResourceTypeList`
/// says now: the list can be rewritten after the resource was associated, and a
/// member that could no longer be associated must still be disassociable.
#[tokio::test]
async fn a_member_is_removable_after_the_resource_type_list_narrows() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let members = vec![instance(0), firewall("edge")];
    let (id, _) = create_set_with_members(
        &router,
        "narrowing",
        &["AWS::EC2::Instance", "AWS::NetworkFirewall::Firewall"],
        &members,
    )
    .await;

    // Narrow the set's `ResourceTypeList` through its own writer: an update keeps
    // the membership, so the firewall is now a member of a set that no longer
    // declares its type.
    let token = with_store(&engine, |store| store.resource_sets[&id].update_token.clone());
    let (status, updated) = call(
        &router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Id": id,
                "UpdateToken": token,
                "Name": "set-narrowing",
                "ResourceTypeList": ["AWS::EC2::Instance"],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "narrowing the ResourceTypeList must answer 200, got {status} with {updated}"
    );
    assert_eq!(
        membership(&engine, &id),
        {
            let mut sorted = members.clone();
            sorted.sort();
            sorted
        },
        "rewriting the document must not change the membership"
    );

    let (status, body) = call(
        &router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": [firewall("edge")] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchDisassociateResource to answer 200, got {status} with {body}"
    );
    assert_eq!(
        reported_failures(&body),
        Vec::<(String, String)>::new(),
        "a resource the set holds must not be reported as a failure, got {body}"
    );
    assert_eq!(
        membership(&engine, &id),
        vec![instance(0)],
        "the member must be gone even though its type is no longer declared"
    );
}

/// The resource set can be named by its `Id` or by its ARN; an identifier that
/// names no stored set is `ResourceNotFoundException`; and one call may carry
/// only so many items, so a batch one past the bound is refused and removes
/// nothing.
#[tokio::test]
async fn the_identifier_is_resolved_and_the_batch_is_bounded() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let members: Vec<String> = (0..3).map(instance).collect();
    let (id, arn) =
        create_set_with_members(&router, "identifiers", &["AWS::EC2::Instance"], &members).await;

    // Named by its ARN, the call answers with the set's `Id` and removes a member.
    let (status, body) = call(
        &router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": arn, "Items": [instance(0)] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the resource set can be named by its ARN too, got {status} with {body}"
    );
    assert_eq!(
        body["ResourceSetIdentifier"].as_str(),
        Some(id.as_str()),
        "the response must name the set's Id, got {body}"
    );
    assert_eq!(
        membership(&engine, &id),
        vec![instance(1), instance(2)],
        "naming the set by its ARN must remove the named member"
    );

    // Exactly as many items as one call may carry is accepted: none of them is a
    // member, so nothing is removed and nothing fails.
    let at_bound: Vec<String> = (100..200).map(instance).collect();
    assert_eq!(
        at_bound.len(),
        100,
        "the batch at the bound must carry the bound's own count"
    );
    let (status, body) = call(
        &router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": at_bound }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a batch at the bound must be accepted, got {status} with {body}"
    );
    assert_eq!(
        reported_failures(&body),
        Vec::<(String, String)>::new(),
        "items the set could hold but does not are no failure, got {body}"
    );

    // One past the bound is refused before anything is classified.
    let over_bound: Vec<String> = (100..201).map(instance).collect();
    let (status, body) = call(
        &router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": over_bound }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidInputException"),
        "a batch past the bound must be refused, got {status} with {body}"
    );
    assert_eq!(
        membership(&engine, &id),
        vec![instance(1), instance(2)],
        "a refused batch must remove nothing"
    );

    // An identifier that names no resource set is not found — whichever
    // character class the well-formed id is drawn from.
    for unknown in [
        UNKNOWN_RESOURCE_SET_ID,
        "0000000000000000000001",
        "abcdefghijklmnopqrstuv",
        "ABCDEFGHIJKLMNOPQRSTUV",
        &state::resource_set_arn(REGION, UNKNOWN_RESOURCE_SET_ID),
    ] {
        let (status, body) = call(
            &router,
            "BatchDisassociateResource",
            &json!({ "ResourceSetIdentifier": unknown, "Items": [instance(1)] }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {unknown:?}: an unknown identifier must be ResourceNotFoundException, got \
             {status} with {body}"
        );
    }
    assert_eq!(
        membership(&engine, &id),
        vec![instance(1), instance(2)],
        "a refused call must leave the real set's membership alone"
    );
}

/// With no administrator association `BatchDisassociateResource` is refused with
/// HTTP 400 `AccessDeniedException` BEFORE the identifier is looked up and before
/// any item is examined — so a nonexistent identifier and a malformed item both
/// get the authorization refusal, and a refused call removes nothing.
#[tokio::test]
async fn disassociating_is_administrator_gated_and_removes_nothing_when_refused() {
    let (router, engine) = emulator_with_engine();

    // Seeded through the store the handlers share, because with no administrator
    // association `PutResourceSet` is refused too: the point of the test is that
    // the gate is decided without the record being consulted.
    let seeded = "zZ9yY8xX7wW6vV5uU4tT3s";
    let member = instance(0);
    with_store(&engine, |store| {
        let now = std::time::SystemTime::now();
        store.resource_sets.insert(
            seeded.to_string(),
            state::ResourceSetRecord {
                id: seeded.to_string(),
                arn: state::resource_set_arn(REGION, seeded),
                update_token: state::mint_identifier(),
                resource_set: ResourceSet {
                    name: "seeded-set".to_string(),
                    resource_type_list: vec!["AWS::EC2::Instance".to_string()],
                    ..ResourceSet::default()
                },
                tags: Vec::new(),
                resources: BTreeSet::from([member.clone()]),
                created_at: now,
                updated_at: now,
            },
        );
    });

    for (identifier, items) in [
        (seeded, vec![member.clone()]),
        (UNKNOWN_RESOURCE_SET_ID, vec![member.clone()]),
        (seeded, vec!["this-is-not-an-arn-at-all".to_string()]),
    ] {
        let (status, body) = call(
            &router,
            "BatchDisassociateResource",
            &json!({ "ResourceSetIdentifier": identifier, "Items": items }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {identifier:?} with {items:?}: with no administrator the call must be refused \
             before the lookup, got {status} with {body}"
        );
    }
    assert_eq!(
        membership(&engine, seeded),
        vec![member.clone()],
        "a refused call must remove nothing"
    );

    // With an administrator associated the same call acts on the store.
    associate_admin(&router).await;
    let (status, body) = call(
        &router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": seeded, "Items": [member] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "with an administrator the seeded member must disassociate, got {status} with {body}"
    );
    assert!(
        membership(&engine, seeded).is_empty(),
        "the member must be gone, got {:?}",
        membership(&engine, seeded)
    );
}
