// SPDX-License-Identifier: CC0-1.0

//! The Directory Service directory lifecycle, driven end to end over HTTP.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. Nothing reaches into the state struct, so a
//! response that is not computed from persisted state cannot pass.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call: the
//! directory record over a table of create requests that differ in size, short name,
//! description, VPC settings, network type and tag count; the `Alias`/`AccessUrl`
//! derivation over a table of alias forms including both published length bounds; the
//! network types a Simple AD directory admits over the whole modelled `NetworkType`
//! catalog; pagination over a table of page sizes spanning 1, several, exactly the
//! record count and past it; and tag merging over a table of overlapping tag sets.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name, `DirectoryService_20150416`.
const TARGET_PREFIX: &str = "DirectoryService_20150416";

/// A password satisfying the modelled `Password` pattern: 8–64 characters with digits,
/// upper case, lower case and a special character. It is never asserted on — no
/// operation reports it back — and exists only so the create requests are well formed.
pub(crate) const PASSWORD: &str = "Passw0rd!2345";

/// The stage a directory is in between the create and the transition that create owes:
/// the first value of the modelled `DirectoryStage` enum.
pub(crate) const STAGE_REQUESTED: &str = "Requested";

/// The stage a created directory reaches once that transition falls due.
pub(crate) const STAGE_ACTIVE: &str = "Active";

/// The network types `CreateDirectoryRequest$NetworkType`'s documentation says a Simple
/// AD directory admits: "Simple AD supports IPv4 and Dual-stack only."
const SIMPLE_AD_NETWORK_TYPES: [&str; 2] = ["IPv4", "Dual-stack"];

/// Directory Service's published Region quota of Simple AD directories, which every
/// `CreateDirectory` counts against: "Simple AD directories: 10" per Region.
pub(crate) const CLOUD_ONLY_QUOTA: usize = 10;

/// Directory Service's published Region quota of AWS Managed Microsoft AD directories,
/// which every `CreateMicrosoftAD` counts against: "AWS Managed Microsoft AD
/// directories: 20" per Region — its own allowance, double the Simple AD one.
pub(crate) const MICROSOFT_AD_QUOTA: usize = 20;

/// Directory Service's published Region quota of AD Connector directories:
/// "AD Connector directories: 10" per Region. Nothing persists one yet
/// (`ConnectDirectory` is still a scaffold stub), so it is only ever reported.
pub(crate) const CONNECTED_QUOTA: usize = 10;

/// AWS's tagging limit of 50 tags per resource.
pub(crate) const TAG_QUOTA: usize = 50;

/// Comfortably longer than one clock-owed transition span, with margin for a loaded
/// test machine.
pub(crate) const PAST_ONE_SPAN: Duration = Duration::from_millis(900);

/// The Region the calls below are signed for, and the one a minted ARN therefore
/// carries. `tests/all/event_topics.rs` and
/// `tests/all/directory_networking_and_settings.rs` reuse the harness below with other
/// Regions, which is why [`call_in_region`] takes it as a parameter.
pub(crate) const REGION: &str = "us-east-1";

/// A router with just this service mounted on a fresh engine.
///
/// This suite's harness is shared with the sibling suites in `tests/all/`, which reach
/// it as `crate::directory_lifecycle::<item>` rather than standing up a second copy.
pub(crate) fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    emulator_on(engine)
}

/// A router with just this service mounted on `engine`, so a test that needs to read
/// or seed the store directly can hold the same engine the handlers serve from.
pub(crate) fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The tests exercise the service, not SigV4: requests carry a credential scope so
    // the router can name the target service, but no real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One `awsJson1_1` call signed for [`REGION`], returning the HTTP status and the
/// parsed response body.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in_region(router, operation, body, REGION).await
}

/// One `awsJson1_1` call whose sigv4 credential scope names `region`, returning the
/// HTTP status and the parsed response body. The credential scope is the only place a
/// request carries the Region an operation such as `RemoveRegion` acts on.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    region: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ds.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDDSTEST/20260101/{region}/ds/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
pub(crate) async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 and the given modelled error code.
pub(crate) async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected {expected_code} from {operation}, got {parsed}"
    );
}

/// A required string member of a response body.
pub(crate) fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// The modelled `DirectoryId` pattern, `^d-[0-9a-f]{10}$`, as an assertion.
pub(crate) fn assert_directory_id(id: &str, case: &str) {
    let suffix = id.strip_prefix("d-").unwrap_or_default();
    assert!(
        suffix.len() == 10 && suffix.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "case {case:?}: expected a DirectoryId matching ^d-[0-9a-f]{{10}}$, got {id:?}"
    );
}

