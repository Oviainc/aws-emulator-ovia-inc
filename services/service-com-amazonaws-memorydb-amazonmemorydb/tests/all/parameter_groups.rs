// SPDX-License-Identifier: CC0-1.0

//! MemoryDB parameter groups, end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router, so what
//! is asserted is the emulator's HTTP behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * `CreateParameterGroup` persists the group and answers the stored record: `Name`
//!   and `Family` folded to lower case (a MemoryDB resource name is a case-insensitive
//!   identifier), `Description` as given or the service's default when none was given,
//!   and an `ARN` scoped to the region the request was signed for;
//! * `DescribeParameterGroups` answers from that same state — the one group a request
//!   names, or every stored group in name order — so a read reports exactly what the
//!   create persisted;
//! * a read changes nothing a later caller sees: two describes of the same store agree;
//! * re-creating a stored group is the modelled already-exists fault and nothing in the
//!   store moves; naming a group that is not stored is the modelled not-found fault;
//! * a listing pages over the whole set in name order for any `MaxResults`, and a page
//!   size or token that names no page is the modelled invalid-value error.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_memorydb_amazonmemorydb::state::{
    default_parameter_group_name, parameter_group_families,
};
use service_com_amazonaws_memorydb_amazonmemorydb::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for. Every ARN the emulator mints must
/// carry it, rather than a constant baked into the service.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One `CreateParameterGroup` request, and what the service must report for it.
///
/// The rows differ along everything `CreateParameterGroup`'s rule generalises over: the
/// group name, the parameter group family, whether a description was supplied, and
/// whether tags were. The families are the ones MemoryDB publishes for its engines; the
/// model carries no family catalog (`smithy_query query="shape" shape="ParameterGroup"`
/// documents `Family` as free text), so the service records whichever the caller sends
/// and these rows only have to differ from one another.
struct Case {
    name: &'static str,
    family: &'static str,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

const CASES: &[Case] = &[
    // No description and no tags: both optional members absent.
    Case {
        name: "alpha-group",
        family: "memorydb_redis7",
        description: None,
        tags: &[],
    },
    // A description, no tags.
    Case {
        name: "beta-group",
        family: "memorydb_redis6",
        description: Some("tuned for the batch tier"),
        tags: &[],
    },
    // A description AND tags.
    Case {
        name: "gamma-group",
        family: "memorydb_valkey7",
        description: Some("multi-tag group"),
        tags: &[("team", "platform"), ("env", "test")],
    },
    // An EMPTY description is still a description: it must round-trip as `""`, not as
    // an absent member.
    Case {
        name: "delta-group",
        family: "memorydb_valkey8",
        description: Some(""),
        tags: &[("only", "one")],
    },
    // A long, digit- and hyphen-bearing name, sorting LAST of these rows: the listing
    // order the pagination cases rely on is the store's, not the insertion order.
    Case {
        name: "zeta-group-0042-with-a-much-longer-name",
        family: "memorydb_redis7",
        description: Some("long name"),
        tags: &[],
    },
];

/// A router with just this service mounted, on one shared engine — what
/// `core/router` builds, narrowed to the service under test.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation, signed for `region`, and return `(status, body)`.
async fn call_signed(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://memory-db.{region}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonMemoryDB.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/memorydb/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes) }))
    };
    (status, parsed)
}

/// Call one operation signed for [`REGION`], and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_signed(router, REGION, operation, body).await
}

/// Call an operation that must succeed, returning its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let (status, body) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {body}"
    );
    body
}

/// The wire error code of a failure body (`__type`, with any namespace prefix dropped).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `CreateParameterGroup` request body for one case.
fn create_body(case: &Case) -> Value {
    let mut body = json!({ "ParameterGroupName": case.name, "Family": case.family });
    if let Some(description) = case.description {
        body["Description"] = json!(description);
    }
    if !case.tags.is_empty() {
        body["Tags"] = Value::Array(
            case.tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        );
    }
    body
}

/// What the service must report for one case, in the region it was created in.
fn expected_group(case: &Case, region: &str) -> Value {
    json!({
        "Name": case.name,
        "Family": case.family,
        // A group created without a `Description` carries the service's own default
        // (`crate::state::DEFAULT_PARAMETER_GROUP_DESCRIPTION`), not an absent member.
        "Description": case.description.unwrap_or(" "),
        "ARN": format!("arn:aws:memorydb:{region}:{ACCOUNT}:parametergroup/{}", case.name),
    })
}

