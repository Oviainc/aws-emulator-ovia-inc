// SPDX-License-Identifier: CC0-1.0

//! What an account reports before anything has been provisioned for it, and what
//! provisioning capacity changes.
//!
//! The rule under test: a resource this API models no operation to create exists only
//! where something OUTSIDE the API provisioned it, so a store holds none of them until
//! it is told otherwise, and every answer is computed from the records it does hold.
//! For `ListOutpostsWithS3` (`@paginated(inputToken: NextToken, outputToken: NextToken,
//! items: Outposts, pageSize: MaxResults)`, per
//! `smithy_query(query="operation", operation="ListOutpostsWithS3")`) that means an empty
//! `Outposts` list for an account with no S3 on Outposts capacity, and a `NextToken` only
//! on a page that records remain beyond — never on the last page, and never on an empty
//! one.
//!
//! Every case goes through the router, so each exercises routing, deserialization,
//! boundary validation, the handler and serialization — the path a client takes.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_s3outposts_s3outposts::state::{
    parse_provisioned_outposts, OutpostRecord,
};
use service_com_amazonaws_s3outposts_s3outposts::ComAmazonawsS3outpostsS3outpostsState;
use service_com_amazonaws_s3outposts_s3outposts::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{
    create, delete_endpoint, list_endpoints, list_outposts, list_shared, minimal_create, string_at,
    unprovisioned_emulator, Answer,
};

/// Every branch of `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$`. Source of the
/// catalogue: `smithy_query(query="shape", shape="OutpostId")`.
const EVERY_OUTPOST_ID_FORM: &[(&str, &str)] = &[
    ("the op- branch", "op-0a1b2c3d4e5f60718"),
    ("the op- branch, all f", "op-fffffffffffffffff"),
    ("the 12-digit branch", "123456789012"),
    ("the ec2 branch", "ec2"),
];

/// A well-formed `EndpointId` (`^[a-zA-Z0-9]{19}$`) nothing ever issued.
const NEVER_ISSUED_ENDPOINT_ID: &str = "A1B2C3D4E5F6G7H8I9J";

/// What `ListOutpostsWithS3` reports for one Outpost: its id, its owning account and its
/// capacity in bytes.
type ReportedOutpost = (String, String, i64);

/// One row of the provisioning table: the case name, the operator setting, and the
/// Outposts the listing must then report.
type ProvisioningCase = (&'static str, &'static str, Vec<ReportedOutpost>);

/// One emulator over a store provisioned with exactly `records`.
fn emulator_provisioned_with(records: Vec<OutpostRecord>) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine.state::<ComAmazonawsS3outpostsS3outpostsState>();
        let mut store = state.lock();
        for record in records {
            store.provision_outpost(record);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The `(OutpostId, OwnerId, CapacityInBytes)` triples a listing reports.
fn reported(answer: &Answer) -> Vec<ReportedOutpost> {
    answer
        .outposts()
        .iter()
        .map(|outpost| {
            (
                string_at(outpost, "OutpostId"),
                string_at(outpost, "OwnerId"),
                outpost["CapacityInBytes"].as_i64().unwrap_or_else(|| {
                    panic!("CapacityInBytes must be a number, got {:?}", outpost)
                }),
            )
        })
        .collect()
}

/// An account nobody has provisioned S3 on Outposts capacity for reports an empty
/// `Outposts` list and NO `NextToken`, whatever page size or cursor the request carries.
///
/// The rows differ along what the rule generalises over: the page size — both bounds
/// `MaxResults`'s `@range(min: 0, max: 100)` publishes, sizes between them, and no
/// `MaxResults` member at all — and whether a cursor is carried.
#[tokio::test]
async fn an_unprovisioned_account_reports_no_outposts_and_no_token() {
    let router = unprovisioned_emulator();

    let cases = [
        ("no MaxResults member at all", ""),
        ("MaxResults 0, the low bound (no preference)", "?maxResults=0"),
        ("one per page", "?maxResults=1"),
        ("two per page", "?maxResults=2"),
        ("fifty per page", "?maxResults=50"),
        ("MaxResults 100, the published ceiling", "?maxResults=100"),
        (
            "a cursor in the op- branch",
            "?nextToken=op-0a1b2c3d4e5f60718",
        ),
        (
            "a cursor and a page size",
            "?maxResults=1&nextToken=op-fffffffffffffffff",
        ),
    ];

    for (case, query) in cases {
        let answer = list_outposts(&router, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: ListOutpostsWithS3 must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported(&answer),
            Vec::new(),
            "case {case:?}: an account with no capacity provisioned reports no Outposts, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.next_token(),
            None,
            "case {case:?}: a listing that terminates carries no NextToken, got {:?}",
            answer.next_token()
        );
    }

    // One past the published ceiling is refused at the boundary, not answered with a
    // page.
    let refused = list_outposts(&router, "?maxResults=101").await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "ValidationException"),
        "MaxResults 101 is past the range the model publishes, got {} with {:?}",
        refused.status,
        refused.body
    );
}

