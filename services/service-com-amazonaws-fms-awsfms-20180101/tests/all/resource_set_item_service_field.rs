// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SERVICE field of a resource-set item's ARN, through
//! both batch membership operations.
//!
//! RULE under test: a resource type names both the kind of resource and the one
//! service that owns resources of that kind, and every ARN of such a resource
//! carries that service's namespace in its service field — so an item may become
//! a member of a resource set only where ONE entry of the set's
//! `ResourceTypeList` answers for both the item's resource field and its service
//! field. An item whose resource field names a declared type under the namespace
//! of a service that does not own that type names a resource nothing holds: it is
//! refused with the modelled `NOT_VALID_SERVICE` and is not associated, while an
//! item whose resource field names no declared type at all is refused on the
//! broader disagreement, `NOT_VALID_RESOURCE_TYPE`.
//!
//! [`an_item_is_refused_when_its_service_field_does_not_own_the_declared_type`]
//! asserts the rule as a GENERATED table: one row per owning service and ARN
//! form, each row associated under its own namespace and refused under every
//! OTHER namespace of the table — including one that owns no type the set names
//! at all — so the only thing that differs between an accepted and a refused item
//! is the service field. The hand-written rows beside it pin that the earlier ARN
//! fields are still weighed first, and that a type no declared entry names is
//! still the resource-type reason rather than this one.
//!
//! [`every_discoverable_resource_type_refuses_a_foreign_services_arn`] walks the
//! whole catalogue of resource types this emulator can discover, so the rule is
//! asserted on every type rather than on the few the table spells ARNs for.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router, reusing the request harness in [`crate::policy_lifecycle`]
//! — so the assertions are about the WIRE response, and what each call wrote is
//! read back through the modelled reader (`ListResourceSetResources`).

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;

use crate::administrator_gated_operations::associate_admin;
use crate::policy_lifecycle::{call, emulator_with_engine, ACCOUNT, REGION};
use crate::resource_set_membership_last_update_time::{create_set_with_members, reported_membership};

/// The `FailedItemReason` an item whose service field does not own the declared
/// type must be reported with (`com.amazonaws.fms#FailedItemReason`).
const NOT_VALID_SERVICE: &str = "NOT_VALID_SERVICE";

/// A Region that is not the one these requests are signed for.
const OTHER_REGION: &str = "eu-central-1";

/// An account id outside this organization.
const FOREIGN_ACCOUNT: &str = "999999999999";

/// The namespace of a service that owns NO resource type a Firewall Manager
/// resource set holds, so no item of this emulator's inventory ever carries it.
const FOREIGN_NAMESPACE: &str = "iam";

/// One owning service and ARN form: a resource type, the namespace the service
/// that owns resources of that type publishes, and the Region, account and
/// resource fields that service writes into such an ARN.
///
/// SOURCE of each namespace and resource field: the ARN the owning service
/// itself mints, which is what this emulator's discoverable inventory reports
/// (`AWS::EC2::Instance` -> `arn:aws:ec2:<region>:<account>:instance/<id>`,
/// `AWS::EC2::EIP` -> an allocation's `elastic-ip/<id>`,
/// `AWS::CloudFront::Distribution` -> a GLOBAL ARN with no Region,
/// `AWS::ApiGateway::Stage` -> an account-less ARN naming its type inside a
/// path, `AWS::NetworkFirewall::Firewall` -> the punctuated `network-firewall`
/// namespace, `AWS::ElasticLoadBalancingV2::LoadBalancer` -> the
/// `elasticloadbalancing` namespace Elastic Load Balancing serves BOTH load
/// balancer generations under).
struct Owned {
    resource_type: &'static str,
    namespace: &'static str,
    region: &'static str,
    account: &'static str,
    resource: &'static str,
}

impl Owned {
    /// This row's ARN with `namespace` in its service field, every other field
    /// left exactly as the owning service writes it — so two ARNs built from one
    /// row differ in the service field and in nothing else.
    fn arn_under(&self, namespace: &str) -> String {
        format!(
            "arn:aws:{namespace}:{}:{}:{}",
            self.region, self.account, self.resource
        )
    }
}

