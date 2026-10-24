// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `VpcLink` create-and-read path.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rules these cases assert:
//!
//! * **`CreateVpcLink` persists a VPC link for the account and answers 201 with the
//!   `VpcLinkId` it minted**, reporting the `Name`, `SubnetIds`, `SecurityGroupIds` and
//!   `Tags` the record holds; `GetVpcLink` reports that same record, member for member.
//! * **Every EC2 resource id a request names must have that resource type's id form** —
//!   the type's prefix (`subnet-`, `sg-`) followed by 8 or 17 lowercase hexadecimal
//!   characters — and a request naming a value that does not, or naming no subnet at
//!   all, is refused 400 `BadRequestException` with nothing persisted.
//! * **A created VPC link reports `PENDING` until its provisioning span has passed and
//!   `AVAILABLE` after it**: a transition the clock owes, so reading it neither moves it
//!   nor holds it back.
//! * **The account holds at most `MAX_VPC_LINKS` VPC links**, after which a create is
//!   refused 429 `TooManyRequestsException`.
//!
//! The rows vary what each rule generalises over: the number of subnets and security
//! groups, which of the two published EC2 id lengths each id uses, which optional
//! members the request carries, the name's length, and — for the refusals — the way the
//! id departs from the form (prefix, length, alphabet, case, position in the list). The
//! recorded cases that filed the issue — a create refused for an invalid subnet, and a
//! link whose provisioning the server manages — are one row of the refusal table and the
//! provisioning test.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    EC2_ID_HEX_LENGTHS, MAX_VPC_LINKS, SECURITY_GROUP_ID_PREFIX, SUBNET_ID_PREFIX,
    VPC_LINK_CREATE_SPAN, VPC_LINK_STATUS_AVAILABLE, VPC_LINK_STATUS_PENDING, VPC_LINK_VERSION_V2,
};
use service_com_amazonaws_apigatewayv2_apigatewayv2::ComAmazonawsApigatewayv2Apigatewayv2State;
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, assert_created_now, assert_minted_id, call, emulator, error_code};

/// The collection URI `CreateVpcLink` is bound to (`POST /v2/vpclinks`).
const VPC_LINKS: &str = "/v2/vpclinks";

/// A fresh emulator holding only this service, plus the engine it was built from, so a
/// test can assert on the store itself — what a refused create must NOT have written.
///
/// [`emulator`] is the same emulator without the engine handle; the suites that do not
/// read the store use that one.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// How many VPC links the store holds.
fn stored_links(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<ComAmazonawsApigatewayv2Apigatewayv2State>()
        .lock()
        .vpc_links
        .len()
}

/// One accepted-create row.
struct Accepted {
    case: &'static str,
    request: Value,
    /// The `SubnetIds` the stored link must report, in this order.
    subnet_ids: Value,
    /// The `SecurityGroupIds` the stored link must report, in this order.
    security_group_ids: Value,
}

fn accepted_rows() -> Vec<Accepted> {
    vec![
        Accepted {
            case: "only the required members: one subnet of the 8-character id form",
            request: json!({ "name": "minimal", "subnetIds": ["subnet-0a1b2c3d"] }),
            subnet_ids: json!(["subnet-0a1b2c3d"]),
            // Named by no request, so the modelled list is reported empty rather than
            // invented.
            security_group_ids: json!([]),
        },
        Accepted {
            case: "one subnet of the 17-character id form and one security group",
            request: json!({
                "name": "long-form-ids",
                "subnetIds": ["subnet-0123456789abcdef0"],
                "securityGroupIds": ["sg-0123456789abcdef0"]
            }),
            subnet_ids: json!(["subnet-0123456789abcdef0"]),
            security_group_ids: json!(["sg-0123456789abcdef0"]),
        },
        Accepted {
            case: "three subnets mixing both id forms, two security groups, and tags",
            request: json!({
                "name": "multi-az",
                "subnetIds": [
                    "subnet-0a1b2c3d",
                    "subnet-00112233445566778",
                    "subnet-ffffffffffffffff0"
                ],
                "securityGroupIds": ["sg-0a1b2c3d", "sg-00112233445566778"],
                "tags": { "team": "platform", "env": "test" }
            }),
            subnet_ids: json!([
                "subnet-0a1b2c3d",
                "subnet-00112233445566778",
                "subnet-ffffffffffffffff0"
            ]),
            security_group_ids: json!(["sg-0a1b2c3d", "sg-00112233445566778"]),
        },
        Accepted {
            case: "an explicitly empty security group list",
            request: json!({
                "name": "no-security-groups",
                "subnetIds": ["subnet-0a1b2c3d"],
                "securityGroupIds": []
            }),
            subnet_ids: json!(["subnet-0a1b2c3d"]),
            security_group_ids: json!([]),
        },
        Accepted {
            // `Name` is a `StringWithLengthBetween1And128`, so 128 characters is its
            // published upper bound and is accepted.
            case: "a name at the modelled 128-character bound",
            request: json!({
                "name": "n".repeat(128),
                "subnetIds": ["subnet-0a1b2c3d"]
            }),
            subnet_ids: json!(["subnet-0a1b2c3d"]),
            security_group_ids: json!([]),
        },
    ]
}