/// `CreateDirectory` with `body`, asserting it was accepted and returning the id.
pub(crate) async fn create_directory(
    router: &MultiServiceRouter,
    body: Value,
    case: &str,
) -> String {
    let answer = ok(router, "CreateDirectory", body, case).await;
    let id = string(&answer, "DirectoryId", case);
    assert_directory_id(&id, case);
    id
}

/// A minimal well-formed `CreateDirectory` body for `name`.
pub(crate) fn minimal_create(name: &str) -> Value {
    json!({ "Name": name, "Password": PASSWORD, "Size": "Small" })
}

/// The `DirectoryDescriptions` of a `DescribeDirectories` answer.
pub(crate) fn descriptions(body: &Value, case: &str) -> Vec<Value> {
    body["DirectoryDescriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected DirectoryDescriptions, got {body}"))
        .clone()
}

/// Every stored directory, unfiltered and unpaginated.
async fn describe_all(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let answer = ok(router, "DescribeDirectories", json!({}), case).await;
    descriptions(&answer, case)
}

/// The one description `DescribeDirectories` reports for `directory_id`.
pub(crate) async fn describe_one(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> Value {
    let answer = ok(
        router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [directory_id] }),
        case,
    )
    .await;
    let found = descriptions(&answer, case);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one description for {directory_id}, got {found:?}"
    );
    found[0].clone()
}

