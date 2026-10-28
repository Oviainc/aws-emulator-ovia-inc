// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's DB-security-group INGRESS slice:
//! `AuthorizeDBSecurityGroupIngress` and `RevokeDBSecurityGroupIngress`, the two
//! EC2-Classic-era operations that modify a group's ingress rules.
//!
//! Every call goes over the wire, through the same mounting helper and awsQuery form POST
//! the sibling suites use, so the assertions read the XML the emulator itself answered
//! with.
//!
//! One rule, asserted over a TABLE rather than over one call: a DB security group of this
//! account is VPC-backed — both operations' own documentation records that "EC2-Classic was
//! retired on August 15, 2022" — and a VPC DB security group cannot be modified through
//! this API version, so every request that would change its rules is refused HTTP 400
//! `InvalidParameterValue` in the one text the service answers that with, and nothing is
//! stored. The table spans the axes the rule generalises over: the operation (both of
//! them), the FORM of authorization the request asks for (an IP range, an EC2 security
//! group by id, one by name and owner, and a request naming none), whether the range it
//! carries is well formed, the case the group's name is sent in, and a repeated call on
//! the same rule.
//!
//! Two further tests pin what the rule does NOT swallow: a name no create stored is still
//! the modelled 404 from both operations (so a caller can tell a missing group from one it
//! may not modify), in the region the request was signed for and no other, and a group
//! reads back holding no ingress rule after every refusal above.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    db_security_group_not_found_message, VPC_DB_SECURITY_GROUP_NOT_MODIFIABLE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, subtrees, text, REGION};

/// A second region, so the regional scope of the group lookup is observable.
const OTHER_REGION: &str = "eu-west-1";

/// The two operations that modify a DB security group's ingress rules, and so owe the same
/// refusal.
const INGRESS_ACTIONS: [&str; 2] = [
    "AuthorizeDBSecurityGroupIngress",
    "RevokeDBSecurityGroupIngress",
];

/// `CreateDBSecurityGroup` in `region`.
async fn create_group_in(router: &MultiServiceRouter, region: &str, name: &str) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateDBSecurityGroup",
        &[
            ("DBSecurityGroupName", name),
            ("DBSecurityGroupDescription", "an ingress probe group"),
        ],
    )
    .await
}

/// One row of the ingress table: which members the request carries beyond the group name.
struct FormCase {
    case: &'static str,
    members: &'static [(&'static str, &'static str)],
}

/// The class the rule generalises over: every form of authorization these operations
/// document ("one of CIDR range, EC2SecurityGroupId for VPC, or (EC2SecurityGroupOwnerId
/// and either EC2SecurityGroupName or EC2SecurityGroupId for non-VPC)"), a malformed range
/// beside a well-formed one, and a request naming no form at all.
const FORM_CASES: &[FormCase] = &[
    FormCase {
        case: "a well-formed CIDR range",
        members: &[("CIDRIP", "203.0.113.0/24")],
    },
    FormCase {
        case: "a host CIDR range",
        members: &[("CIDRIP", "10.1.2.3/32")],
    },
    FormCase {
        case: "a CIDR range that is not a CIDR range at all",
        members: &[("CIDRIP", "not-a-cidr")],
    },
    FormCase {
        case: "a CIDR range with a prefix length past 32",
        members: &[("CIDRIP", "10.0.0.0/64")],
    },
    FormCase {
        case: "an EC2 security group by id",
        members: &[("EC2SecurityGroupId", "sg-0123456789abcdef0")],
    },
    FormCase {
        case: "an EC2 security group by name and owner",
        members: &[
            ("EC2SecurityGroupName", "app-tier"),
            ("EC2SecurityGroupOwnerId", "000000000000"),
        ],
    },
    FormCase {
        case: "no form of authorization at all",
        members: &[],
    },
];

/// The request parameters of one ingress call: the group name plus the row's members.
fn ingress_params(name: &str, members: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut params = vec![("DBSecurityGroupName".to_string(), name.to_string())];
    for (member, value) in members {
        params.push(((*member).to_string(), (*value).to_string()));
    }
    params
}

/// One ingress call against [`REGION`].
async fn ingress(
    router: &MultiServiceRouter,
    action: &str,
    name: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let params = ingress_params(name, members);
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(member, value)| (member.as_str(), value.as_str()))
        .collect();
    call(router, action, &borrowed).await
}

/// How many `tag` elements a response carries.
fn count(xml: &str, tag: &str) -> usize {
    subtrees(xml, tag).len()
}

