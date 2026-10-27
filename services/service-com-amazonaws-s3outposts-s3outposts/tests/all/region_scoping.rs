// SPDX-License-Identifier: CC0-1.0

//! Which Region a resource of this service belongs to, end to end over HTTP.
//!
//! The rule under test: an Outpost is installed in one Region and the S3 on Outposts
//! endpoints on it belong to that same Region, so a request is served exactly the
//! resources of the Region it was signed for — a resource of another Region is reported by
//! no listing of this one and every reference to it is the modelled 404 — and every ARN an
//! answer carries names the Region its own resource lives in, whichever Region's endpoint
//! the client is talking to.
//!
//! The rows span what the rule generalises over: the relationship between the Region a
//! resource lives in and the Region it is read from (another Region of the same partition,
//! a Region of another partition, a Region whose code shares or extends the other's prefix,
//! a Region nothing was ever provisioned in), and a home Region outside the commercial
//! partition read from inside it. Every case goes through the router, so each exercises
//! routing, deserialization, boundary validation, the handler, the persisted state and
//! serialization.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::Value;
use service_com_amazonaws_s3outposts_s3outposts::state::ENDPOINT_TRANSITION;

use crate::endpoint_lifecycle::{
    create_in, delete_endpoint_in, emulator_holding, emulator_in_region, endpoint_id_of,
    list_endpoints_in, list_outposts_in, list_shared_in, minimal_create, owned_outposts_in,
    string_at, Answer,
};
use smithy_server_runtime::MultiServiceRouter;

/// The one text a reference under an `OutpostId` that does not resolve answers with,
/// written out here rather than imported so a wrong constant in the service cannot make a
/// case pass (`not_found_messages.rs` is that text's own subject).
const OUTPOST_MISSING: &str =
    "The specified outpost does not exist, or does not have S3 capacity provisioned on it.";

/// The Region a resource is installed in, and a DIFFERENT Region it is read from.
struct Row {
    case: &'static str,
    home: &'static str,
    reader: &'static str,
}

/// One row per shape of relationship between the two Regions. The Region codes are AWS's
/// own, taken from the emulator's Region catalog (`emulator_telemetry::policy::KNOWN_REGIONS`,
/// "every AWS region the wire may carry"); the pairs that share or extend a prefix are here
/// so no case can pass by comparing Region codes loosely.
const ROWS: &[Row] = &[
    Row {
        case: "another Region of the same partition",
        home: "us-east-1",
        reader: "eu-west-1",
    },
    Row {
        case: "a Region whose code shares the home Region's prefix",
        home: "us-east-1",
        reader: "us-east-2",
    },
    Row {
        case: "a Region of another partition",
        home: "us-east-1",
        reader: "cn-north-1",
    },
    Row {
        case: "a home Region outside the commercial partition, read from inside it",
        home: "us-gov-west-1",
        reader: "us-east-1",
    },
    Row {
        case: "a Region whose code extends the home Region's",
        home: "us-iso-east-1",
        reader: "us-isob-east-1",
    },
    Row {
        case: "the home Region whose code extends the reader's",
        home: "us-isob-east-1",
        reader: "us-iso-east-1",
    },
    Row {
        case: "a Region nothing was ever provisioned in",
        home: "ap-southeast-2",
        reader: "me-central-1",
    },
];

/// The `EndpointArn`s an answer reports.
fn reported_arns(answer: &Answer) -> Vec<String> {
    answer
        .endpoints()
        .iter()
        .map(|endpoint| string_at(endpoint, "EndpointArn"))
        .collect()
}

/// The `OutpostId`s an inventory answer reports.
fn reported_outposts(answer: &Answer) -> Vec<String> {
    answer
        .outposts()
        .iter()
        .map(|outpost| string_at(outpost, "OutpostId"))
        .collect()
}

/// The `Message` a refusal carries, checked to be the service's own 404 text.
fn assert_outpost_missing(case: &str, what: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: {what} must answer 404 ResourceNotFoundException, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.message(),
        OUTPOST_MISSING,
        "case {case:?}: {what} must carry the service's own 404 text, got {:?}",
        answer.message()
    );
}