/// The `Tags` of a `ListTagsForResource` answer, as a key -> value map.
pub(crate) fn tag_map(body: &Value, case: &str) -> BTreeMap<String, String> {
    body["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Tags, got {body}"))
        .iter()
        .map(|tag| {
            (
                string(tag, "Key", case),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// One row of the create table: a request and what the stored record must then report.
struct CreateCase {
    name: &'static str,
    body: Value,
    expect: Vec<(&'static str, Value)>,
}

#[tokio::test]
async fn a_created_directory_is_reported_from_persisted_state() {
    let router = service();

    // The rows differ along everything `CreateDirectory` persists: size, whether a
    // short name / description / VPC settings / network type were given, and how many
    // tags came with the request.
    let cases = vec![
        CreateCase {
            name: "minimal Small",
            body: minimal_create("corp.example.com"),
            expect: vec![
                ("Name", json!("corp.example.com")),
                ("Size", json!("Small")),
                ("Type", json!("SimpleAD")),
                ("SsoEnabled", json!(false)),
            ],
        },
        CreateCase {
            name: "Large with short name and description",
            body: json!({
                "Name": "sales.example.org",
                "Password": PASSWORD,
                "Size": "Large",
                "ShortName": "SALES",
                "Description": "Sales directory"
            }),
            expect: vec![
                ("Name", json!("sales.example.org")),
                ("Size", json!("Large")),
                ("ShortName", json!("SALES")),
                ("Description", json!("Sales directory")),
            ],
        },
        CreateCase {
            name: "with VPC settings and network type",
            body: json!({
                "Name": "eng-dept.example.com",
                "Password": PASSWORD,
                "Size": "Small",
                "NetworkType": "Dual-stack",
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                }
            }),
            expect: vec![
                ("NetworkType", json!("Dual-stack")),
                (
                    "VpcSettings",
                    json!({
                        "VpcId": "vpc-0a1b2c3d",
                        "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                    }),
                ),
            ],
        },
        CreateCase {
            name: "with three tags",
            body: json!({
                "Name": "ops.example.com",
                "Password": PASSWORD,
                "Size": "Small",
                "Tags": [
                    { "Key": "env", "Value": "dev" },
                    { "Key": "owner", "Value": "team-a" },
                    { "Key": "cost-centre", "Value": "" }
                ]
            }),
            expect: vec![("Name", json!("ops.example.com"))],
        },
    ];

    let mut ids: Vec<String> = Vec::new();
    for case in &cases {
        let id = create_directory(&router, case.body.clone(), case.name).await;
        ids.push(id);
    }

    // Two creates must yield two distinct ids and two distinct records.
    let distinct: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "expected {} distinct directory ids, got {ids:?}",
        ids.len()
    );

    for (case, id) in cases.iter().zip(&ids) {
        let described = describe_one(&router, id, case.name).await;
        for (member, expected) in &case.expect {
            assert_eq!(
                &described[*member], expected,
                "case {:?}: expected {member} to be {expected}, got {described}",
                case.name
            );
        }
        // Every description names the directory it was read back by, and derives its
        // alias and access URL from it: "If no alias exists, the alias is the directory
        // identifier", and the access URL is the HOST that alias names in the
        // `awsapps.com` domain — a bare host, with no scheme for a client to trip over.
        assert_eq!(
            described["DirectoryId"],
            json!(id),
            "case {:?}: expected DirectoryId {id}, got {described}",
            case.name
        );
        assert_eq!(
            described["Alias"],
            json!(id),
            "case {:?}: expected Alias to default to the directory id, got {described}",
            case.name
        );
        assert_eq!(
            described["AccessUrl"],
            json!(format!("{id}.awsapps.com")),
            "case {:?}: expected the access URL derived from the alias, got {described}",
            case.name
        );
        assert!(
            described["LaunchTime"].is_number(),
            "case {:?}: expected a LaunchTime timestamp, got {described}",
            case.name
        );
    }

    // The tags submitted with the fourth create are the tags the store holds for it.
    let tags = ok(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": ids[3] }),
        "with three tags",
    )
    .await;
    assert_eq!(
        tag_map(&tags, "with three tags"),
        BTreeMap::from([
            ("env".to_string(), "dev".to_string()),
            ("owner".to_string(), "team-a".to_string()),
            ("cost-centre".to_string(), String::new()),
        ]),
        "expected CreateDirectory's tags to be the tags ListTagsForResource reports"
    );
    // A directory created without tags reports none, not the previous directory's.
    let untagged = ok(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": ids[0] }),
        "minimal Small",
    )
    .await;
    assert!(
        tag_map(&untagged, "minimal Small").is_empty(),
        "expected no tags for a directory created without any, got {untagged}"
    );
}

#[tokio::test]
async fn a_created_directory_leaves_the_requested_stage_when_the_clock_says_so() {
    let router = service();
    let case = "stage transition";
    let id = create_directory(&router, minimal_create("corp.example.com"), case).await;

    // A caller acting straight after the create still sees the transient stage the
    // modelled DirectoryStage enum opens with.
    let fresh = describe_one(&router, &id, case).await;
    assert_eq!(
        fresh["Stage"],
        json!(STAGE_REQUESTED),
        "case {case:?}: expected the transient stage straight after the create, got {fresh}"
    );

    tokio::time::sleep(PAST_ONE_SPAN).await;

    let settled = describe_one(&router, &id, case).await;
    assert_eq!(
        settled["Stage"],
        json!(STAGE_ACTIVE),
        "case {case:?}: expected the stage the clock owed, got {settled}"
    );
    let launched = fresh["LaunchTime"].as_f64().unwrap_or_default();
    let updated = settled["StageLastUpdatedDateTime"]
        .as_f64()
        .unwrap_or_default();
    assert!(
        updated >= launched,
        "case {case:?}: expected StageLastUpdatedDateTime ({updated}) at or after LaunchTime ({launched})"
    );
    // Reading twice does not move the stage on again.
    let again = describe_one(&router, &id, case).await;
    assert_eq!(
        again["Stage"], settled["Stage"],
        "case {case:?}: expected a second read to report the same stage, got {again}"
    );
}

#[tokio::test]
async fn an_alias_drives_the_access_url_and_is_assigned_once() {
    let router = service();

    // The alias forms differ along the modelled `AliasName` constraints: both
    // published length bounds (1 and 62), a hyphenated form and a digit-bearing one.
    let aliases = [
        ("minimum length", "a".to_string()),
        ("hyphenated", "corp-dev".to_string()),
        ("with digits", "corp2024".to_string()),
        ("maximum length", format!("a{}", "b".repeat(61))),
    ];

    for (index, (case, alias)) in aliases.iter().enumerate() {
        let id = create_directory(
            &router,
            minimal_create(&format!("dir{index}.example.com")),
            case,
        )
        .await;
        // An alias is assigned only to a directory that has reached its serving stage.
        tokio::time::sleep(PAST_ONE_SPAN).await;

        let answer = ok(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": &id, "Alias": alias }),
            case,
        )
        .await;
        assert_eq!(
            answer["DirectoryId"],
            json!(&id),
            "case {case:?}: expected CreateAlias to report the directory it stored the alias on, got {answer}"
        );
        assert_eq!(
            answer["Alias"],
            json!(alias),
            "case {case:?}: expected CreateAlias to report the stored alias, got {answer}"
        );

        let described = describe_one(&router, &id, case).await;
        assert_eq!(
            described["Alias"],
            json!(alias),
            "case {case:?}: expected the stored alias to be reported, got {described}"
        );
        assert_eq!(
            described["AccessUrl"],
            json!(format!("{alias}.awsapps.com")),
            "case {case:?}: expected the access URL derived from the stored alias, got {described}"
        );

        // "After an alias has been created, it cannot be deleted or reused": neither a
        // second alias for this directory nor this alias for another one is accepted.
        refused(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": &id, "Alias": "another" }),
            "EntityAlreadyExistsException",
            &format!("{case}: second alias on the same directory"),
        )
        .await;
        let other = create_directory(
            &router,
            minimal_create(&format!("other{index}.example.com")),
            case,
        )
        .await;
        tokio::time::sleep(PAST_ONE_SPAN).await;
        refused(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": &other, "Alias": alias }),
            "EntityAlreadyExistsException",
            &format!("{case}: alias already taken"),
        )
        .await;
        // The refusal left the other directory's alias at its default.
        let untouched = describe_one(&router, &other, case).await;
        assert_eq!(
            untouched["Alias"],
            json!(&other),
            "case {case:?}: expected a refused CreateAlias to store nothing, got {untouched}"
        );
    }

    refused(
        &router,
        "CreateAlias",
        json!({ "DirectoryId": "d-00000000ff", "Alias": "nowhere" }),
        "EntityDoesNotExistException",
        "alias for an unknown directory",
    )
    .await;
}

#[tokio::test]
async fn simple_ad_admits_only_the_network_types_the_model_documents() {
    let router = service();

    // The catalog is the whole modelled `NetworkType` enum (Dual-stack | IPv4 | IPv6),
    // with the accept/refuse column taken from `CreateDirectoryRequest$NetworkType`'s
    // own documentation ("Simple AD supports IPv4 and Dual-stack only") rather than
    // from the implementation, so a change to either side is caught here.
    let catalog = [
        ("Dual-stack", true),
        ("IPv4", true),
        ("IPv6", false),
    ];

    for (index, (network_type, admitted)) in catalog.iter().enumerate() {
        let body = json!({
            "Name": format!("net{index}.example.com"),
            "Password": PASSWORD,
            "Size": "Small",
            "NetworkType": network_type
        });
        assert_eq!(
            SIMPLE_AD_NETWORK_TYPES.contains(network_type),
            *admitted,
            "case {network_type:?}: the documented catalog and the accept column disagree"
        );
        if *admitted {
            let id = create_directory(&router, body, network_type).await;
            let described = describe_one(&router, &id, network_type).await;
            assert_eq!(
                described["NetworkType"],
                json!(network_type),
                "case {network_type:?}: expected the stored network type, got {described}"
            );
        } else {
            refused(
                &router,
                "CreateDirectory",
                body,
                "InvalidParameterException",
                network_type,
            )
            .await;
        }
    }

    // Exactly the two admitted rows were stored; the refused one left nothing behind.
    let stored = describe_all(&router, "network type catalog").await;
    assert_eq!(
        stored.len(),
        2,
        "expected one directory per admitted network type, got {stored:?}"
    );
}

#[tokio::test]
async fn tags_are_merged_into_the_stored_map_and_reported_from_it() {
    let router = service();

    // The rows differ along tag count, whether the added keys overlap the stored ones,
    // and whether the directory started out with tags at all.
    let cases: Vec<(&str, Value, Value, BTreeMap<&str, &str>)> = vec![
        (
            "no tags, then one",
            json!([]),
            json!([{ "Key": "env", "Value": "dev" }]),
            BTreeMap::from([("env", "dev")]),
        ),
        (
            "disjoint keys merge",
            json!([{ "Key": "env", "Value": "dev" }]),
            json!([{ "Key": "owner", "Value": "team-a" }]),
            BTreeMap::from([("env", "dev"), ("owner", "team-a")]),
        ),
        (
            "same key overwrites",
            json!([{ "Key": "env", "Value": "dev" }, { "Key": "owner", "Value": "team-a" }]),
            json!([{ "Key": "env", "Value": "prod" }]),
            BTreeMap::from([("env", "prod"), ("owner", "team-a")]),
        ),
        (
            "several at once",
            json!([{ "Key": "a", "Value": "1" }]),
            json!([
                { "Key": "b", "Value": "2" },
                { "Key": "c", "Value": "3" },
                { "Key": "a", "Value": "9" }
            ]),
            BTreeMap::from([("a", "9"), ("b", "2"), ("c", "3")]),
        ),
    ];

    for (index, (case, initial, added, expected)) in cases.iter().enumerate() {
        let id = create_directory(
            &router,
            json!({
                "Name": format!("tagged{index}.example.com"),
                "Password": PASSWORD,
                "Size": "Small",
                "Tags": initial
            }),
            case,
        )
        .await;

        let empty = ok(
            &router,
            "AddTagsToResource",
            json!({ "ResourceId": &id, "Tags": added }),
            case,
        )
        .await;
        assert_eq!(
            empty,
            json!({}),
            "case {case:?}: expected AddTagsToResource to answer an empty body, got {empty}"
        );

        let listed = ok(
            &router,
            "ListTagsForResource",
            json!({ "ResourceId": &id }),
            case,
        )
        .await;
        let expected: BTreeMap<String, String> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            tag_map(&listed, case),
            expected,
            "case {case:?}: expected the merged tag map, got {listed}"
        );
    }

    refused(
        &router,
        "AddTagsToResource",
        json!({ "ResourceId": "d-00000000ff", "Tags": [{ "Key": "env", "Value": "dev" }] }),
        "EntityDoesNotExistException",
        "tags for an unknown directory",
    )
    .await;
    refused(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": "d-00000000ff" }),
        "EntityDoesNotExistException",
        "list tags of an unknown directory",
    )
    .await;
}

#[tokio::test]
async fn describe_directories_pages_over_the_stored_records() {
    let router = service();

    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        created.push(
            create_directory(
                &router,
                minimal_create(&format!("page{index}.example.com")),
                "paging",
            )
            .await,
        );
    }

    // Page sizes spanning one at a time, several at a time, exactly the record count,
    // past it, and the "limitations of the operation" the member's docs name for an
    // absent or zero `Limit`.
    for limit in [Some(1), Some(2), Some(3), Some(5), Some(7), Some(0), None] {
        let case = format!("Limit={limit:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(limit) = limit {
                request["Limit"] = json!(limit);
            }
            if let Some(token) = &next {
                request["NextToken"] = json!(token);
            }
            let answer = ok(&router, "DescribeDirectories", request, &case).await;
            let page = descriptions(&answer, &case);
            if let Some(limit) = limit.filter(|value| *value > 0) {
                assert!(
                    page.len() <= usize::try_from(limit).unwrap_or(usize::MAX),
                    "case {case:?}: expected at most {limit} descriptions on a page, got {}",
                    page.len()
                );
            }
            for description in &page {
                seen.push(string(description, "DirectoryId", &case));
            }
            next = answer["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, created,
            "case {case:?}: expected the pages to cover every stored directory once, in creation order"
        );
    }

    // A token this service did not issue is refused rather than served as a first
    // page — including a token another paginated operation issued.
    let tags_token = {
        let id = &created[0];
        ok(
            &router,
            "AddTagsToResource",
            json!({ "ResourceId": id, "Tags": [{ "Key": "a", "Value": "1" }, { "Key": "b", "Value": "2" }] }),
            "foreign token",
        )
        .await;
        let page = ok(
            &router,
            "ListTagsForResource",
            json!({ "ResourceId": id, "Limit": 1 }),
            "foreign token",
        )
        .await;
        string(&page, "NextToken", "foreign token")
    };
    for (case, token) in [
        ("not hex", "not-a-token".to_string()),
        ("hex but not a cursor", "6162636465".to_string()),
        ("another listing's cursor", tags_token.clone()),
        // Longer than any token this service issues, so it is refused before being
        // decoded at all.
        ("absurdly long", "61".repeat(2_000)),
    ] {
        refused(
            &router,
            "DescribeDirectories",
            json!({ "NextToken": token }),
            "InvalidNextTokenException",
            case,
        )
        .await;
    }
    refused(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": &created[0], "NextToken": "not-a-token" }),
        "InvalidNextTokenException",
        "bad tag cursor",
    )
    .await;

    // A token resumes the listing it was issued for: the `DirectoryIds` filter is part
    // of the cursor, so replaying a token under a different filter is refused rather
    // than serving a page of another listing.
    let filtered = created[..3].to_vec();
    let unfiltered_token = {
        let page = ok(
            &router,
            "DescribeDirectories",
            json!({ "Limit": 1 }),
            "cursor filter",
        )
        .await;
        string(&page, "NextToken", "cursor filter")
    };
    let filtered_token = {
        let page = ok(
            &router,
            "DescribeDirectories",
            json!({ "Limit": 1, "DirectoryIds": &filtered }),
            "cursor filter",
        )
        .await;
        string(&page, "NextToken", "cursor filter")
    };
    for (case, token, request_filter) in [
        (
            "unfiltered cursor replayed with a filter",
            &unfiltered_token,
            Some(filtered.clone()),
        ),
        ("filtered cursor replayed unfiltered", &filtered_token, None),
        (
            "filtered cursor replayed with another filter",
            &filtered_token,
            Some(created[3..].to_vec()),
        ),
    ] {
        let mut request = json!({ "NextToken": token, "Limit": 1 });
        if let Some(ids) = request_filter {
            request["DirectoryIds"] = json!(ids);
        }
        refused(
            &router,
            "DescribeDirectories",
            request,
            "InvalidNextTokenException",
            case,
        )
        .await;
    }
    // Replayed with the filter it was issued under, the same token pages on.
    let resumed = ok(
        &router,
        "DescribeDirectories",
        json!({ "NextToken": &filtered_token, "Limit": 1, "DirectoryIds": &filtered }),
        "cursor filter",
    )
    .await;
    let page = descriptions(&resumed, "cursor filter");
    assert_eq!(
        page.len(),
        1,
        "case \"cursor filter\": expected the second page of the filtered listing, got {page:?}"
    );
    assert_eq!(
        page[0]["DirectoryId"],
        json!(&filtered[1]),
        "case \"cursor filter\": expected the filtered listing to resume at its second record, got {page:?}"
    );
}

#[tokio::test]
async fn list_tags_for_resource_pages_over_the_stored_tags() {
    let router = service();
    let case = "tag paging";
    let id = create_directory(&router, minimal_create("tags.example.com"), case).await;
    let keys = ["alpha", "beta", "gamma", "delta"];
    ok(
        &router,
        "AddTagsToResource",
        json!({
            "ResourceId": &id,
            "Tags": keys.iter().map(|key| json!({ "Key": key, "Value": key })).collect::<Vec<_>>()
        }),
        case,
    )
    .await;

    let mut expected: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();
    expected.sort();

    for limit in [Some(1), Some(2), Some(4), Some(9), None] {
        let case = format!("{case} Limit={limit:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut request = json!({ "ResourceId": &id });
            if let Some(limit) = limit {
                request["Limit"] = json!(limit);
            }
            if let Some(token) = &next {
                request["NextToken"] = json!(token);
            }
            let answer = ok(&router, "ListTagsForResource", request, &case).await;
            seen.extend(tag_map(&answer, &case).into_keys());
            next = answer["NextToken"].as_str().map(str::to_string);
            if next.is_none() {
                break;
            }
            assert!(
                seen.len() <= keys.len(),
                "case {case:?}: pagination did not terminate, saw {seen:?}"
            );
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: expected the pages to cover every stored tag once"
        );
    }
}

#[tokio::test]
async fn describe_directories_narrows_to_the_requested_ids() {
    let router = service();
    let mut created: Vec<String> = Vec::new();
    for index in 0..3 {
        created.push(
            create_directory(
                &router,
                minimal_create(&format!("filter{index}.example.com")),
                "filtering",
            )
            .await,
        );
    }

    // The filters differ along count and the order the ids are asked for in.
    let filters: Vec<(&str, Vec<String>)> = vec![
        ("one id", vec![created[1].clone()]),
        (
            "two ids, reversed",
            vec![created[2].clone(), created[0].clone()],
        ),
        ("every id", created.clone()),
        (
            "a repeated id",
            vec![created[0].clone(), created[0].clone()],
        ),
    ];
    for (case, requested) in &filters {
        let answer = ok(
            &router,
            "DescribeDirectories",
            json!({ "DirectoryIds": requested }),
            case,
        )
        .await;
        let found: BTreeSet<String> = descriptions(&answer, case)
            .iter()
            .map(|description| string(description, "DirectoryId", case))
            .collect();
        let expected: BTreeSet<String> = requested.iter().cloned().collect();
        assert_eq!(
            found, expected,
            "case {case:?}: expected the filter to narrow the result to the ids asked for"
        );
    }

    // "An empty list results in an InvalidParameterException being thrown."
    refused(
        &router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [] }),
        "InvalidParameterException",
        "empty DirectoryIds",
    )
    .await;
    // An id naming no stored directory is reported, not silently dropped.
    refused(
        &router,
        "DescribeDirectories",
        json!({ "DirectoryIds": ["d-00000000ff"] }),
        "EntityDoesNotExistException",
        "unknown id",
    )
    .await;
    refused(
        &router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [&created[0], "d-00000000ff"] }),
        "EntityDoesNotExistException",
        "one known and one unknown id",
    )
    .await;
    // A `DirectoryIds` list longer than this emulator will resolve is refused; the
    // guard mirrors `state::MAX_DIRECTORY_IDS_PER_REQUEST` and sits well above any
    // list a caller has reason to send.
    let too_many: Vec<String> = (0..=100)
        .map(|index| format!("d-{index:010x}"))
        .collect();
    refused(
        &router,
        "DescribeDirectories",
        json!({ "DirectoryIds": too_many }),
        "InvalidParameterException",
        "one past the DirectoryIds bound",
    )
    .await;
    // A list that merely repeats more ids than the Region holds is legal and is
    // answered, not refused.
    let repeated: Vec<String> = (0..20).map(|_| created[0].clone()).collect();
    let answer = ok(
        &router,
        "DescribeDirectories",
        json!({ "DirectoryIds": repeated }),
        "a long but legal repeated filter",
    )
    .await;
    assert_eq!(
        descriptions(&answer, "a long but legal repeated filter").len(),
        1,
        "expected a repeated filter to report the one directory it names, got {answer}"
    );
}