/// `ParameterGroups[*].Name` of a `DescribeParameterGroups` body, in answer order.
fn group_names(body: &Value, case: &str) -> Vec<String> {
    body.get("ParameterGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected ParameterGroups in {body}"))
        .iter()
        .map(|group| {
            group
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected Name in {group}"))
                .to_string()
        })
        .collect()
}

/// Every name a full listing must report, in name order: the groups a row created plus
/// the `default.<family>` groups MemoryDB itself provides in every account (one per
/// family of the engine catalog), which exist without being created.
fn expected_listing(created: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = created.iter().map(|name| name.to_string()).collect();
    names.extend(
        parameter_group_families()
            .iter()
            .map(|family| default_parameter_group_name(family)),
    );
    names.sort();
    names
}

/// A create persists the group, and a later describe answers from that state: the same
/// name, family, description and ARN come back, whichever members the request carried.
#[tokio::test]
async fn a_created_parameter_group_is_reported_by_a_later_describe() {
    let router = router();

    for case in CASES {
        let created = ok(&router, "CreateParameterGroup", &create_body(case), case.name).await;
        assert_eq!(
            created["ParameterGroup"],
            expected_group(case, REGION),
            "case {:?}: the create must answer the record it persisted, got {created}",
            case.name
        );
    }

    // Read every one of them back through the implemented reader, one at a time.
    for case in CASES {
        let described = ok(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": case.name }),
            case.name,
        )
        .await;
        let groups = described["ParameterGroups"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: expected a list, got {described}", case.name));
        assert_eq!(
            groups.len(),
            1,
            "case {:?}: naming one group must answer exactly that group, got {described}",
            case.name
        );
        assert_eq!(
            groups[0],
            expected_group(case, REGION),
            "case {:?}: the describe must report what the create persisted, got {described}",
            case.name
        );
    }

    // And the unfiltered listing is every stored group, in name order — the groups
    // these cases created AND the AWS-owned default group of every family, which the
    // account already had.
    let listed = ok(&router, "DescribeParameterGroups", &json!({}), "all").await;
    let expected = expected_listing(&CASES.iter().map(|case| case.name).collect::<Vec<_>>());
    assert_eq!(
        group_names(&listed, "all"),
        expected,
        "case {:?}: the listing must report every stored group in name order, got {listed}",
        "all"
    );
}

/// A group belongs to the region the request that created it was signed for, and it
/// keeps that region: its ARN does not follow a later caller's region.
#[tokio::test]
async fn a_parameter_group_keeps_the_region_it_was_created_in() {
    const OTHER_REGION: &str = "eu-west-1";
    let router = router();

    for region in [REGION, OTHER_REGION] {
        let case = &CASES[1];
        let name = format!("{}-{region}", case.name);
        let mut body = create_body(case);
        body["ParameterGroupName"] = json!(name);
        let (status, created) = call_signed(&router, region, "CreateParameterGroup", &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200, got {status} with {created}"
        );
        assert_eq!(
            created["ParameterGroup"]["ARN"],
            json!(format!(
                "arn:aws:memorydb:{region}:{ACCOUNT}:parametergroup/{name}"
            )),
            "case {name:?}: the ARN must carry the request's region, got {created}"
        );

        // Described from the OTHER region, the group reports the same ARN.
        let elsewhere = if region == REGION { OTHER_REGION } else { REGION };
        let (status, described) = call_signed(
            &router,
            elsewhere,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from {elsewhere}, got {status} with {described}"
        );
        assert_eq!(
            described["ParameterGroups"][0]["ARN"],
            json!(format!(
                "arn:aws:memorydb:{region}:{ACCOUNT}:parametergroup/{name}"
            )),
            "case {name:?}: a describe signed for {elsewhere} must not move the group, \
             got {described}"
        );
    }
}

/// A describe changes nothing a later caller sees: two reads of the same store agree,
/// and the create's own answer is still what the second read reports.
#[tokio::test]
async fn describing_a_parameter_group_changes_nothing() {
    let router = router();
    for case in CASES {
        ok(&router, "CreateParameterGroup", &create_body(case), case.name).await;
    }

    let first = ok(&router, "DescribeParameterGroups", &json!({}), "first read").await;
    let second = ok(&router, "DescribeParameterGroups", &json!({}), "second read").await;
    assert_eq!(
        first, second,
        "case {:?}: a read must not change what the next read sees, got {first} then {second}",
        "repeat read"
    );
    for case in CASES {
        let described = ok(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": case.name }),
            case.name,
        )
        .await;
        assert_eq!(
            described["ParameterGroups"][0],
            expected_group(case, REGION),
            "case {:?}: the third read must still report the stored record, got {described}",
            case.name
        );
    }
}

/// The listing pages over the whole set in name order for ANY page size, and the pages
/// together are exactly the unpaged listing.
#[tokio::test]
async fn the_listing_pages_over_every_group_in_name_order() {
    let router = router();
    for case in CASES {
        ok(&router, "CreateParameterGroup", &create_body(case), case.name).await;
    }
    let mut expected: Vec<String> =
        expected_listing(&CASES.iter().map(|case| case.name).collect::<Vec<_>>());
    expected.sort();

    // Every page size from 1 to one PAST the number of groups: a size larger than the
    // set is one page with no token.
    for page_size in 1..=(expected.len() + 1) {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // At most one page per group, plus one: a page size of at least 1 always makes
        // progress, so this cannot loop forever.
        for _ in 0..=expected.len() {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let page = ok(&router, "DescribeParameterGroups", &body, &case).await;
            let names = group_names(&page, &case);
            assert!(
                names.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} groups, got {page}"
            );
            seen.extend(names);
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the listing must end, still holding token {token:?}"
        );
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages together must be the whole listing in name order"
        );
    }
}