/// A resource is reached only through the Region it lives in: the Region it was created in
/// serves it and reports its ARN, and another Region reports nothing for it and refuses
/// every reference to it — without touching what is stored.
#[tokio::test]
async fn a_resource_is_reached_only_through_the_region_it_lives_in() {
    let account = engine::emulator_account_id();
    for Row { case, home, reader } in ROWS {
        // The fixture's Outposts are installed in `home`, so `reader` has none at all.
        let router = emulator_in_region(home);
        let outpost_id = owned_outposts_in(&router, home)
            .await
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the emulator presents an owned Outpost"))
            .clone();

        let created = create_in(&router, home, minimal_create(&outpost_id)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateEndpoint in {home} must answer 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_at(&created.body, "EndpointArn");
        let endpoint_id = endpoint_id_of(&arn);
        // Everything after the partition field, which is the Region's own and is
        // `arn_partitions.rs`'s subject.
        assert!(
            arn.ends_with(&format!(
                ":s3-outposts:{home}:{account}:outpost/{outpost_id}/endpoint/{endpoint_id}"
            )),
            "case {case:?}: the EndpointArn must name the endpoint's own Region {home}, its \
             owning account and its ids, got {arn:?}"
        );
        assert!(
            !arn.contains(&format!(":{reader}:")),
            "case {case:?}: the EndpointArn must not name the reading Region {reader}, got {arn:?}"
        );

        // The Region it lives in serves it: both listings report it, under that one ARN.
        let at_home = list_endpoints_in(&router, home, "?maxResults=100").await;
        assert_eq!(
            reported_arns(&at_home),
            vec![arn.clone()],
            "case {case:?}: {home} must report exactly the endpoint created in it, got {:?}",
            at_home.body
        );
        let shared_at_home =
            list_shared_in(&router, home, &format!("?outpostId={outpost_id}&maxResults=100")).await;
        assert_eq!(
            reported_arns(&shared_at_home),
            vec![arn.clone()],
            "case {case:?}: the Outpost's own listing in {home} must report it, got {:?}",
            shared_at_home.body
        );
        assert!(
            reported_outposts(&list_outposts_in(&router, home, "?maxResults=100").await)
                .contains(&outpost_id),
            "case {case:?}: {home} must report the Outpost installed in it"
        );

        // The other Region reports nothing of it, with no cursor promising more.
        let elsewhere = list_endpoints_in(&router, reader, "?maxResults=100").await;
        assert_eq!(
            elsewhere.status, 200,
            "case {case:?}: ListEndpoints in {reader} must answer 200, got {} with {:?}",
            elsewhere.status, elsewhere.body
        );
        assert_eq!(
            reported_arns(&elsewhere),
            Vec::<String>::new(),
            "case {case:?}: {reader} must report no endpoint of {home}, got {:?}",
            elsewhere.body
        );
        assert_eq!(
            elsewhere.next_token(),
            None,
            "case {case:?}: an empty listing carries no NextToken, got {:?}",
            elsewhere.next_token()
        );
        let inventory = list_outposts_in(&router, reader, "?maxResults=100").await;
        assert_eq!(
            reported_outposts(&inventory),
            Vec::<String>::new(),
            "case {case:?}: {reader} must report no Outpost of {home}, got {:?}",
            inventory.body
        );

        // And every reference to it from that Region is the modelled 404.
        assert_outpost_missing(
            case,
            &format!("ListSharedEndpoints in {reader}"),
            &list_shared_in(
                &router,
                reader,
                &format!("?outpostId={outpost_id}&maxResults=100"),
            )
            .await,
        );
        assert_outpost_missing(
            case,
            &format!("CreateEndpoint in {reader}"),
            &create_in(&router, reader, minimal_create(&outpost_id)).await,
        );
        assert_outpost_missing(
            case,
            &format!("DeleteEndpoint in {reader}"),
            &delete_endpoint_in(&router, reader, &outpost_id, &endpoint_id).await,
        );

        // None of those refusals changed what is stored, and the ARN did not move.
        let after = list_endpoints_in(&router, home, "?maxResults=100").await;
        assert_eq!(
            reported_arns(&after),
            vec![arn.clone()],
            "case {case:?}: a refusal in {reader} must leave {home}'s endpoint exactly as it \
             was, got {:?}",
            after.body
        );

        // The Region it lives in is the one that can remove it. The teardown that starts
        // is the clock's to complete, so the record goes when the span has passed.
        let deleted = delete_endpoint_in(&router, home, &outpost_id, &endpoint_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteEndpoint in {home} must answer 200, got {} with {:?}",
            deleted.status, deleted.body
        );
        std::thread::sleep(ENDPOINT_TRANSITION + Duration::from_millis(200));
        assert_eq!(
            reported_arns(&list_endpoints_in(&router, home, "?maxResults=100").await),
            Vec::<String>::new(),
            "case {case:?}: the deleted endpoint must be gone from {home}"
        );
    }
}

/// One store holding Outposts in several Regions answers each Region with exactly that
/// Region's own records: every record is reachable through its own Region, no record is
/// reported by two Regions, and each ARN names the Region of the record it identifies — so
/// one stored endpoint has exactly ONE ARN however many Regions are read.
#[tokio::test]
async fn each_region_answers_with_its_own_records_and_one_arn_per_record() {
    let account = engine::emulator_account_id();
    // The Regions whose resources this store holds: two in one partition, one in another,
    // and one whose code extends another's.
    const REGIONS: &[&str] = &["us-east-1", "us-east-2", "cn-northwest-1", "us-isob-east-1"];
    // A Region signed for but never provisioned in: it holds nothing, so it answers
    // nothing.
    const UNPROVISIONED: &str = "eu-central-1";

    let declared: Vec<(String, &str)> = REGIONS
        .iter()
        .enumerate()
        .map(|(index, region)| (format!("op-{index:017x}"), *region))
        .collect();
    let router = emulator_holding(&declared);

    // One endpoint on each Outpost, created through the Region that Outpost is in.
    let mut minted: Vec<(String, &str, String)> = Vec::new();
    for (outpost_id, region) in &declared {
        let created = create_in(&router, region, minimal_create(outpost_id)).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: CreateEndpoint must answer 200, got {} with {:?}",
            created.status, created.body
        );
        minted.push((
            outpost_id.clone(),
            region,
            string_at(&created.body, "EndpointArn"),
        ));
    }

    let mut seen_arns: BTreeSet<String> = BTreeSet::new();
    for (outpost_id, region, arn) in &minted {
        let case = format!("{outpost_id} in {region}");

        let endpoints = list_endpoints_in(&router, region, "?maxResults=100").await;
        assert_eq!(
            reported_arns(&endpoints),
            vec![arn.clone()],
            "case {case:?}: the Region must report exactly its own endpoint, got {:?}",
            endpoints.body
        );
        let outposts = list_outposts_in(&router, region, "?maxResults=100").await;
        assert_eq!(
            reported_outposts(&outposts),
            vec![outpost_id.clone()],
            "case {case:?}: the Region must report exactly its own Outpost, got {:?}",
            outposts.body
        );

        let endpoint_id = endpoint_id_of(arn);
        assert!(
            arn.ends_with(&format!(
                ":s3-outposts:{region}:{account}:outpost/{outpost_id}/endpoint/{endpoint_id}"
            )),
            "case {case:?}: the EndpointArn must name its own Region, got {arn:?}"
        );
        let outpost = outposts.outposts()[0].clone();
        for member in ["OutpostArn", "S3OutpostArn"] {
            let reported = string_at(&outpost, member);
            assert!(
                reported.contains(&format!(":{region}:")),
                "case {case:?}: the {member} must name the Outpost's own Region, got {reported:?}"
            );
        }

        assert!(
            seen_arns.insert(arn.clone()),
            "case {case:?}: no two records may be reported under one ARN, {arn:?} repeated"
        );
    }

    // A Region the store holds nothing in answers with nothing, and the Outposts of the
    // other Regions do not resolve there.
    let empty = list_endpoints_in(&router, UNPROVISIONED, "?maxResults=100").await;
    assert_eq!(
        reported_arns(&empty),
        Vec::<String>::new(),
        "a Region nothing was provisioned in reports no endpoint, got {:?}",
        empty.body
    );
    let empty_inventory = list_outposts_in(&router, UNPROVISIONED, "?maxResults=100").await;
    assert_eq!(
        reported_outposts(&empty_inventory),
        Vec::<String>::new(),
        "a Region nothing was provisioned in reports no Outpost, got {:?}",
        empty_inventory.body
    );
    for (outpost_id, region, _) in &minted {
        assert_outpost_missing(
            &format!("{outpost_id} of {region}"),
            &format!("ListSharedEndpoints in {UNPROVISIONED}"),
            &list_shared_in(
                &router,
                UNPROVISIONED,
                &format!("?outpostId={outpost_id}&maxResults=100"),
            )
            .await,
        );
    }

    // Every record the store holds was reached, each through its own Region alone.
    assert_eq!(
        seen_arns.len(),
        minted.len(),
        "every stored endpoint must be reachable through its own Region, got {seen_arns:?}"
    );
}

/// A request that carries no SigV4 credential scope to read a Region from is served the
/// emulator's default Region, and still only that Region's resources.
///
/// The default is the Region an Outpost an operator declared no Region for is installed in,
/// so the two agree: a scope-less caller reaches exactly what such an operator provisioned.
#[tokio::test]
async fn a_request_with_no_credential_scope_is_served_the_default_region() {
    const DEFAULT: &str = "us-east-1";
    const OTHER: &str = "eu-west-1";
    let router = emulator_holding(&[
        ("op-00000000000000001".to_string(), DEFAULT),
        ("op-00000000000000002".to_string(), OTHER),
    ]);

    let answer = unsigned(&router, "GET", "/S3Outposts/ListOutpostsWithS3?maxResults=100").await;
    assert_eq!(
        answer.status, 200,
        "ListOutpostsWithS3 must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        reported_outposts(&answer),
        vec!["op-00000000000000001".to_string()],
        "a request with no credential scope is served the default Region's Outposts, got {:?}",
        answer.body
    );
}

/// A request with no `authorization` header at all, so nothing names a Region for it.
async fn unsigned(router: &MultiServiceRouter, method: &str, uri: &str) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "s3-outposts.amazonaws.com")
        .body(::bytes::Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}