#[tokio::test]
async fn a_deleted_directory_is_gone_and_its_alias_stays_reserved() {
    let router = service();
    let case = "delete";
    let doomed = create_directory(&router, minimal_create("doomed.example.com"), case).await;
    let survivor = create_directory(&router, minimal_create("keeper.example.com"), case).await;
    ok(
        &router,
        "AddTagsToResource",
        json!({ "ResourceId": &doomed, "Tags": [{ "Key": "env", "Value": "dev" }] }),
        case,
    )
    .await;
    // An alias is assigned only to a directory that has reached its serving stage.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "CreateAlias",
        json!({ "DirectoryId": &doomed, "Alias": "shared-alias" }),
        case,
    )
    .await;

    let deleted = ok(
        &router,
        "DeleteDirectory",
        json!({ "DirectoryId": &doomed }),
        case,
    )
    .await;
    assert_eq!(
        deleted["DirectoryId"],
        json!(&doomed),
        "case {case:?}: expected DeleteDirectory to report the deleted id, got {deleted}"
    );

    // Every reader of the deleted id now reports it does not exist, and a second
    // delete does too — with ONE exception, the reporting call whose `DirectoryIds` is a
    // FILTER: an id this account HAD and deleted narrows `DescribeDirectories` to nothing
    // and is answered 200 with an empty page, which is what a client polling for its own
    // deletion reads it off (`crate::refusal_classification_rules` is the rule's own
    // suite). Every MUTATION of the id still refuses.
    for (operation, body) in [
        ("ListTagsForResource", json!({ "ResourceId": &doomed })),
        ("DeleteDirectory", json!({ "DirectoryId": &doomed })),
        (
            "AddTagsToResource",
            json!({ "ResourceId": &doomed, "Tags": [{ "Key": "env", "Value": "dev" }] }),
        ),
        (
            "CreateAlias",
            json!({ "DirectoryId": &doomed, "Alias": "revived" }),
        ),
    ] {
        refused(
            &router,
            operation,
            body,
            "EntityDoesNotExistException",
            &format!("{operation} after delete"),
        )
        .await;
    }
    let polled = ok(
        &router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [&doomed] }),
        "DescribeDirectories after delete",
    )
    .await;
    assert_eq!(
        polled["DirectoryDescriptions"],
        json!([]),
        "case {case:?}: expected the deleted id to narrow the page to nothing, got {polled}"
    );

    // The surviving directory is untouched, and the alias the deleted directory held
    // stays reserved: "After an alias has been created, it cannot be deleted or
    // reused."
    let remaining = describe_all(&router, case).await;
    let ids: Vec<String> = remaining
        .iter()
        .map(|description| string(description, "DirectoryId", case))
        .collect();
    assert_eq!(
        ids,
        vec![survivor.clone()],
        "case {case:?}: expected only the surviving directory to remain"
    );
    refused(
        &router,
        "CreateAlias",
        json!({ "DirectoryId": &survivor, "Alias": "shared-alias" }),
        "EntityAlreadyExistsException",
        "reusing a deleted directory's alias",
    )
    .await;
    let described = describe_one(&router, &survivor, case).await;
    assert_eq!(
        described["Alias"],
        json!(&survivor),
        "case {case:?}: expected the refused reuse to leave the survivor's alias at its default, got {described}"
    );
    // A different alias is still assignable.
    ok(
        &router,
        "CreateAlias",
        json!({ "DirectoryId": &survivor, "Alias": "fresh-alias" }),
        case,
    )
    .await;
    let renamed = describe_one(&router, &survivor, case).await;
    assert_eq!(
        renamed["AccessUrl"],
        json!("fresh-alias.awsapps.com"),
        "case {case:?}: expected the assigned alias to drive the access URL, got {renamed}"
    );
}

