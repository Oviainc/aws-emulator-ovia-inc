// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the `ResourceSetStatus` of a stored resource set
//! lets a caller DO with it, as opposed to what it reports.
//!
//! `ResourceSet$ResourceSetStatus` states the rule for both of its values:
//! `ACTIVE` is "the administrator can manage and delete the resource set", and
//! `OUT_OF_ADMIN_SCOPE` is "the administrator can view the resource set, but
//! they can't edit or delete the resource set. Existing protections stay in
//! place". So a stored resource set the administrative scope governing it does
//! not cover is READ-ONLY: rewriting its document (`PutResourceSet` naming its
//! `Id`), changing which resources it holds (`BatchAssociateResource`,
//! `BatchDisassociateResource`) and removing it (`DeleteResourceSet`) are all
//! refused with the modelled `InvalidOperationException`, the stored document,
//! its `UpdateToken` and its membership are left as they were, and the resource
//! set keeps being readable. A resource set that scope DOES cover takes all four
//! calls, exactly as before.
//!
//! This is the resource-set reading of the rule the sibling suite
//! [`crate::policy_mutation_admin_scope`] asserts for a policy, so the
//! administrative-scope table it drives
//! ([`crate::policy_status_derivation::scope_cases`]) is reused rather than
//! copied; a resource set's status follows the REGION scope alone (it carries no
//! policy type), which is why each row's expected status is derived from that
//! dimension of the row rather than written down.
//!
//! The resource-set rows below vary what the rule generalises over that the
//! scope rows do not: how many resource types the set declares, whether it
//! carries a `Description` and how long its `Name` is (the model's own bounds
//! for `ResourceSet`: `Name` 1..128, `Description` 0..256), and how many
//! resources one batch call names.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router, reusing the request harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::policy_lifecycle::{call_in, emulator_with_administrator, error_code, ACCOUNT, REGION};
use crate::policy_status_derivation::{
    admin_scope, emulator_with_another_default_administrator, put_admin_scope, scope_cases,
    OTHER_REGION,
};

/// The `ResourceSetStatus` of a resource set a governing administrative scope
/// does not cover.
const OUT_OF_ADMIN_SCOPE: &str = "OUT_OF_ADMIN_SCOPE";

/// The `ResourceSetStatus` of one it covers.
const ACTIVE: &str = "ACTIVE";

/// The modelled error a refused edit or delete of a view-only resource set
/// answers.
const REFUSAL: &str = "InvalidOperationException";

/// An EC2 instance ARN of this account in `region`, one per index.
fn instance(region: &str, index: usize) -> String {
    format!("arn:aws:ec2:{region}:{ACCOUNT}:instance/i-{index:017x}")
}

/// A Network Firewall ARN of this account in `region`.
fn firewall(region: &str, name: &str) -> String {
    format!("arn:aws:network-firewall:{region}:{ACCOUNT}:firewall/{name}")
}

/// An Application Load Balancer ARN of this account in `region`.
fn load_balancer(region: &str, name: &str) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{region}:{ACCOUNT}:loadbalancer/app/{name}/50dc6c495c0c9188"
    )
}

/// One row of the resource-set table: a name for the assertion messages, the
/// `ResourceSet` document the create sends, and resources of the types that
/// document declares, so a batch call on an in-scope set really does change the
/// membership.
struct Case {
    name: &'static str,
    document: fn() -> Value,
    items: fn(&str) -> Vec<String>,
}