/// One refused-create row: the request, and why no VPC link could be built from it.
struct Refused {
    case: &'static str,
    request: Value,
}

fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "no subnet at all",
            request: json!({ "name": "no-subnets", "subnetIds": [] }),
        },
        Refused {
            case: "a subnet id that is not an EC2 resource id",
            request: json!({ "name": "bad-subnet", "subnetIds": ["not-a-subnet"] }),
        },
        Refused {
            case: "a subnet id carrying another resource type's prefix",
            request: json!({ "name": "vpc-for-subnet", "subnetIds": ["vpc-0a1b2c3d"] }),
        },
        Refused {
            case: "a subnet id one character short of the 8-character form",
            request: json!({ "name": "short-subnet", "subnetIds": ["subnet-0a1b2c3"] }),
        },
        Refused {
            case: "a subnet id of 16 characters, between the two published lengths",
            request: json!({
                "name": "between-forms",
                "subnetIds": ["subnet-0123456789abcdef"]
            }),
        },
        Refused {
            case: "a subnet id one character past the 17-character form",
            request: json!({
                "name": "long-subnet",
                "subnetIds": ["subnet-0123456789abcdef01"]
            }),
        },
        Refused {
            case: "a subnet id written in uppercase hexadecimal",
            request: json!({ "name": "upper-subnet", "subnetIds": ["subnet-0A1B2C3D"] }),
        },
        Refused {
            case: "a subnet id with a character outside the hexadecimal alphabet",
            request: json!({ "name": "nonhex-subnet", "subnetIds": ["subnet-0a1b2c3g"] }),
        },
        Refused {
            case: "a well-formed subnet id followed by a malformed one",
            request: json!({
                "name": "second-subnet-bad",
                "subnetIds": ["subnet-0a1b2c3d", "subnet-nope"]
            }),
        },
        Refused {
            case: "a security group id that is not an EC2 resource id",
            request: json!({
                "name": "bad-security-group",
                "subnetIds": ["subnet-0a1b2c3d"],
                "securityGroupIds": ["default"]
            }),
        },
        Refused {
            case: "a security group id carrying the subnet prefix",
            request: json!({
                "name": "subnet-for-security-group",
                "subnetIds": ["subnet-0a1b2c3d"],
                "securityGroupIds": ["subnet-0a1b2c3d"]
            }),
        },
        Refused {
            case: "a security group id one character past the 17-character form",
            request: json!({
                "name": "long-security-group",
                "subnetIds": ["subnet-0a1b2c3d"],
                "securityGroupIds": ["sg-0123456789abcdef01"]
            }),
        },
    ]
}