/// Every request that would modify the rules of a VPC-backed DB security group — whichever
/// of the two operations it names, whichever form it asks for, and whatever case the group
/// is named in — is refused HTTP 400 `InvalidParameterValue` in the service's own text,
/// and the group is left holding no ingress rule.
#[tokio::test]
async fn modifying_a_vpc_group_is_refused_whichever_form_the_request_asks_for() {
    let router = emulator();
    expect_ok(
        "create the group under test",
        create_group_in(&router, REGION, "Emu-Ingress-Grp").await,
    );

    for action in INGRESS_ACTIONS {
        for FormCase { case, members } in FORM_CASES {
            // The name is sent in the case the create used and in the case the store keys
            // it under: one spelling cannot be served differently from the other.
            for named in ["Emu-Ingress-Grp", "emu-ingress-grp", "EMU-INGRESS-GRP"] {
                let case = format!("{action} on {named:?} with {case}");
                // The same call TWICE: a repeat is refused exactly as the first was,
                // because the first stored nothing to collide with.
                for attempt in ["the first call", "the second call"] {
                    let case = format!("{case}, {attempt}");
                    let body = expect_error(
                        &case,
                        ingress(&router, action, named, members).await,
                        400,
                        "InvalidParameterValue",
                    );
                    assert_eq!(
                        text(&body, "Message"),
                        VPC_DB_SECURITY_GROUP_NOT_MODIFIABLE,
                        "case {case:?}: expected the refusal this API version owes a VPC DB \
                         security group, got {body}"
                    );
                }
            }
        }
    }

    // Nothing above was stored: the group reads back holding no rule of either form.
    let listed = expect_ok(
        "the group after every refusal",
        call(
            &router,
            "DescribeDBSecurityGroups",
            &[("DBSecurityGroupName", "emu-ingress-grp")],
        )
        .await,
    );
    assert_eq!(
        (
            count(&listed, "DBSecurityGroup"),
            count(&listed, "IPRange"),
            count(&listed, "EC2SecurityGroup"),
        ),
        (1, 0, 0),
        "a refused authorization must leave the group holding no ingress rule, got {listed}"
    );
}

/// A group name no create stored is the modelled 404 from both operations, naming the
/// group in the spelling the store keys it under — so a caller can tell a missing group
/// from one it may not modify.
#[tokio::test]
async fn a_group_no_create_stored_is_the_modelled_not_found() {
    let router = emulator();

    for action in INGRESS_ACTIONS {
        for named in ["emu-absent-grp", "Another-Absent-Grp", "x"] {
            let case = format!("{action} on the absent {named:?}");
            let body = expect_error(
                &case,
                ingress(&router, action, named, &[("CIDRIP", "203.0.113.0/24")]).await,
                404,
                "DBSecurityGroupNotFound",
            );
            assert_eq!(
                text(&body, "Message"),
                db_security_group_not_found_message(&named.to_ascii_lowercase()),
                "case {case:?}: expected the not-found wording this service shares, got {body}"
            );
        }
    }
}

/// The lookup is regional: a group only one region holds is not found in the other, so the
/// refusal above cannot be answered for a group this region does not have.
#[tokio::test]
async fn the_group_is_resolved_in_the_region_the_request_was_signed_for() {
    let router = emulator();
    expect_ok(
        "create the group in the signed region",
        create_group_in(&router, REGION, "emu-regional-ingress").await,
    );

    for action in INGRESS_ACTIONS {
        let params = ingress_params("emu-regional-ingress", &[("CIDRIP", "203.0.113.0/24")]);
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(member, value)| (member.as_str(), value.as_str()))
            .collect();

        // In the region that holds it, the group resolves and the version refusal is what
        // the caller sees...
        let body = expect_error(
            &format!("{action} in {REGION}"),
            call_in(&router, REGION, action, &borrowed).await,
            400,
            "InvalidParameterValue",
        );
        assert_eq!(
            text(&body, "Message"),
            VPC_DB_SECURITY_GROUP_NOT_MODIFIABLE,
            "case {action:?} in {REGION}: expected the version refusal, got {body}"
        );

        // ...and in the region that does not, the same name is the modelled 404.
        expect_error(
            &format!("{action} in {OTHER_REGION}"),
            call_in(&router, OTHER_REGION, action, &borrowed).await,
            404,
            "DBSecurityGroupNotFound",
        );
    }
}
