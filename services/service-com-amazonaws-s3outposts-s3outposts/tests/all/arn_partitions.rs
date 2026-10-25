// SPDX-License-Identifier: CC0-1.0

//! The partition field of every ARN this service reports, end to end over HTTP.
//!
//! The rule under test: an ARN names its resource in FULL, so the partition field and the
//! Region field of one ARN describe the same place — the partition is a function of the
//! Region the request was signed for, never a constant. `EndpointArn`, `OutpostArn` and
//! `S3OutpostArn` each spell the alternation out
//! (`^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):…`) beside a Region field their
//! pattern leaves open, so a request signed for a GovCloud Region owes `aws-us-gov` ARNs,
//! one signed for a China Region owes `aws-cn` ARNs, and so on.
//!
//! Every case drives the four operations that report an ARN — `CreateEndpoint`,
//! `ListEndpoints`, `ListSharedEndpoints` and `ListOutpostsWithS3` — through the router in
//! the row's Region, so each answer is the one a client signing for that Region receives.

use serde_json::Value;

use crate::endpoint_lifecycle::{
    call_in, emulator, endpoint_id_of, minimal_create, owned_outposts, string_at, Answer,
};
use smithy_server_runtime::MultiServiceRouter;

/// A Region and the partition its ARNs belong to.
///
/// The Region codes are taken from the emulator's own Region catalog
/// (`emulator_telemetry::policy::KNOWN_REGIONS`, "every AWS region the wire may carry") and
/// the partition names from this service's ARN patterns
/// (smithy_query(query="shape", shape="EndpointArn,OutpostArn,S3OutpostArn")), whose
/// alternation lists `aws`, `aws-cn`, `aws-us-gov`, `aws-iso` and `aws-iso-b`; the isolated
/// Regions AWS added after those patterns were published carry their own partition, since
/// the Region is what decides.
struct Row {
    case: &'static str,
    region: &'static str,
    partition: &'static str,
}

/// One row per partition the catalog reaches, plus a second Region in the partitions that
/// have one, so a case cannot pass by matching a single Region code.
const ROWS: &[Row] = &[
    Row {
        case: "the commercial partition",
        region: "us-east-1",
        partition: "aws",
    },
    Row {
        case: "another commercial Region",
        region: "ap-southeast-2",
        partition: "aws",
    },
    Row {
        case: "a commercial Region whose code is longest",
        region: "ap-northeast-3",
        partition: "aws",
    },
    Row {
        case: "the China partition",
        region: "cn-north-1",
        partition: "aws-cn",
    },
    Row {
        case: "the other China Region",
        region: "cn-northwest-1",
        partition: "aws-cn",
    },
    Row {
        case: "GovCloud (US-West)",
        region: "us-gov-west-1",
        partition: "aws-us-gov",
    },
    Row {
        case: "GovCloud (US-East)",
        region: "us-gov-east-1",
        partition: "aws-us-gov",
    },
    Row {
        case: "the isolated partition",
        region: "us-iso-east-1",
        partition: "aws-iso",
    },
    Row {
        case: "the other isolated Region",
        region: "us-iso-west-1",
        partition: "aws-iso",
    },
    Row {
        case: "the second isolated partition, whose prefix extends the first",
        region: "us-isob-east-1",
        partition: "aws-iso-b",
    },
    Row {
        case: "an isolated partition added after the ARN patterns were published",
        region: "us-isof-east-1",
        partition: "aws-iso-f",
    },
    Row {
        case: "the European isolated partition",
        region: "eu-isoe-west-1",
        partition: "aws-iso-e",
    },
];

async fn create_in(router: &MultiServiceRouter, region: &str, request: Value) -> Answer {
    call_in(
        router,
        region,
        "POST",
        "/S3Outposts/CreateEndpoint",
        Some(request),
    )
    .await
}

async fn list_endpoints_in(router: &MultiServiceRouter, region: &str) -> Answer {
    call_in(
        router,
        region,
        "GET",
        "/S3Outposts/ListEndpoints?maxResults=100",
        None,
    )
    .await
}

async fn list_shared_in(router: &MultiServiceRouter, region: &str, outpost_id: &str) -> Answer {
    call_in(
        router,
        region,
        "GET",
        &format!("/S3Outposts/ListSharedEndpoints?outpostId={outpost_id}&maxResults=100"),
        None,
    )
    .await
}

async fn list_outposts_in(router: &MultiServiceRouter, region: &str) -> Answer {
    call_in(
        router,
        region,
        "GET",
        "/S3Outposts/ListOutpostsWithS3?maxResults=100",
        None,
    )
    .await
}

/// The partition field of an ARN: its second colon-separated field.
fn partition_of(arn: &str) -> &str {
    let mut fields = arn.split(':');
    assert_eq!(
        fields.next(),
        Some("arn"),
        "{arn:?} must begin with the literal arn"
    );
    fields
        .next()
        .unwrap_or_else(|| panic!("{arn:?} carries no partition field"))
}

/// The endpoint in `answer` whose `EndpointArn` is `arn`.
fn endpoint_with_arn(answer: &Answer, arn: &str) -> Value {
    answer
        .endpoints()
        .iter()
        .find(|endpoint| endpoint["EndpointArn"].as_str() == Some(arn))
        .unwrap_or_else(|| panic!("no endpoint with ARN {arn} in {:?}", answer.body))
        .clone()
}