/// With no Outpost provisioned, every reference to one is the modelled 404 — whichever
/// form of `OutpostId` names it, and whichever operation resolves it — while the listing
/// of this account's own endpoints is simply empty.
#[tokio::test]
async fn every_outpost_reference_on_an_unprovisioned_account_is_the_modelled_404() {
    let router = unprovisioned_emulator();

    for (form, outpost_id) in EVERY_OUTPOST_ID_FORM {
        let created = create(&router, minimal_create(outpost_id)).await;
        assert_eq!(
            (created.status, created.error_code()),
            (404, "ResourceNotFoundException"),
            "case {form:?}: CreateEndpoint on an Outpost nothing provisioned must answer 404, \
             got {} with {:?}",
            created.status,
            created.body
        );

        let shared = list_shared(&router, &format!("?outpostId={outpost_id}&maxResults=100")).await;
        assert_eq!(
            (shared.status, shared.error_code()),
            (404, "ResourceNotFoundException"),
            "case {form:?}: ListSharedEndpoints on an Outpost nothing provisioned must answer \
             404, got {} with {:?}",
            shared.status,
            shared.body
        );

        let deleted = delete_endpoint(&router, outpost_id, NEVER_ISSUED_ENDPOINT_ID).await;
        assert_eq!(
            (deleted.status, deleted.error_code()),
            (404, "ResourceNotFoundException"),
            "case {form:?}: DeleteEndpoint under an Outpost nothing provisioned must answer 404, \
             got {} with {:?}",
            deleted.status,
            deleted.body
        );
    }

    let endpoints = list_endpoints(&router, "?maxResults=100").await;
    assert_eq!(
        endpoints.status, 200,
        "ListEndpoints must answer 200, got {} with {:?}",
        endpoints.status, endpoints.body
    );
    assert!(
        endpoints.endpoints().is_empty() && endpoints.next_token().is_none(),
        "an account with nothing provisioned owns no endpoints, got {:?}",
        endpoints.body
    );
}