/// The owning services and ARN forms the rule is asserted over. The rows differ
/// in the owning service, in whether its namespace is spelled the way the
/// resource type's own name spells it (`ec2` for `AWS::EC2::Instance`), with
/// punctuation (`network-firewall`), or differently altogether
/// (`elasticloadbalancing` for `AWS::ElasticLoadBalancingV2::LoadBalancer`), in
/// which ARN field that service leaves empty (CloudFront's Region, API Gateway's
/// account, none of Amazon EC2's), in how the resource field spells the type (a
/// leading segment, a keyword of the service's own, a plural segment inside a
/// path) and in the depth of that field.
fn owners() -> Vec<Owned> {
    vec![
        Owned {
            resource_type: state::RESOURCE_TYPE_EC2_INSTANCE,
            namespace: "ec2",
            region: REGION,
            account: ACCOUNT,
            resource: "instance/i-0123456789abcdef0",
        },
        Owned {
            resource_type: state::RESOURCE_TYPE_EC2_EIP,
            namespace: "ec2",
            region: REGION,
            account: ACCOUNT,
            resource: "elastic-ip/eipalloc-0123456789abcdef0",
        },
        Owned {
            resource_type: state::RESOURCE_TYPE_CLOUDFRONT_DISTRIBUTION,
            namespace: "cloudfront",
            region: "",
            account: ACCOUNT,
            resource: "distribution/E1EXAMPLE1EXAM",
        },
        Owned {
            resource_type: state::RESOURCE_TYPE_APIGATEWAY_STAGE,
            namespace: "apigateway",
            region: REGION,
            account: "",
            resource: "/restapis/a1b2c3d4e5/stages/prod",
        },
        Owned {
            resource_type: state::RESOURCE_TYPE_NETWORK_FIREWALL_FIREWALL,
            namespace: "network-firewall",
            region: REGION,
            account: ACCOUNT,
            resource: "firewall/edge",
        },
        Owned {
            resource_type: state::RESOURCE_TYPE_ELBV2_LOAD_BALANCER,
            namespace: "elasticloadbalancing",
            region: REGION,
            account: ACCOUNT,
            resource: "loadbalancer/app/edge/0123456789abcdef",
        },
    ]
}

/// One item a batch call carries: the URI and the `FailedItemReason` both
/// operations must report it with, or `None` when the set can hold it.
struct Item {
    case: String,
    uri: String,
    reason: Option<&'static str>,
}

/// The items the generated table sends: every row's own ARN (which the set
/// holds), and that row's resource field under every other namespace of the
/// table plus [`FOREIGN_NAMESPACE`] (which the set cannot hold, on the service
/// field alone).
fn service_field_items() -> Vec<Item> {
    let owners = owners();
    let mut namespaces: Vec<&'static str> = owners.iter().map(|owner| owner.namespace).collect();
    namespaces.push(FOREIGN_NAMESPACE);
    namespaces.sort_unstable();
    namespaces.dedup();

    let mut items = Vec::new();
    for owner in &owners {
        items.push(Item {
            case: format!("{} under {}", owner.resource_type, owner.namespace),
            uri: owner.arn_under(owner.namespace),
            reason: None,
        });
        for namespace in &namespaces {
            if *namespace == owner.namespace {
                continue;
            }
            items.push(Item {
                case: format!("{} under {namespace}", owner.resource_type),
                uri: owner.arn_under(namespace),
                reason: Some(NOT_VALID_SERVICE),
            });
        }
    }
    items
}

/// The items that pin the REST of the classification beside the service field:
/// the earlier ARN fields are still reported first, and a resource field no
/// declared type names is still the resource-type reason rather than the service
/// one.
fn other_field_items() -> Vec<Item> {
    vec![
        Item {
            case: "a type no declared entry names, under its owning service".to_string(),
            uri: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:natgateway/nat-0123456789abcdef0"),
            reason: Some("NOT_VALID_RESOURCE_TYPE"),
        },
        Item {
            case: "a type no declared entry names, under a foreign service".to_string(),
            uri: format!("arn:aws:{FOREIGN_NAMESPACE}::{ACCOUNT}:role/example"),
            reason: Some("NOT_VALID_RESOURCE_TYPE"),
        },
        Item {
            case: "another partition, beside a foreign service".to_string(),
            uri: format!("arn:aws-cn:{FOREIGN_NAMESPACE}:{REGION}:{ACCOUNT}:instance/i-1"),
            reason: Some("NOT_VALID_PARTITION"),
        },
        Item {
            case: "another Region, beside a foreign service".to_string(),
            uri: format!("arn:aws:{FOREIGN_NAMESPACE}:{OTHER_REGION}:{ACCOUNT}:instance/i-1"),
            reason: Some("NOT_VALID_REGION"),
        },
        Item {
            case: "an account outside the organization, beside a foreign service".to_string(),
            uri: format!("arn:aws:{FOREIGN_NAMESPACE}:{REGION}:{FOREIGN_ACCOUNT}:instance/i-1"),
            reason: Some("NOT_VALID_ACCOUNT_ID"),
        },
        Item {
            case: "no ARN at all".to_string(),
            uri: "ec2:instance/i-0123456789abcdef0".to_string(),
            reason: Some("NOT_VALID_ARN"),
        },
    ]
}

/// The `URI -> Reason` the `FailedItems` of one response reports, looked up per
/// item.
fn reported_reason(body: &Value, uri: &str) -> Option<String> {
    body["FailedItems"]
        .as_array()
        .unwrap_or_else(|| panic!("FailedItems must be a list, got {body}"))
        .iter()
        .find(|item| item["URI"].as_str() == Some(uri))
        .map(|item| item["Reason"].as_str().unwrap_or_default().to_string())
}