/// The resource-set rows every rule below is asserted on.
///
/// The `ResourceTypeList` entries are resource types the emulator's own
/// `DISCOVERABLE_RESOURCE_TYPES` catalog carries, each paired with an ARN of
/// that type so `BatchAssociateResource` admits it.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The smallest a resource set can be: a one-character name, one
            // declared type, no description — and a batch of one resource.
            name: "one-type-shortest-name",
            document: || {
                json!({
                    "Name": "a",
                    "ResourceTypeList": ["AWS::EC2::Instance"],
                })
            },
            items: |region| vec![instance(region, 0)],
        },
        Case {
            // An empty description is inside the modelled 0..256 and is not the
            // same thing as an absent one; two declared types, two resources.
            name: "two-types-empty-description",
            document: || {
                json!({
                    "Name": "two-types-empty-description",
                    "Description": "",
                    "ResourceTypeList": [
                        "AWS::EC2::Instance",
                        "AWS::NetworkFirewall::Firewall",
                    ],
                })
            },
            items: |region| vec![instance(region, 1), firewall(region, "edge")],
        },
        Case {
            // The upper published bound of `Name`, three declared types and a
            // batch of three resources.
            name: "three-types-longest-name",
            document: || {
                json!({
                    "Name": "n".repeat(128),
                    "Description": "d".repeat(256),
                    "ResourceTypeList": [
                        "AWS::EC2::Instance",
                        "AWS::NetworkFirewall::Firewall",
                        "AWS::ElasticLoadBalancingV2::LoadBalancer",
                    ],
                })
            },
            items: |region| {
                vec![
                    instance(region, 2),
                    firewall(region, "core"),
                    load_balancer(region, "front"),
                ]
            },
        },
    ]
}

/// One stored resource set, as the test needs to address it afterwards.
struct Stored {
    id: String,
    update_token: String,
    document: Value,
}

/// Create the row's resource set in `region` and answer what the test needs to
/// address it.
async fn create(router: &MultiServiceRouter, row: &str, case: &Case, region: &str) -> Stored {
    let (status, body) = call_in(
        router,
        "PutResourceSet",
        &json!({ "ResourceSet": (case.document)() }),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected PutResourceSet to create the resource set with 200, got {status} \
         with {body}"
    );
    let stored = body["ResourceSet"].clone();
    Stored {
        id: stored["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: no minted Id in {stored}"))
            .to_string(),
        update_token: stored["UpdateToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {row:?}: no minted UpdateToken in {stored}"))
            .to_string(),
        document: stored,
    }
}

/// An edit of `stored`: the same document under the same (unchangeable) name,
/// carrying the set's id, the token it was last reported with and a new
/// `Description`.
fn edit(case: &Case, stored: &Stored, description: &str) -> Value {
    let mut document = (case.document)();
    document["Id"] = json!(stored.id);
    document["UpdateToken"] = json!(stored.update_token);
    document["Description"] = json!(description);
    json!({ "ResourceSet": document })
}

/// The whole stored `ResourceSet` `GetResourceSet` reports, which a refused
/// mutation must leave untouched.
async fn read(router: &MultiServiceRouter, row: &str, id: &str, region: &str) -> Value {
    let (status, body) = call_in(router, "GetResourceSet", &json!({ "Identifier": id }), region)
        .await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected GetResourceSet to answer 200, got {status} with {body}"
    );
    body["ResourceSet"].clone()
}

/// The URIs `ListResourceSetResources` reports as one set's membership.
async fn membership(router: &MultiServiceRouter, row: &str, id: &str, region: &str) -> Vec<String> {
    let (status, body) = call_in(
        router,
        "ListResourceSetResources",
        &json!({ "Identifier": id }),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected ListResourceSetResources to answer 200, got {status} with {body}"
    );
    let mut uris: Vec<String> = body["Items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {row:?}: no Items in {body}"))
        .iter()
        .filter_map(|item| item["URI"].as_str().map(str::to_string))
        .collect();
    uris.sort();
    uris
}

/// Assert that one call is refused as a mutation of a view-only resource set.
async fn assert_refused(
    router: &MultiServiceRouter,
    row: &str,
    operation: &str,
    request: &Value,
    region: &str,
) {
    let (status, body) = call_in(router, operation, request, region).await;
    assert_eq!(
        status, 400,
        "case {row:?}: expected {operation} on a view-only resource set to be refused with 400, \
         got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        REFUSAL,
        "case {row:?}: expected {operation} to answer the refusal {REFUSAL}, got {body}"
    );
}