#[tokio::test]
async fn create_vpc_link_persists_the_record_get_vpc_link_reports_back() {
    let emulator = emulator();

    for row in accepted_rows() {
        let case = row.case;
        let (status, created) = call(&emulator, "POST", VPC_LINKS, Some(row.request.clone())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateVpcLink, got {status} {created}"
        );

        let vpc_link_id = as_str(&created, "vpcLinkId");
        assert_minted_id(case, "VpcLinkId", &vpc_link_id);
        assert_created_now(case, &created);

        assert_eq!(
            created.get("name"),
            row.request.get("name"),
            "case {case:?}: expected the Name the request named, got {created}"
        );
        assert_eq!(
            created.get("subnetIds"),
            Some(&row.subnet_ids),
            "case {case:?}: expected SubnetIds {}, got {created}",
            row.subnet_ids
        );
        assert_eq!(
            created.get("securityGroupIds"),
            Some(&row.security_group_ids),
            "case {case:?}: expected SecurityGroupIds {}, got {created}",
            row.security_group_ids
        );
        // `V2` is the only value `VpcLinkVersion` models, so it is what every link this
        // service mints reports.
        assert_eq!(
            as_str(&created, "vpcLinkVersion"),
            VPC_LINK_VERSION_V2,
            "case {case:?}: expected VpcLinkVersion {VPC_LINK_VERSION_V2}, got {created}"
        );
        // A link is still being provisioned when the create call answers.
        assert_eq!(
            as_str(&created, "vpcLinkStatus"),
            VPC_LINK_STATUS_PENDING,
            "case {case:?}: expected VpcLinkStatus {VPC_LINK_STATUS_PENDING}, got {created}"
        );
        assert!(
            !as_str(&created, "vpcLinkStatusMessage").is_empty(),
            "case {case:?}: expected a VpcLinkStatusMessage summarising the status, got {created}"
        );
        match row.request.get("tags") {
            Some(sent) => assert_eq!(
                created.get("tags"),
                Some(sent),
                "case {case:?}: expected Tags {sent}, got {created}"
            ),
            None => assert!(
                created
                    .get("tags")
                    .is_none_or(|tags| tags == &json!({})),
                "case {case:?}: no Tags were requested, got {created}"
            ),
        }

        // The round trip: the reader reports the record the create persisted. The
        // status is left out of the comparison because the CLOCK may have moved it
        // between the two calls — `a_created_vpc_link_becomes_available_after_its_span`
        // is the table over that — and it is asserted to be one of the two statuses a
        // stored link reaches.
        let (status, read) = call(&emulator, "GET", &format!("{VPC_LINKS}/{vpc_link_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetVpcLink, got {status} {read}"
        );
        for member in [
            "vpcLinkId",
            "name",
            "subnetIds",
            "securityGroupIds",
            "tags",
            "createdDate",
            "vpcLinkVersion",
        ] {
            assert_eq!(
                read.get(member),
                created.get(member),
                "case {case:?}: expected GetVpcLink to report the stored {member} {:?}, got {read}",
                created.get(member)
            );
        }
        let read_status = as_str(&read, "vpcLinkStatus");
        assert!(
            [VPC_LINK_STATUS_PENDING, VPC_LINK_STATUS_AVAILABLE].contains(&read_status.as_str()),
            "case {case:?}: expected GetVpcLink to report {VPC_LINK_STATUS_PENDING} or \
             {VPC_LINK_STATUS_AVAILABLE}, got {read_status:?}"
        );
    }
}

#[tokio::test]
async fn a_request_naming_an_unusable_subnet_or_security_group_is_refused() {
    let (emulator, engine) = emulator_with_store();

    for row in refused_rows() {
        let case = row.case;
        let (status, body) = call(&emulator, "POST", VPC_LINKS, Some(row.request.clone())).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateVpcLink, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        // The refusal says what to fix: the member and the value it rejected.
        let message = as_str(&body, "message");
        assert!(
            !message.is_empty(),
            "case {case:?}: expected a message naming the member, got {body}"
        );
        // Nothing was persisted, so no refused request leaves a link behind.
        assert_eq!(
            stored_links(&engine),
            0,
            "case {case:?}: a refused CreateVpcLink must persist nothing"
        );
    }

    // And the same emulator still accepts a well-formed request: the refusals above
    // consumed nothing.
    let (status, created) = call(
        &emulator,
        "POST",
        VPC_LINKS,
        Some(json!({ "name": "after-refusals", "subnetIds": ["subnet-0a1b2c3d"] })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"a valid create after the refusals\": expected 201, got {status} {created}"
    );
    assert_eq!(
        stored_links(&engine),
        1,
        "case \"a valid create after the refusals\": expected the store to hold one link"
    );
}

#[tokio::test]
async fn the_accepted_subnet_id_lengths_are_the_published_ec2_id_forms() {
    let emulator = emulator();

    // The catalog this service validates against is the EC2 resource-id documentation's
    // own: a prefix followed by 8 or 17 hexadecimal characters. Every published length
    // is accepted, and the length either side of each is not.
    for length in EC2_ID_HEX_LENGTHS.iter().copied() {
        for (case, hex_length, expected) in [
            ("one character short", length - 1, 400),
            ("the published length", length, 201),
            ("one character long", length + 1, 400),
        ] {
            // A length that is also the OTHER published form is accepted, so skip it
            // rather than assert the neighbour rule against it (17 - 1 = 16 and 8 + 1 =
            // 9 are neither).
            if expected == 400 && EC2_ID_HEX_LENGTHS.contains(&hex_length) {
                continue;
            }
            let subnet_id = format!("{SUBNET_ID_PREFIX}{}", "a".repeat(hex_length));
            let security_group_id = format!("{SECURITY_GROUP_ID_PREFIX}{}", "a".repeat(hex_length));
            let (status, body) = call(
                &emulator,
                "POST",
                VPC_LINKS,
                Some(json!({
                    "name": format!("len-{hex_length}"),
                    "subnetIds": [subnet_id],
                    "securityGroupIds": [security_group_id]
                })),
            )
            .await;
            assert_eq!(
                status, expected,
                "case {case:?} ({hex_length} hexadecimal characters): expected {expected}, got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_created_vpc_link_becomes_available_after_its_span() {
    let emulator = emulator();

    let (status, created) = call(
        &emulator,
        "POST",
        VPC_LINKS,
        Some(json!({ "name": "provisioning", "subnetIds": ["subnet-0a1b2c3d"] })),
    )
    .await;
    assert_eq!(status, 201, "expected 201 from CreateVpcLink, got {status} {created}");
    let vpc_link_id = as_str(&created, "vpcLinkId");
    let uri = format!("{VPC_LINKS}/{vpc_link_id}");

    // A caller reading straight after the call still sees the link coming up, and
    // reading it twice does not hurry it along.
    for round in 0..2 {
        let (_, read) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            as_str(&read, "vpcLinkStatus"),
            VPC_LINK_STATUS_PENDING,
            "case GetVpcLink(round {round}) within the span: expected {VPC_LINK_STATUS_PENDING}, got {read}"
        );
    }
    let pending_message = as_str(&created, "vpcLinkStatusMessage");

    // The transition is one the clock owes: once the span has passed the link is
    // available, for every reader, and it stays that way.
    tokio::time::sleep(VPC_LINK_CREATE_SPAN + Duration::from_millis(100)).await;
    for round in 0..2 {
        let (status, read) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case GetVpcLink(round {round}) after the span: expected 200, got {status} {read}"
        );
        assert_eq!(
            as_str(&read, "vpcLinkStatus"),
            VPC_LINK_STATUS_AVAILABLE,
            "case GetVpcLink(round {round}) after the span: expected {VPC_LINK_STATUS_AVAILABLE}, got {read}"
        );
        // The message summarises the status it is reported with, so it moved with it.
        let available_message = as_str(&read, "vpcLinkStatusMessage");
        assert_ne!(
            available_message, pending_message,
            "case GetVpcLink(round {round}) after the span: expected the status message to \
             summarise {VPC_LINK_STATUS_AVAILABLE}, got {available_message:?}"
        );
        // Nothing else about the record changed: a read is not a write.
        assert_eq!(
            read.get("createdDate"),
            created.get("createdDate"),
            "case GetVpcLink(round {round}) after the span: expected the stored CreatedDate, got {read}"
        );
    }
}

#[tokio::test]
async fn an_unknown_vpc_link_id_is_not_found() {
    let emulator = emulator();

    // One link exists, so a miss is a miss on the identifier and not on an empty store.
    let (_, created) = call(
        &emulator,
        "POST",
        VPC_LINKS,
        Some(json!({ "name": "the-one-link", "subnetIds": ["subnet-0a1b2c3d"] })),
    )
    .await;
    let known = as_str(&created, "vpcLinkId");

    for (case, vpc_link_id) in [
        ("an id no link carries", "aaaaaaaaaa".to_string()),
        ("an id that is not of the minted form", "no-such-link".to_string()),
        (
            "the stored id with its last character changed",
            // Map the last character onto one it is not, so the row is a miss whatever
            // the id bijection mints.
            format!(
                "{}{}",
                &known[..known.len() - 1],
                if known.ends_with('z') { 'a' } else { 'z' }
            ),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &format!("{VPC_LINKS}/{vpc_link_id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetVpcLink, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn the_account_holds_at_most_the_published_number_of_vpc_links() {
    let (emulator, engine) = emulator_with_store();

    // Every create up to the published quota is accepted, and each mints its own id.
    let mut ids = std::collections::BTreeSet::new();
    for index in 0..MAX_VPC_LINKS {
        let (status, created) = call(
            &emulator,
            "POST",
            VPC_LINKS,
            Some(json!({
                "name": format!("link-{index}"),
                "subnetIds": ["subnet-0a1b2c3d"]
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case \"link {index} of {MAX_VPC_LINKS}\": expected 201, got {status} {created}"
        );
        assert!(
            ids.insert(as_str(&created, "vpcLinkId")),
            "case \"link {index} of {MAX_VPC_LINKS}\": expected a VpcLinkId no other link carries, got {created}"
        );
    }

    // One past the bound is refused, and the store is left at the bound.
    let (status, body) = call(
        &emulator,
        "POST",
        VPC_LINKS,
        Some(json!({ "name": "one-too-many", "subnetIds": ["subnet-0a1b2c3d"] })),
    )
    .await;
    assert_eq!(
        status, 429,
        "case \"one past the quota\": expected 429, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "case \"one past the quota\": expected TooManyRequestsException, got {body}"
    );
    assert_eq!(
        stored_links(&engine),
        MAX_VPC_LINKS,
        "case \"one past the quota\": expected the store to hold {MAX_VPC_LINKS} links"
    );
}