/// The operator setting is the ONLY thing that puts an Outpost in the inventory, and what
/// the listing reports is exactly the records it declared: the id, the owning account and
/// the capacity, with the ARNs built from them.
///
/// The rows differ along what the rule generalises over: how many Outposts are declared,
/// which branch of `OutpostId` names them, whether the owner is the caller or another
/// account, and the surrounding space — plus every way an entry can fail to declare an
/// Outpost this API could report, which must provision nothing.
#[tokio::test]
async fn only_what_was_provisioned_is_reported() {
    let caller = engine::emulator_account_id().to_string();
    let cases: Vec<ProvisioningCase> = vec![
        ("nothing declared", "", Vec::new()),
        ("only separators", " , ", Vec::new()),
        (
            "one Outpost of this account",
            "op-0a1b2c3d4e5f60718:52776558133248",
            vec![(
                "op-0a1b2c3d4e5f60718".to_string(),
                caller.clone(),
                52_776_558_133_248,
            )],
        ),
        (
            "two Outposts of this account",
            "op-0a1b2c3d4e5f60718:52776558133248,op-1b2c3d4e5f6071829:105553116266496",
            vec![
                (
                    "op-0a1b2c3d4e5f60718".to_string(),
                    caller.clone(),
                    52_776_558_133_248,
                ),
                (
                    "op-1b2c3d4e5f6071829".to_string(),
                    caller.clone(),
                    105_553_116_266_496,
                ),
            ],
        ),
        (
            "an Outpost another account shares, with space around every field",
            " op-2c3d4e5f607182930 : 26388279066624 : 000000000001 ",
            vec![(
                "op-2c3d4e5f607182930".to_string(),
                "000000000001".to_string(),
                26_388_279_066_624,
            )],
        ),
        (
            "the ec2 branch of OutpostId",
            "ec2:1099511627776",
            vec![("ec2".to_string(), caller.clone(), 1_099_511_627_776)],
        ),
        (
            "the 12-digit branch of OutpostId",
            "000000000000:1099511627776",
            vec![(
                "000000000000".to_string(),
                caller.clone(),
                1_099_511_627_776,
            )],
        ),
        (
            "a good entry beside a malformed one",
            "op-zzzzzzzzzzzzzzzzz:1024,op-0a1b2c3d4e5f60718:52776558133248",
            vec![(
                "op-0a1b2c3d4e5f60718".to_string(),
                caller.clone(),
                52_776_558_133_248,
            )],
        ),
        (
            "an id the model's pattern does not admit",
            "outpost-1:1024",
            Vec::new(),
        ),
        (
            "an op- id of the wrong width",
            "op-0a1b2c3d:1024",
            Vec::new(),
        ),
        ("no capacity at all", "op-0a1b2c3d4e5f60718", Vec::new()),
        (
            "a capacity that is not a number",
            "op-0a1b2c3d4e5f60718:plenty",
            Vec::new(),
        ),
        (
            "zero capacity, which is no S3 provisioned",
            "op-0a1b2c3d4e5f60718:0",
            Vec::new(),
        ),
        (
            "a negative capacity",
            "op-0a1b2c3d4e5f60718:-1024",
            Vec::new(),
        ),
        (
            "an owner that is not a 12-digit account",
            "op-0a1b2c3d4e5f60718:1024:12345",
            Vec::new(),
        ),
        (
            "a field the form does not have",
            "op-0a1b2c3d4e5f60718:1024:000000000001:extra",
            Vec::new(),
        ),
    ];

    for (case, setting, expected) in cases {
        let records = parse_provisioned_outposts(setting);
        let router = emulator_provisioned_with(records);
        let answer = list_outposts(&router, "?maxResults=100").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: ListOutpostsWithS3 must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported(&answer),
            expected,
            "case {case:?}: the listing must report exactly the provisioned Outposts, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.next_token(),
            None,
            "case {case:?}: the whole inventory fits in the page, so no NextToken is due, got \
             {:?}",
            answer.next_token()
        );

        // The ARNs are built from the record's own ids and owner, for the Region the
        // request was signed for.
        for outpost in answer.outposts() {
            let id = string_at(outpost, "OutpostId");
            let owner = string_at(outpost, "OwnerId");
            assert_eq!(
                string_at(outpost, "OutpostArn"),
                format!("arn:aws:outposts:us-east-1:{owner}:outpost/{id}"),
                "case {case:?}: the OutpostArn must name the record's owner and id, got {outpost:?}"
            );
            assert_eq!(
                string_at(outpost, "S3OutpostArn"),
                format!("arn:aws:s3-outposts:us-east-1:{owner}:outpost/{id}/s3"),
                "case {case:?}: the S3OutpostArn must name the record's owner and id, got \
                 {outpost:?}"
            );
        }
    }
}