/// Re-creating a stored group is refused with the modelled already-exists fault, and
/// the stored group is left exactly as it was.
#[tokio::test]
async fn recreating_a_stored_group_is_refused_and_changes_nothing() {
    let router = router();

    for case in CASES {
        ok(&router, "CreateParameterGroup", &create_body(case), case.name).await;
    }

    for case in CASES {
        // The second create differs from the first in family and description, so a
        // refusal that silently overwrote the record would be visible below.
        let body = json!({
            "ParameterGroupName": case.name,
            "Family": "memorydb_redis6",
            "Description": "an overwrite that must not happen",
        });
        let (status, refused) = call(&router, "CreateParameterGroup", &body).await;
        assert_eq!(
            status, 400,
            "case {:?}: re-creating a stored group must answer 400, got {status} with {refused}",
            case.name
        );
        assert_eq!(
            error_code(&refused),
            "ParameterGroupAlreadyExistsFault",
            "case {:?}: expected the modelled already-exists fault, got {refused}",
            case.name
        );
        let described = ok(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": case.name }),
            case.name,
        )
        .await;
        assert_eq!(
            described["ParameterGroups"][0],
            expected_group(case, REGION),
            "case {:?}: the refused create must leave the group unchanged, got {described}",
            case.name
        );
    }
}

/// Naming a group that is not in state is the modelled not-found fault, and a page size
/// or token that names no page is the modelled invalid-value error.
#[tokio::test]
async fn a_request_that_names_nothing_is_refused() {
    let router = router();
    ok(
        &router,
        "CreateParameterGroup",
        &create_body(&CASES[0]),
        CASES[0].name,
    )
    .await;

    // Unknown names, including one that is a prefix of a stored name, one that extends
    // it and one that differs by a trailing space: a name is folded to lower case
    // before it is looked up, but nothing else about it is normalised.
    for name in [
        "no-such-group",
        "alpha",
        "alpha-group-2",
        "alpha-group ",
    ] {
        let (status, refused) = call(
            &router,
            "DescribeParameterGroups",
            &json!({ "ParameterGroupName": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: an unknown group must answer 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ParameterGroupNotFoundFault",
            "case {name:?}: expected the modelled not-found fault, got {refused}"
        );
    }

    // A page size below 1 names no page.
    for max_results in [0, -1, -7] {
        let (status, refused) = call(
            &router,
            "DescribeParameterGroups",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {max_results:?}: a page size below 1 must answer 400, got {status} with \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParameterValueException",
            "case {max_results:?}: expected the modelled invalid-value error, got {refused}"
        );
    }

    // A token this listing never minted names no position in it.
    for token in ["zzzz-not-a-position", "~"] {
        let (status, refused) = call(
            &router,
            "DescribeParameterGroups",
            &json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: a token past the listing must answer 400, got {status} with \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParameterValueException",
            "case {token:?}: expected the modelled invalid-value error, got {refused}"
        );
    }
}