/// Drive every mutation of one resource set that the rule covers and assert it
/// SUCCEEDS: the batch calls change the membership, the edit is persisted and
/// rotates the token, and the delete makes the set unreadable.
async fn assert_every_mutation_succeeds(
    router: &MultiServiceRouter,
    row: &str,
    case: &Case,
    stored: &Stored,
    region: &str,
) {
    let items = (case.items)(region);
    let (status, body) = call_in(
        router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": stored.id, "Items": items }),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected BatchAssociateResource on an in-scope resource set to answer 200, \
         got {status} with {body}"
    );
    assert_eq!(
        body["FailedItems"],
        json!([]),
        "case {row:?}: every item of the batch must associate, got {body}"
    );
    let mut expected = items.clone();
    expected.sort();
    assert_eq!(
        membership(router, row, &stored.id, region).await,
        expected,
        "case {row:?}: expected the associated resources to be the set's membership"
    );

    let (status, body) = call_in(
        router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": stored.id, "Items": [items[0].clone()] }),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected BatchDisassociateResource on an in-scope resource set to answer \
         200, got {status} with {body}"
    );
    let left: Vec<String> = expected
        .iter()
        .filter(|uri| uri.as_str() != items[0].as_str())
        .cloned()
        .collect();
    assert_eq!(
        membership(router, row, &stored.id, region).await,
        left,
        "case {row:?}: expected the disassociated resource to be gone from the membership"
    );

    let (status, body) = call_in(
        router,
        "PutResourceSet",
        &edit(case, stored, "edited"),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected the edit of an in-scope resource set to answer 200, got {status} \
         with {body}"
    );
    assert_eq!(
        body["ResourceSet"]["Description"],
        json!("edited"),
        "case {row:?}: expected the edit to be reported, got {}",
        body["ResourceSet"]
    );
    assert_ne!(
        body["ResourceSet"]["UpdateToken"],
        json!(stored.update_token),
        "case {row:?}: expected a successful edit to rotate the UpdateToken, got {}",
        body["ResourceSet"]
    );
    let after = read(router, row, &stored.id, region).await;
    assert_eq!(
        after["Description"],
        json!("edited"),
        "case {row:?}: expected the edit to be persisted, got {after}"
    );

    let (status, body) = call_in(
        router,
        "DeleteResourceSet",
        &json!({ "Identifier": stored.id }),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected the delete of an in-scope resource set to answer 200, got {status} \
         with {body}"
    );
    let (status, body) = call_in(
        router,
        "GetResourceSet",
        &json!({ "Identifier": stored.id }),
        region,
    )
    .await;
    assert_eq!(
        status, 400,
        "case {row:?}: expected the deleted resource set to be gone, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {row:?}: expected the deleted resource set to be not found, got {body}"
    );
}

