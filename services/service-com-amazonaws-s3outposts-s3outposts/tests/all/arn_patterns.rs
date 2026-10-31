// SPDX-License-Identifier: CC0-1.0

//! Every ARN this service reports is one the reporting member's own pattern admits.
//!
//! The rule under test: a response member whose declared pattern admits only SOME of the
//! forms an identifier can take carries a value that pattern admits, or carries nothing at
//! all — never a string the pattern rejects. `OutpostId`'s three branches
//! (`op-[a-f0-9]{17}`, `\d{12}` and `ec2`) are admitted by different subsets of the three
//! ARN shapes, so for some Outposts one of the ARNs has no legal value at all; the member
//! is optional, so it is then absent while everything else about the resource is still
//! reported.
//!
//! Every expectation below is written out per case — the whole ARN where the member can
//! carry one, and nothing where it cannot — read off the members' own patterns, which this
//! file quotes, and never from the service's own idea of which branch an ARN admits, so a
//! wrong constant in the service cannot make a case pass.
//!
//! Every case goes through the router, so each exercises routing, deserialization,
//! boundary validation, the handler, the persisted state and serialization.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{
    create_in, emulator_holding_owned, list_endpoints_in, list_outposts_in, list_shared_in,
    minimal_create, shared_query, string_at, Answer, SHARING_ACCOUNT_ID, WHOLE_PAGE,
};

/// A Region and the partition its ARNs name, limited to the partitions the three ARN
/// patterns' own alternation lists (`^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):…`):
/// which partition a Region belongs to is `arn_partitions.rs`'s subject, and the Region is
/// varied here only so no case can pass by matching one Region's ARN prefix.
const REGIONS: &[(&str, &str)] = &[
    ("us-east-1", "aws"),
    ("ap-southeast-2", "aws"),
    ("cn-north-1", "aws-cn"),
    ("us-gov-west-1", "aws-us-gov"),
    ("us-iso-east-1", "aws-iso"),
    ("us-isob-east-1", "aws-iso-b"),
];

/// One Outpost to provision, and which ARN members can name it.
///
/// The rows span every branch of `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$` (source of
/// the catalogue: `smithy_query(query="shape", shape="OutpostId")`), several values inside
/// each branch, and both an Outpost of the caller's own and one another account shared with
/// it — the owner being the account field the Outpost ARNs carry.
///
/// The three expectations are read off the ARN members' own patterns, quoted from
/// `smithy_query(query="shape", shape="OutpostArn,S3OutpostArn,EndpointArn")`:
/// `OutpostArn` ends `…:outpost/(op-[a-f0-9]{17}|ec2)$`, `S3OutpostArn` ends
/// `…:outpost/(op-[a-f0-9]{17}|\d{12})/s3$` and `EndpointArn` ends
/// `…:outpost/(op-[a-f0-9]{17}|ec2)/endpoint/[a-zA-Z0-9]{19}$` — so each row states, as
/// data, which members have a legal value for its id at all.
struct Row {
    case: &'static str,
    outpost_id: &'static str,
    owner: Owner,
    /// Whether `Outpost$OutpostArn`'s pattern admits this row's id.
    outpost_arn: bool,
    /// Whether `Outpost$S3OutpostArn`'s pattern admits this row's id.
    s3_outpost_arn: bool,
    /// Whether `Endpoint$EndpointArn`'s pattern admits this row's id.
    endpoint_arn: bool,
}

/// Who owns the Outpost of a row: the caller's own account, or the account that shared it.
enum Owner {
    Caller,
    Sharing,
}

const ROWS: &[Row] = &[
    Row {
        case: "the op- branch",
        outpost_id: "op-0a1b2c3d4e5f60718",
        owner: Owner::Caller,
        outpost_arn: true,
        s3_outpost_arn: true,
        endpoint_arn: true,
    },
    Row {
        case: "the op- branch, all zero, shared by another account",
        outpost_id: "op-00000000000000000",
        owner: Owner::Sharing,
        outpost_arn: true,
        s3_outpost_arn: true,
        endpoint_arn: true,
    },
    Row {
        case: "the op- branch, all f",
        outpost_id: "op-fffffffffffffffff",
        owner: Owner::Caller,
        outpost_arn: true,
        s3_outpost_arn: true,
        endpoint_arn: true,
    },
    Row {
        case: "the 12-digit branch",
        outpost_id: "987654321098",
        owner: Owner::Caller,
        outpost_arn: false,
        s3_outpost_arn: true,
        endpoint_arn: false,
    },
    Row {
        case: "the 12-digit branch, all zero",
        outpost_id: "000000000000",
        owner: Owner::Caller,
        outpost_arn: false,
        s3_outpost_arn: true,
        endpoint_arn: false,
    },
    Row {
        case: "the 12-digit branch, shared by another account",
        outpost_id: "999999999999",
        owner: Owner::Sharing,
        outpost_arn: false,
        s3_outpost_arn: true,
        endpoint_arn: false,
    },
    Row {
        case: "the ec2 branch",
        outpost_id: "ec2",
        owner: Owner::Caller,
        outpost_arn: true,
        s3_outpost_arn: false,
        endpoint_arn: true,
    },
    Row {
        case: "the ec2 branch, shared by another account",
        outpost_id: "ec2",
        owner: Owner::Sharing,
        outpost_arn: true,
        s3_outpost_arn: false,
        endpoint_arn: true,
    },
];