/// The Outpost `outpost_id` in `answer`.
fn outpost_with_id(answer: &Answer, outpost_id: &str) -> Value {
    answer
        .outposts()
        .iter()
        .find(|outpost| outpost["OutpostId"].as_str() == Some(outpost_id))
        .unwrap_or_else(|| panic!("no Outpost {outpost_id} in {:?}", answer.body))
        .clone()
}

/// Every ARN an answer reports names the partition of the Region the request was signed
/// for: the endpoint ARN `CreateEndpoint` mints and the listings report, and the Outpost's
/// own two ARNs.
#[tokio::test]
async fn every_arn_names_the_partition_of_the_region_the_request_was_signed_for() {
    let account = engine::emulator_account_id();
    for Row {
        case,
        region,
        partition,
    } in ROWS
    {
        // Its own emulator per row, so the endpoint the row creates is the only one of
        // this account in the answer.
        let router = emulator();
        let outpost_id = owned_outposts(&router)
            .await
            .first()
            .expect("the emulator presents an owned Outpost")
            .clone();

        let created = create_in(&router, region, minimal_create(&outpost_id)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?} ({region}): CreateEndpoint must answer 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_at(&created.body, "EndpointArn");
        let endpoint_id = endpoint_id_of(&arn);
        assert_eq!(
            arn,
            format!(
                "arn:{partition}:s3-outposts:{region}:{account}:outpost/{outpost_id}/endpoint/{endpoint_id}"
            ),
            "case {case:?} ({region}): the EndpointArn must name the Region's own partition, got {arn:?}"
        );

        let listed = list_endpoints_in(&router, region).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?} ({region}): ListEndpoints must answer 200, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            string_at(&endpoint_with_arn(&listed, &arn), "EndpointArn"),
            arn,
            "case {case:?} ({region}): the listed EndpointArn must be the created one"
        );

        let shared = list_shared_in(&router, region, &outpost_id).await;
        assert_eq!(
            shared.status, 200,
            "case {case:?} ({region}): ListSharedEndpoints must answer 200, got {} with {:?}",
            shared.status, shared.body
        );
        assert_eq!(
            string_at(&endpoint_with_arn(&shared, &arn), "EndpointArn"),
            arn,
            "case {case:?} ({region}): the Outpost-scoped listing must report the same ARN"
        );

        let outposts = list_outposts_in(&router, region).await;
        assert_eq!(
            outposts.status, 200,
            "case {case:?} ({region}): ListOutpostsWithS3 must answer 200, got {} with {:?}",
            outposts.status, outposts.body
        );
        let outpost = outpost_with_id(&outposts, &outpost_id);
        assert_eq!(
            string_at(&outpost, "OutpostArn"),
            format!("arn:{partition}:outposts:{region}:{account}:outpost/{outpost_id}"),
            "case {case:?} ({region}): the OutpostArn must name the Region's own partition"
        );
        assert_eq!(
            string_at(&outpost, "S3OutpostArn"),
            format!("arn:{partition}:s3-outposts:{region}:{account}:outpost/{outpost_id}/s3"),
            "case {case:?} ({region}): the S3OutpostArn must name the Region's own partition"
        );
    }
}

/// The partition is a function of the Region ALONE: one store, read in several Regions,
/// reports every ARN of one answer in that caller's partition — no ARN of an answer names
/// a partition another ARN of the same answer contradicts, and no two Regions in different
/// partitions report the same partition.
#[tokio::test]
async fn one_store_reports_each_region_s_own_partition_consistently() {
    let router = emulator();
    let outpost_id = owned_outposts(&router)
        .await
        .first()
        .expect("the emulator presents an owned Outpost")
        .clone();

    // One endpoint, created once, then read back from every Region: the records are the
    // same, so any difference in the answers is the projection under test.
    let created = create_in(&router, "us-east-1", minimal_create(&outpost_id)).await;
    assert_eq!(
        created.status, 200,
        "CreateEndpoint must answer 200, got {} with {:?}",
        created.status, created.body
    );
    let endpoint_id = endpoint_id_of(&string_at(&created.body, "EndpointArn"));

    for Row {
        case,
        region,
        partition,
    } in ROWS
    {
        let listed = list_endpoints_in(&router, region).await;
        let endpoint_arn = listed
            .endpoints()
            .iter()
            .map(|endpoint| string_at(endpoint, "EndpointArn"))
            .find(|arn| arn.ends_with(&format!("/endpoint/{endpoint_id}")))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?} ({region}): endpoint {endpoint_id} must be listed, got {:?}",
                    listed.body
                )
            });

        let outposts = list_outposts_in(&router, region).await;
        let outpost = outpost_with_id(&outposts, &outpost_id);
        let outpost_arn = string_at(&outpost, "OutpostArn");
        let s3_outpost_arn = string_at(&outpost, "S3OutpostArn");

        for reported in [&endpoint_arn, &outpost_arn, &s3_outpost_arn] {
            assert_eq!(
                partition_of(reported),
                *partition,
                "case {case:?} ({region}): every ARN of the answer must name that Region's \
                 partition, got {reported:?}"
            );
            assert!(
                reported.contains(&format!(":{region}:")),
                "case {case:?}: an ARN naming partition {partition} must name the Region it \
                 belongs to, got {reported:?}"
            );
        }
    }
}