/// A view-only resource set refuses every edit and the delete, keeps its
/// document, its token and its membership, and stays readable — and a resource
/// set the governing scope covers takes all four calls.
#[tokio::test]
async fn an_out_of_scope_resource_set_refuses_every_edit_and_the_delete() {
    for scope_case in scope_cases() {
        let router = emulator_with_another_default_administrator().await;
        put_admin_scope(&router, scope_case.name, &scope_case.scope).await;

        for case in cases() {
            for region in [REGION, OTHER_REGION] {
                let row = format!("{}/{}/{region}", scope_case.name, case.name);
                // Only the Region scope decides a resource set's status: a set
                // in a Region the scope covers is ACTIVE even when that scope
                // manages no policy type at all.
                let want = if (scope_case.manages_region)(region) {
                    ACTIVE
                } else {
                    OUT_OF_ADMIN_SCOPE
                };
                let stored = create(&router, &row, &case, region).await;
                let reported = read(&router, &row, &stored.id, region).await;
                assert_eq!(
                    reported["ResourceSetStatus"],
                    json!(want),
                    "case {row:?}: expected the stored resource set to be reported {want}, got \
                     {reported}"
                );

                if want == ACTIVE {
                    assert_every_mutation_succeeds(&router, &row, &case, &stored, region).await;
                    continue;
                }

                let items = (case.items)(region);
                for (operation, request) in [
                    ("PutResourceSet", edit(&case, &stored, "edited")),
                    (
                        "BatchAssociateResource",
                        json!({ "ResourceSetIdentifier": stored.id, "Items": items }),
                    ),
                    (
                        "BatchDisassociateResource",
                        json!({ "ResourceSetIdentifier": stored.id, "Items": items }),
                    ),
                    ("DeleteResourceSet", json!({ "Identifier": stored.id })),
                ] {
                    assert_refused(&router, &row, operation, &request, region).await;
                }

                // Nothing was written: the document the create reported is
                // still what every reader sees, the token the caller already
                // holds is still current, the membership is still empty, and
                // the resource set is still readable.
                let after = read(&router, &row, &stored.id, region).await;
                for member in ["Id", "Name", "Description", "ResourceTypeList", "UpdateToken"] {
                    assert_eq!(
                        after[member], stored.document[member],
                        "case {row:?}: a refused mutation must leave {member} alone, got {after}"
                    );
                }
                assert_eq!(
                    membership(&router, &row, &stored.id, region).await,
                    Vec::<String>::new(),
                    "case {row:?}: a refused BatchAssociateResource must associate nothing"
                );
            }
        }
    }
}

/// Whether a resource set may be edited is decided per CALL from the scope that
/// governs it, so narrowing the administrator's scope makes a resource set
/// view-only and widening it makes the very same resource set editable and
/// deletable again — with the token the refused edit was not allowed to rotate.
#[tokio::test]
async fn widening_the_administrative_scope_makes_the_resource_set_editable_again() {
    let router = emulator_with_another_default_administrator().await;
    let case = cases().remove(0);
    let stored = create(&router, "initially-in-scope", &case, REGION).await;

    // Narrowed to another Region, so the resource set is out of the caller's
    // scope.
    put_admin_scope(
        &router,
        "narrowed",
        &admin_scope(
            json!({ "AllPolicyTypesEnabled": true }),
            json!({ "Regions": [OTHER_REGION] }),
        ),
    )
    .await;
    let items = (case.items)(REGION);
    for (operation, request) in [
        ("PutResourceSet", edit(&case, &stored, "edited")),
        (
            "BatchAssociateResource",
            json!({ "ResourceSetIdentifier": stored.id, "Items": items }),
        ),
        (
            "BatchDisassociateResource",
            json!({ "ResourceSetIdentifier": stored.id, "Items": items }),
        ),
        ("DeleteResourceSet", json!({ "Identifier": stored.id })),
    ] {
        assert_refused(&router, "narrowed", operation, &request, REGION).await;
    }

    // Widened again: the same resource set, never touched in between, takes
    // every call the narrowed scope refused — with the token it still carries.
    put_admin_scope(
        &router,
        "widened",
        &admin_scope(
            json!({ "AllPolicyTypesEnabled": true }),
            json!({ "AllRegionsEnabled": true }),
        ),
    )
    .await;
    assert_every_mutation_succeeds(&router, "widened", &case, &stored, REGION).await;
}

/// The ordinary single-administrator organisation is untouched: with no
/// delegated administrator configured for the caller, every resource set is in
/// the default administrator's full scope, so every row is editable and
/// deletable in every Region.
#[tokio::test]
async fn the_default_administrator_edits_and_deletes_every_resource_set() {
    let router = emulator_with_administrator().await;
    for case in cases() {
        for region in [REGION, OTHER_REGION] {
            let row = format!("{}/{region}", case.name);
            let stored = create(&router, &row, &case, region).await;
            let reported = read(&router, &row, &stored.id, region).await;
            assert_eq!(
                reported["ResourceSetStatus"],
                json!(ACTIVE),
                "case {row:?}: the default administrator's resource sets are ACTIVE, got {reported}"
            );
            assert_every_mutation_succeeds(&router, &row, &case, &stored, region).await;
        }
    }
}