/// The owning account id of a row.
fn owner_of(row: &Row) -> String {
    match row.owner {
        Owner::Caller => engine::emulator_account_id().to_string(),
        Owner::Sharing => SHARING_ACCOUNT_ID.to_string(),
    }
}

/// Which of the Outpost's two ARN members have a legal value for an `OutpostId`, read off
/// the members' own patterns (`smithy_query(query="shape",
/// shape="OutpostArn,S3OutpostArn")`): `OutpostArn` lists `(op-[a-f0-9]{17}|ec2)` as its
/// Outpost segment and `S3OutpostArn` lists `(op-[a-f0-9]{17}|\d{12})`, so the `op-` branch
/// is named by both, `ec2` only by the first and the 12-digit form only by the second.
///
/// One row of [`ROWS`] states the same thing per id; the first test checks the two agree,
/// so neither can drift.
pub(crate) fn outpost_arn_members(outpost_id: &str) -> (bool, bool) {
    match outpost_id {
        "ec2" => (true, false),
        id if id.starts_with("op-") => (true, true),
        _ => (false, true),
    }
}

/// What an ARN member must carry: `candidate` where the member's pattern admits this
/// Outpost, and NOTHING where it does not.
pub(crate) fn legal_or_absent(admitted: bool, candidate: String) -> Option<String> {
    admitted.then_some(candidate)
}

/// The string member `name` of `value`, or `None` when the answer carries no such member.
pub(crate) fn optional_member(value: &Value, name: &str) -> Option<String> {
    value[name].as_str().map(str::to_string)
}