#[tokio::test]
async fn the_alias_reservation_index_is_bounded() {
    let router = service();
    let case = "alias reservations";

    // Each pass reserves one alias and then deletes the directory, so no directory
    // quota is ever reached but the reservation index grows by one — the only store
    // in this slice a caller can grow without deleting anything. The guard mirrors
    // `state::MAX_RESERVED_ALIASES`, which is stated as its own figure rather than
    // derived from a directory quota.
    //
    // The passes run in BATCHES of the Region's own Simple AD quota: an alias is assigned
    // only to a directory that has reached its serving stage, so each batch creates its
    // directories, waits ONCE for the transition their creates owe, and then reserves and
    // deletes — which is the same sequence per pass with one wait per batch instead of one
    // per directory.
    const ALIAS_RESERVATION_GUARD: usize = 100;
    const BATCH: usize = CLOUD_ONLY_QUOTA;
    for batch in 0..ALIAS_RESERVATION_GUARD / BATCH {
        let mut created: Vec<(usize, String)> = Vec::with_capacity(BATCH);
        for offset in 0..BATCH {
            let index = batch * BATCH + offset;
            created.push((
                index,
                create_directory(
                    &router,
                    minimal_create(&format!("res{index}.example.com")),
                    case,
                )
                .await,
            ));
        }
        tokio::time::sleep(PAST_ONE_SPAN).await;
        for (index, id) in &created {
            ok(
                &router,
                "CreateAlias",
                json!({ "DirectoryId": id, "Alias": format!("reserved{index}") }),
                case,
            )
            .await;
            ok(&router, "DeleteDirectory", json!({ "DirectoryId": id }), case).await;
        }
    }

    let id = create_directory(&router, minimal_create("last.example.com"), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    refused(
        &router,
        "CreateAlias",
        json!({ "DirectoryId": &id, "Alias": "one-too-many" }),
        "ClientException",
        "one past the alias reservation guard",
    )
    .await;
    // The refused call stored nothing: the directory still reports its default alias.
    let described = describe_one(&router, &id, case).await;
    assert_eq!(
        described["Alias"],
        json!(&id),
        "case {case:?}: expected the refused CreateAlias to store nothing, got {described}"
    );
}

#[tokio::test]
async fn the_directory_and_tag_quotas_are_enforced() {
    let router = service();

    for index in 0..CLOUD_ONLY_QUOTA {
        create_directory(
            &router,
            minimal_create(&format!("quota{index}.example.com")),
            "directory quota",
        )
        .await;
    }
    refused(
        &router,
        "CreateDirectory",
        minimal_create("one-too-many.example.com"),
        "DirectoryLimitExceededException",
        "one past the directory quota",
    )
    .await;

    // The tag quota, on a directory already carrying the whole allowance.
    let full: Vec<Value> = (0..TAG_QUOTA)
        .map(|index| json!({ "Key": format!("key{index}"), "Value": "v" }))
        .collect();
    let stored = describe_all(&router, "tag quota").await;
    let id = string(&stored[0], "DirectoryId", "tag quota");
    ok(
        &router,
        "AddTagsToResource",
        json!({ "ResourceId": &id, "Tags": full }),
        "tag quota",
    )
    .await;
    // A key already present is an overwrite, so it stays inside the quota...
    ok(
        &router,
        "AddTagsToResource",
        json!({ "ResourceId": &id, "Tags": [{ "Key": "key0", "Value": "w" }] }),
        "tag quota overwrite",
    )
    .await;
    // ...while a new key would take the directory past it.
    refused(
        &router,
        "AddTagsToResource",
        json!({ "ResourceId": &id, "Tags": [{ "Key": "one-too-many", "Value": "v" }] }),
        "TagLimitExceededException",
        "one past the tag quota",
    )
    .await;
    // A single request carrying more than the whole allowance is refused on the
    // request's own size, before it is merged with anything.
    let over_quota: Vec<Value> = (0..=TAG_QUOTA)
        .map(|index| json!({ "Key": format!("bulk{index}"), "Value": "v" }))
        .collect();
    refused(
        &router,
        "AddTagsToResource",
        json!({ "ResourceId": &id, "Tags": over_quota }),
        "TagLimitExceededException",
        "a request past the tag quota on its own",
    )
    .await;
    let listed = ok(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": &id, "Limit": TAG_QUOTA }),
        "tag quota",
    )
    .await;
    let tags = tag_map(&listed, "tag quota");
    assert_eq!(
        tags.len(),
        TAG_QUOTA,
        "expected the refused merge to leave the stored tags at the quota, got {}",
        tags.len()
    );
    assert_eq!(
        tags.get("key0").map(String::as_str),
        Some("w"),
        "expected the overwrite to have taken effect, got {tags:?}"
    );
}