/// An item is associable only where one declared resource type answers for both
/// its resource field and its service field: the same resource field under
/// another service's namespace is refused with `NOT_VALID_SERVICE` and is not
/// stored, whatever else about the ARN agrees. Asserted on BOTH batch operations,
/// which share the classifier.
#[tokio::test]
async fn an_item_is_refused_when_its_service_field_does_not_own_the_declared_type() {
    let reasons = service_com_amazonaws_fms_awsfms_20180101::ops::types::FAILED_ITEM_REASON_VALUES;
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    let types: Vec<&str> = owners().iter().map(|owner| owner.resource_type).collect();
    for resource_type in &types {
        assert!(
            state::DISCOVERABLE_RESOURCE_TYPES.contains(resource_type),
            "case {resource_type:?}: the table must name a resource type this emulator can \
             discover, which {:?} does not list",
            state::DISCOVERABLE_RESOURCE_TYPES
        );
    }
    let id = create_set_with_members(&router, "service-field", &types, &[]).await;

    let mut items = service_field_items();
    items.extend(other_field_items());
    assert!(
        items.len() <= state::MAX_RESOURCE_SET_ITEMS_PER_CALL,
        "the generated table must fit in ONE call of at most {} items, got {}",
        state::MAX_RESOURCE_SET_ITEMS_PER_CALL,
        items.len()
    );
    for item in &items {
        if let Some(reason) = item.reason {
            assert!(
                reasons.contains(&reason),
                "case {:?}: {reason:?} is not in the model's FailedItemReason catalog {reasons:?}",
                item.case
            );
        }
    }
    assert!(
        items
            .iter()
            .any(|item| item.reason == Some(NOT_VALID_SERVICE)),
        "the table must span the service field, and no row expects {NOT_VALID_SERVICE}"
    );

    let uris: Vec<&str> = items.iter().map(|item| item.uri.as_str()).collect();
    let (status, associated) = call(
        &router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": uris }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchAssociateResource to answer 200, got {status} with {associated}"
    );
    for item in &items {
        let reported = reported_reason(&associated, &item.uri);
        assert_eq!(
            reported.as_deref(),
            item.reason,
            "case {:?}: expected BatchAssociateResource to report {:?} for {:?}, got {reported:?} \
             in {associated}",
            item.case,
            item.reason,
            item.uri
        );
    }

    // What was classified is what was stored: the refused items are no members.
    let mut expected: Vec<String> = items
        .iter()
        .filter(|item| item.reason.is_none())
        .map(|item| item.uri.clone())
        .collect();
    expected.sort();
    let held = reported_membership(&router, "service-field", &id).await;
    assert_eq!(
        held, expected,
        "expected the membership {expected:?}, got {held:?}"
    );

    // `BatchDisassociateResource` reads the SAME classifier, so every item is
    // classified identically there and the members leave the set.
    let (status, disassociated) = call(
        &router,
        "BatchDisassociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": uris }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchDisassociateResource to answer 200, got {status} with {disassociated}"
    );
    for item in &items {
        let reported = reported_reason(&disassociated, &item.uri);
        assert_eq!(
            reported.as_deref(),
            item.reason,
            "case {:?}: both batch operations must classify an item alike, expected {:?}, got \
             {reported:?} in {disassociated}",
            item.case,
            item.reason
        );
    }
    let left = reported_membership(&router, "service-field", &id).await;
    assert_eq!(
        left,
        Vec::<String>::new(),
        "disassociating the whole batch must leave the set empty, got {left:?}"
    );
}

/// The rule holds for EVERY resource type this emulator can discover, not only
/// for the ones the table above spells an owning service's ARN for: a set
/// declaring one type refuses an ARN that names that very type under the
/// namespace of a service owning no type of the catalogue.
///
/// The catalogue is the crate's own [`state::DISCOVERABLE_RESOURCE_TYPES`], so no
/// type can be added there without this assertion covering it, and the resource
/// field is built from the type's own `AWS::<Service>::<Type>` name so the item
/// really names the declared type.
#[tokio::test]
async fn every_discoverable_resource_type_refuses_a_foreign_services_arn() {
    for (index, resource_type) in state::DISCOVERABLE_RESOURCE_TYPES.iter().enumerate() {
        let (router, _engine) = emulator_with_engine();
        associate_admin(&router).await;
        let id = create_set_with_members(
            &router,
            &format!("foreign-service-{index}"),
            &[resource_type],
            &[],
        )
        .await;

        let type_name = resource_type
            .rsplit("::")
            .next()
            .unwrap_or(resource_type)
            .to_ascii_lowercase();
        let uri =
            format!("arn:aws:{FOREIGN_NAMESPACE}:{REGION}:{ACCOUNT}:{type_name}/resource-{index}");
        let (status, body) = call(
            &router,
            "BatchAssociateResource",
            &json!({ "ResourceSetIdentifier": id, "Items": [uri.clone()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {resource_type:?}: expected BatchAssociateResource to answer 200, got {status} \
             with {body}"
        );
        let reported = reported_reason(&body, &uri);
        assert_eq!(
            reported.as_deref(),
            Some(NOT_VALID_SERVICE),
            "case {resource_type:?}: {uri:?} names the declared type under a service that does \
             not own it, so the reason must be {NOT_VALID_SERVICE}, got {reported:?} in {body}"
        );
        let held = reported_membership(&router, resource_type, &id).await;
        assert_eq!(
            held,
            Vec::<String>::new(),
            "case {resource_type:?}: a refused item must not be stored, got {held:?}"
        );
    }
}