/// The single Outpost a one-Outpost inventory reports in `region`.
pub(crate) async fn the_outpost(router: &MultiServiceRouter, region: &str) -> Value {
    let answer = list_outposts_in(router, region, WHOLE_PAGE).await;
    assert_eq!(
        answer.status, 200,
        "ListOutpostsWithS3 must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    let outposts = answer.outposts();
    assert_eq!(
        outposts.len(),
        1,
        "the inventory holds exactly one Outpost, got {:?}",
        answer.body
    );
    outposts[0].clone()
}

/// `ListOutpostsWithS3` reports, for every branch of `OutpostId` and in every partition,
/// exactly the ARN each member's pattern admits for that Outpost — and leaves the member
/// out where its pattern admits no ARN for it, while still reporting the Outpost itself.
#[tokio::test]
async fn an_outpost_reports_only_the_arns_its_members_patterns_admit() {
    let mut outpost_arns_absent = 0;
    let mut s3_outpost_arns_absent = 0;

    for row in ROWS {
        let owner = owner_of(row);
        let id = row.outpost_id;

        for (region, partition) in REGIONS {
            let case = format!("{} in {region}", row.case);
            // Its own emulator per Region, since the Outpost is installed in one Region.
            let router = emulator_holding_owned(&[(id, owner.as_str(), region)]).0;
            let outpost = the_outpost(&router, region).await;

            // The Outpost itself is reported whatever its id's branch: an ARN a pattern
            // cannot express is left out, it does not hide the resource.
            assert_eq!(
                string_at(&outpost, "OutpostId"),
                id,
                "case {case:?}: the inventory must report the provisioned Outpost, got \
                 {outpost:?}"
            );
            assert_eq!(
                string_at(&outpost, "OwnerId"),
                owner,
                "case {case:?}: the Outpost must report its owning account, got {outpost:?}"
            );

            // The row's own expectations and the branch table must say the same thing.
            assert_eq!(
                outpost_arn_members(id),
                (row.outpost_arn, row.s3_outpost_arn),
                "case {case:?}: the row's expectations and the branch read off the patterns \
                 must agree for {id}"
            );

            let expected_outpost_arn = legal_or_absent(
                row.outpost_arn,
                format!("arn:{partition}:outposts:{region}:{owner}:outpost/{id}"),
            );
            assert_eq!(
                optional_member(&outpost, "OutpostArn"),
                expected_outpost_arn,
                "case {case:?}: expected OutpostArn {expected_outpost_arn:?} — the ARN built \
                 from the record where OutpostArn's pattern admits this id, nothing where it \
                 does not — got {outpost:?}"
            );
            if expected_outpost_arn.is_none() {
                outpost_arns_absent += 1;
            }

            let expected_s3_arn = legal_or_absent(
                row.s3_outpost_arn,
                format!("arn:{partition}:s3-outposts:{region}:{owner}:outpost/{id}/s3"),
            );
            assert_eq!(
                optional_member(&outpost, "S3OutpostArn"),
                expected_s3_arn,
                "case {case:?}: expected S3OutpostArn {expected_s3_arn:?} — the ARN built from \
                 the record where S3OutpostArn's pattern admits this id, nothing where it does \
                 not — got {outpost:?}"
            );
            if expected_s3_arn.is_none() {
                s3_outpost_arns_absent += 1;
            }
        }
    }

    // The table covers both halves of the rule: some rows have no legal ARN for one of the
    // two members, so a service that reported every member always would fail above.
    assert!(
        outpost_arns_absent > 0 && s3_outpost_arns_absent > 0,
        "the table must cover Outposts whose ids each ARN pattern rejects, got \
         {outpost_arns_absent} absent OutpostArn(s) and {s3_outpost_arns_absent} absent \
         S3OutpostArn(s)"
    );
}

/// `CreateEndpoint` mints an `EndpointArn` only where `EndpointArn`'s pattern admits the
/// Outpost the endpoint sits on; either way the endpoint is stored and the listings that
/// serve its Outpost report it, carrying the very same ARN member the create answered with.
#[tokio::test]
async fn an_endpoint_reports_only_an_arn_its_pattern_admits() {
    let caller = engine::emulator_account_id().to_string();
    let mut endpoint_arns_absent = 0;

    for row in ROWS {
        let id = row.outpost_id;
        let owner = owner_of(row);

        for (region, partition) in REGIONS {
            let case = format!("{} in {region}", row.case);
            // Its own emulator per case, so the endpoint created here is the only one.
            let router = emulator_holding_owned(&[(id, owner.as_str(), region)]).0;

            let created = create_in(&router, region, minimal_create(id)).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateEndpoint must answer 200, got {} with {:?}",
                created.status, created.body
            );

            // An endpoint ARN names the account that created the endpoint, whoever owns the
            // Outpost it is on.
            let outpost_prefix =
                format!("arn:{partition}:s3-outposts:{region}:{caller}:outpost/{id}");

            let reported = optional_member(&created.body, "EndpointArn");
            assert_eq!(
                reported.is_some(),
                row.endpoint_arn,
                "case {case:?}: the EndpointArn member must be carried exactly where its \
                 pattern admits an ARN on this Outpost (admitted: {}), got {:?}",
                row.endpoint_arn,
                created.body
            );
            if let Some(arn) = reported.as_deref() {
                let (prefix, endpoint_id) = arn
                    .rsplit_once("/endpoint/")
                    .unwrap_or_else(|| panic!("case {case:?}: {arn:?} is no EndpointArn"));
                assert_eq!(
                    prefix, outpost_prefix,
                    "case {case:?}: the EndpointArn must name this caller and this Outpost, got \
                     {arn:?}"
                );
                // `EndpointId`'s own `^[a-zA-Z0-9]{19}$`, so the whole ARN is one the
                // member's pattern admits.
                assert!(
                    endpoint_id.len() == 19
                        && endpoint_id.bytes().all(|b| b.is_ascii_alphanumeric()),
                    "case {case:?}: the EndpointArn must end in a 19-character alphanumeric \
                     endpoint id, got {arn:?}"
                );
            } else {
                endpoint_arns_absent += 1;
            }

            // Stored either way, and read back with the same ARN member by the listing of
            // this account's own endpoints — and, for an Outpost another account shared
            // with this caller, by the Outpost-scoped listing too, which serves exactly
            // those (`shared_endpoint_listing.rs`).
            let mut answers: Vec<(&str, Answer)> = vec![(
                "ListEndpoints",
                list_endpoints_in(&router, region, WHOLE_PAGE).await,
            )];
            match row.owner {
                Owner::Sharing => answers.push((
                    "ListSharedEndpoints",
                    list_shared_in(&router, region, &shared_query(id)).await,
                )),
                Owner::Caller => {
                    let refused = list_shared_in(&router, region, &shared_query(id)).await;
                    assert_eq!(
                        (refused.status, refused.error_code()),
                        (404, "ResourceNotFoundException"),
                        "case {case:?}: an Outpost this account owns is not one \
                         ListSharedEndpoints serves, got {} with {:?}",
                        refused.status,
                        refused.body
                    );
                }
            }

            for (listing, answer) in answers {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: {listing} must answer 200, got {} with {:?}",
                    answer.status, answer.body
                );
                let endpoints = answer.endpoints();
                assert_eq!(
                    endpoints.len(),
                    1,
                    "case {case:?}: {listing} must report the created endpoint, got {:?}",
                    answer.body
                );
                assert_eq!(
                    string_at(&endpoints[0], "OutpostsId"),
                    id,
                    "case {case:?}: {listing} must report the endpoint on its own Outpost, got \
                     {:?}",
                    answer.body
                );
                assert_eq!(
                    optional_member(&endpoints[0], "EndpointArn"),
                    reported,
                    "case {case:?}: {listing} must report the same EndpointArn member the create \
                     answered with ({reported:?}), got {:?}",
                    answer.body
                );
            }
        }
    }

    // Some rows name an Outpost no EndpointArn can express, so the absent half of the rule
    // is genuinely exercised.
    assert!(
        endpoint_arns_absent > 0,
        "the table must cover endpoints on Outposts EndpointArn's pattern rejects, got \
         {endpoint_arns_absent} absent EndpointArn(s)"
    );
}