/// Paging terminates on the last page for an inventory of ANY size, the empty one
/// included: a page the inventory has nothing beyond carries no `NextToken`, and a
/// shorter one carries a cursor that leads to the rest and then stops.
#[tokio::test]
async fn paging_carries_a_token_only_while_records_remain() {
    for count in 0..=4u32 {
        // `count` Outposts, each in the op- branch of `OutpostId` (17 hex digits).
        let setting = (0..count)
            .map(|index| format!("op-{index:017x}:{}", (index as i64 + 1) * 1024))
            .collect::<Vec<_>>()
            .join(",");
        let declared = parse_provisioned_outposts(&setting);
        assert_eq!(
            declared.len(),
            count as usize,
            "case {count}: the setting {setting:?} must declare {count} Outpost(s)"
        );
        let router = emulator_provisioned_with(declared);

        // A page the whole inventory fits in: everything, and no token.
        for query in ["?maxResults=100", ""] {
            let answer = list_outposts(&router, query).await;
            assert_eq!(
                answer.outposts().len(),
                count as usize,
                "case {count} with {query:?}: the page must hold the whole inventory, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.next_token(),
                None,
                "case {count} with {query:?}: nothing remains beyond the page, so no NextToken is \
                 due, got {:?}",
                answer.next_token()
            );
        }

        // Page sizes from one up to the inventory's own size: the walk visits every
        // Outpost once and the last page of each walk carries no token.
        for size in 1..=count.max(1) {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let query = match token.as_deref() {
                    None => format!("?maxResults={size}"),
                    Some(cursor) => format!("?maxResults={size}&nextToken={cursor}"),
                };
                let answer = list_outposts(&router, &query).await;
                assert_eq!(
                    answer.status, 200,
                    "case {count}/{size}: page {pages} must answer 200, got {} with {:?}",
                    answer.status, answer.body
                );
                let page = answer.outposts();
                assert!(
                    page.len() <= size as usize,
                    "case {count}/{size}: a page must not be longer than MaxResults, got {:?}",
                    answer.body
                );
                seen.extend(page.iter().map(|outpost| string_at(outpost, "OutpostId")));
                token = answer.next_token();
                pages += 1;
                assert!(
                    pages <= count + 1,
                    "case {count}/{size}: paging must terminate, got {pages} pages for {count} \
                     Outpost(s)"
                );
                if token.is_none() {
                    break;
                }
            }
            let mut unique = seen.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {count}/{size}: paging must report each Outpost once, got {seen:?}"
            );
            assert_eq!(
                seen.len(),
                count as usize,
                "case {count}/{size}: paging must enumerate every Outpost, got {seen:?}"
            );
        }

        // A page SHORTER than the inventory does carry a cursor.
        if count >= 2 {
            let answer = list_outposts(&router, &format!("?maxResults={}", count - 1)).await;
            assert!(
                answer.next_token().is_some(),
                "case {count}: a page the inventory has records beyond must carry a NextToken, \
                 got {:?}",
                answer.body
            );
        }
    }
}

/// An Outpost that was provisioned is one `CreateEndpoint` can attach an endpoint to, and
/// the endpoint reads back through `ListEndpoints` — so the inventory the listing computes
/// is the same one the rest of the service resolves against.
#[tokio::test]
async fn an_endpoint_can_be_created_on_a_provisioned_outpost_only() {
    let unknown = "op-fffffffffffffffff";
    let router = emulator_provisioned_with(parse_provisioned_outposts(
        "op-0a1b2c3d4e5f60718:52776558133248",
    ));
    let provisioned = string_at(&list_outposts(&router, "").await.outposts()[0], "OutpostId");

    let refused = create(&router, minimal_create(unknown)).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (404, "ResourceNotFoundException"),
        "an Outpost outside the inventory must answer 404, got {} with {:?}",
        refused.status,
        refused.body
    );

    let created = create(
        &router,
        json!({
            "OutpostId": provisioned,
            "SubnetId": "subnet-0a1b2c3d4e5f60719",
            "SecurityGroupId": "sg-0a1b2c3d4e5f60719",
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "an endpoint must be creatable on a provisioned Outpost, got {} with {:?}",
        created.status, created.body
    );
    let arn = string_at(&created.body, "EndpointArn");

    let listed = list_endpoints(&router, "?maxResults=100").await;
    let arns: Vec<String> = listed
        .endpoints()
        .iter()
        .map(|endpoint| string_at(endpoint, "EndpointArn"))
        .collect();
    assert_eq!(
        arns,
        vec![arn.clone()],
        "ListEndpoints must report exactly the created endpoint, got {:?}",
        listed.body
    );
    assert_eq!(
        string_at(&listed.endpoints()[0], "OutpostsId"),
        provisioned,
        "the endpoint must be reported on the Outpost it was created on, got {:?}",
        listed.body
    );
}
