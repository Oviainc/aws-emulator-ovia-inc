// SPDX-License-Identifier: CC0-1.0

//! `ListLocations`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: FmrsService.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router. Nothing reaches into the state struct, so a response that is
//! not computed from persisted state cannot pass.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for. It is deliberately NOT the
/// emulator's default, so an ARN that carries it proves the region came from the
/// request's SigV4 credential scope.
const SIGNED_REGION: &str = "us-west-2";

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("FmrsService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDDATASYNCTEST/20260101/{SIGNED_REGION}\
                 /datasync/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
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
        serde_json::json!({})
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|_| {
            serde_json::json!({ "__unparsed": String::from_utf8_lossy(&payload) })
        })
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    case: &str,
) -> serde_json::Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 `InvalidRequestException` whose message
/// is `expected_message`.
async fn invalid_request(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    expected_message: &str,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    assert!(
        parsed["__type"]
            .as_str()
            .unwrap_or_default()
            .contains("InvalidRequestException"),
        "case {case:?}: expected an InvalidRequestException from {operation}, got {parsed}"
    );
    assert_eq!(
        parsed["message"].as_str().unwrap_or_default(),
        expected_message,
        "case {case:?}: wrong refusal message, got {parsed}"
    );
}

/// A required string member of a response body.
fn string(body: &serde_json::Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// One location this suite creates: the `CreateLocation*` that mints it, the matching
/// `DescribeLocation*` that reads it back, and the `LocationType` the modelled filter
/// of that name compares it with.
struct Fixture {
    case: &'static str,
    create: &'static str,
    describe: &'static str,
    request: serde_json::Value,
    expected_uri: &'static str,
    /// The kind of storage this location addresses, upper-cased: what the modelled
    /// `LocationType` filter compares (the filter's own documentation filters Amazon S3
    /// locations with `"Values": "S3"`).
    location_type: &'static str,
}

/// The locations every case in this suite lists.
///
/// The rows span the kinds of storage this service can create (Amazon S3, Azure Blob
/// Storage, Amazon EFS and an object storage system), with and without a subdirectory,
/// and include two Azure Blob Storage locations created from IDENTICAL configuration —
/// independent resources that share a `LocationUri` and must both be listed under their
/// own ARNs.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            case: "an S3 bucket with a prefix",
            create: "CreateLocationS3",
            describe: "DescribeLocationS3",
            request: serde_json::json!({
                "S3BucketArn": "arn:aws:s3:::alpha-bucket",
                "S3Config": { "BucketAccessRoleArn": "arn:aws:iam::000000000000:role/datasync" },
                "Subdirectory": "/in",
            }),
            expected_uri: "s3://alpha-bucket/in/",
            location_type: "S3",
        },
        Fixture {
            case: "an Azure Blob Storage container",
            create: "CreateLocationAzureBlob",
            describe: "DescribeLocationAzureBlob",
            request: serde_json::json!({
                "ContainerUrl": "https://listing.blob.core.windows.net/shared-container",
                "AuthenticationType": "NONE",
            }),
            expected_uri: "azure-blob://listing.blob.core.windows.net/shared-container/",
            location_type: "AZURE_BLOB",
        },
        Fixture {
            case: "the same Azure Blob Storage configuration again",
            create: "CreateLocationAzureBlob",
            describe: "DescribeLocationAzureBlob",
            request: serde_json::json!({
                "ContainerUrl": "https://listing.blob.core.windows.net/shared-container",
                "AuthenticationType": "NONE",
            }),
            expected_uri: "azure-blob://listing.blob.core.windows.net/shared-container/",
            location_type: "AZURE_BLOB",
        },
        Fixture {
            case: "an Amazon EFS file system root",
            create: "CreateLocationEfs",
            describe: "DescribeLocationEfs",
            request: serde_json::json!({
                "EfsFilesystemArn":
                    "arn:aws:elasticfilesystem:us-east-2:000000000000:file-system/fs-08e04cd4",
                "Ec2Config": {
                    "SubnetArn":
                        "arn:aws:ec2:us-east-2:000000000000:subnet/subnet-1234567890abcdef1",
                    "SecurityGroupArns": [
                        "arn:aws:ec2:us-east-2:000000000000:security-group/sg-1234567890abcdef2"
                    ],
                },
            }),
            expected_uri: "efs://us-east-2.fs-08e04cd4/",
            location_type: "EFS",
        },
        Fixture {
            case: "an object storage bucket",
            create: "CreateLocationObjectStorage",
            describe: "DescribeLocationObjectStorage",
            request: serde_json::json!({
                "ServerHostname": "storage.example.com",
                "BucketName": "objects",
            }),
            expected_uri: "object-storage://storage.example.com/objects/",
            location_type: "OBJECT_STORAGE",
        },
        Fixture {
            case: "an S3 bucket root",
            create: "CreateLocationS3",
            describe: "DescribeLocationS3",
            request: serde_json::json!({
                "S3BucketArn": "arn:aws:s3:::beta-bucket",
                "S3Config": { "BucketAccessRoleArn": "arn:aws:iam::000000000000:role/datasync" },
            }),
            expected_uri: "s3://beta-bucket/",
            location_type: "S3",
        },
    ]
}

/// Create every fixture, in order, returning the ARN each one was minted with.
async fn create_fixtures(router: &MultiServiceRouter, fixtures: &[Fixture]) -> Vec<String> {
    let mut arns = Vec::with_capacity(fixtures.len());
    for fixture in fixtures {
        let created = ok(router, fixture.create, fixture.request.clone(), fixture.case).await;
        let arn = string(&created, "LocationArn", fixture.case);
        assert!(
            !arns.contains(&arn),
            "case {:?}: every create mints a fresh ARN, but {arn:?} was minted before",
            fixture.case
        );
        arns.push(arn);
    }
    arns
}

/// The `LocationArn`s one `ListLocations` response carries, in the order it listed them.
fn listed_arns(page: &serde_json::Value) -> Vec<String> {
    page["Locations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["LocationArn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every location on one unpaginated `ListLocations` call with `filters` applied.
async fn list_with(
    router: &MultiServiceRouter,
    filters: serde_json::Value,
    case: &str,
) -> Vec<String> {
    let page = ok(
        router,
        "ListLocations",
        serde_json::json!({ "Filters": filters }),
        case,
    )
    .await;
    assert!(
        page["NextToken"].is_null(),
        "case {case:?}: a page holding every match must carry no NextToken, got {page}"
    );
    listed_arns(&page)
}

/// `ListLocations` answers with the locations the service holds — an empty list before
/// any create, then every created location, each entry carrying the `LocationUri` its
/// own `DescribeLocation*` reports.
#[tokio::test]
async fn list_locations_reports_every_stored_location() {
    let router = service();
    let fixtures = fixtures();

    let empty = ok(
        &router,
        "ListLocations",
        serde_json::json!({}),
        "before any create",
    )
    .await;
    assert_eq!(
        listed_arns(&empty),
        Vec::<String>::new(),
        "case \"before any create\": expected no locations, got {empty}"
    );
    assert!(
        empty["NextToken"].is_null(),
        "case \"before any create\": expected no NextToken, got {empty}"
    );

    let arns = create_fixtures(&router, &fixtures).await;

    let listed = ok(
        &router,
        "ListLocations",
        serde_json::json!({}),
        "every location",
    )
    .await;
    assert_eq!(
        listed_arns(&listed),
        arns,
        "case \"every location\": expected every created location in creation order, got {listed}"
    );

    let entries = listed["Locations"].as_array().cloned().unwrap_or_default();
    for (index, fixture) in fixtures.iter().enumerate() {
        let entry = &entries[index];
        assert_eq!(
            entry["LocationUri"].as_str(),
            Some(fixture.expected_uri),
            "case {:?}: wrong listed LocationUri, got {entry}",
            fixture.case
        );
        // The same value, read back through this kind's own reader: the listing reports
        // the stored location and nothing assembled for the list.
        let described = ok(
            &router,
            fixture.describe,
            serde_json::json!({ "LocationArn": arns[index] }),
            fixture.case,
        )
        .await;
        assert_eq!(
            entry["LocationUri"].as_str(),
            described["LocationUri"].as_str(),
            "case {:?}: the listed LocationUri must be the one {} reports, got {entry} and {described}",
            fixture.case,
            fixture.describe
        );
    }

    // A deleted location leaves the listing.
    ok(
        &router,
        "DeleteLocation",
        serde_json::json!({ "LocationArn": arns[2] }),
        "after a delete",
    )
    .await;
    let after_delete = ok(
        &router,
        "ListLocations",
        serde_json::json!({}),
        "after a delete",
    )
    .await;
    let survivors: Vec<String> = arns
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 2)
        .map(|(_, arn)| arn.clone())
        .collect();
    assert_eq!(
        listed_arns(&after_delete),
        survivors,
        "case \"after a delete\": expected the surviving locations, got {after_delete}"
    );
}

/// `ListLocations` pages over every location in creation order, resuming exactly where
/// the previous page ended and omitting the token on the last page.
///
/// The rows are every page size from 1 to one past the number of locations, which
/// exhausts the sizes that can split them, plus the modelled maximum.
#[tokio::test]
async fn list_locations_pages_in_creation_order_without_gaps_or_repeats() {
    let router = service();
    let fixtures = fixtures();
    let created = create_fixtures(&router, &fixtures).await;

    for page_size in [1usize, 2, 3, 4, 5, 6, 7, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = serde_json::json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = serde_json::json!(token);
            }
            let page = ok(&router, "ListLocations", request, &case).await;
            let arns = listed_arns(&page);
            assert!(
                arns.len() <= page_size,
                "case {case:?}: a page carried {} entries, more than MaxResults",
                arns.len()
            );
            for arn in arns {
                assert!(
                    !seen.contains(&arn),
                    "case {case:?}: {arn:?} appeared on two pages"
                );
                seen.push(arn);
            }
            token = page["NextToken"].as_str().map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= created.len() + 2,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, created,
            "case {case:?}: paging must yield every location in creation order"
        );
        let expected_pages = created.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // A cursor stays valid across a delete: the page after it resumes at the next
    // surviving location rather than repeating or skipping one.
    let first_page = ok(
        &router,
        "ListLocations",
        serde_json::json!({ "MaxResults": 2 }),
        "delete between pages",
    )
    .await;
    let token = string(&first_page, "NextToken", "delete between pages");
    ok(
        &router,
        "DeleteLocation",
        serde_json::json!({ "LocationArn": created[3] }),
        "delete between pages",
    )
    .await;
    let second_page = ok(
        &router,
        "ListLocations",
        serde_json::json!({ "MaxResults": 10, "NextToken": token }),
        "delete between pages",
    )
    .await;
    assert_eq!(
        listed_arns(&second_page),
        vec![
            created[2].clone(),
            created[4].clone(),
            created[5].clone()
        ],
        "case \"delete between pages\": expected the surviving tail, got {second_page}"
    );

    // A token this service never issued is refused rather than silently restarting.
    for bad in ["not-a-token", "ZZZZ", "00"] {
        invalid_request(
            &router,
            "ListLocations",
            serde_json::json!({ "NextToken": bad }),
            "NextToken is not a token this service issued.",
            bad,
        )
        .await;
    }
}

/// `ListLocations` applies the filters the model declares: `LocationUri` and
/// `LocationType` as text, and `CreationTime` as an instant.
///
/// The rows span every `LocationFilterName` the model admits (`LocationUri`,
/// `LocationType`, `CreationTime`), the operators each one can be compared with
/// including the negated and the multi-valued ones, the separator spellings of a
/// multi-word location type, and a request carrying two filters at once.
#[tokio::test]
async fn list_locations_applies_modelled_filters() {
    let router = service();
    let fixtures = fixtures();
    let before_any = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs()
        - 60;
    let arns = create_fixtures(&router, &fixtures).await;
    let all: Vec<usize> = (0..fixtures.len()).collect();

    // Each row: the case, the `Filters` the request carries, and the fixture indices
    // the service must answer with, in creation order.
    let rows: Vec<(&str, serde_json::Value, Vec<usize>)> = vec![
        (
            "LocationType Equals one kind",
            serde_json::json!([{ "Name": "LocationType", "Values": ["S3"], "Operator": "Equals" }]),
            vec![0, 5],
        ),
        (
            "LocationType Equals a multi-word kind",
            serde_json::json!([
                { "Name": "LocationType", "Values": ["AZURE_BLOB"], "Operator": "Equals" }
            ]),
            vec![1, 2],
        ),
        (
            "LocationType Equals the same kind spelled with a hyphen",
            serde_json::json!([
                { "Name": "LocationType", "Values": ["azure-blob"], "Operator": "Equals" }
            ]),
            vec![1, 2],
        ),
        (
            "LocationType Equals the same kind spelled as one word",
            serde_json::json!([
                { "Name": "LocationType", "Values": ["AzureBlob"], "Operator": "Equals" }
            ]),
            vec![1, 2],
        ),
        (
            "LocationType In two kinds",
            serde_json::json!([
                {
                    "Name": "LocationType",
                    "Values": ["EFS", "OBJECT_STORAGE"],
                    "Operator": "In",
                }
            ]),
            vec![3, 4],
        ),
        (
            "LocationType NotEquals one kind",
            serde_json::json!([
                { "Name": "LocationType", "Values": ["S3"], "Operator": "NotEquals" }
            ]),
            vec![1, 2, 3, 4],
        ),
        (
            "LocationType Equals a kind nothing holds",
            serde_json::json!([
                { "Name": "LocationType", "Values": ["HDFS"], "Operator": "Equals" }
            ]),
            vec![],
        ),
        (
            "LocationUri Equals a URI two independent locations share",
            serde_json::json!([
                {
                    "Name": "LocationUri",
                    "Values": ["azure-blob://listing.blob.core.windows.net/shared-container/"],
                    "Operator": "Equals",
                }
            ]),
            vec![1, 2],
        ),
        (
            "LocationUri BeginsWith a scheme",
            serde_json::json!([
                { "Name": "LocationUri", "Values": ["s3://"], "Operator": "BeginsWith" }
            ]),
            vec![0, 5],
        ),
        (
            "LocationUri Contains a bucket name",
            serde_json::json!([
                { "Name": "LocationUri", "Values": ["beta-bucket"], "Operator": "Contains" }
            ]),
            vec![5],
        ),
        (
            "LocationUri NotContains a bucket name",
            serde_json::json!([
                { "Name": "LocationUri", "Values": ["beta-bucket"], "Operator": "NotContains" }
            ]),
            vec![0, 1, 2, 3, 4],
        ),
        (
            "LocationUri In two URIs",
            serde_json::json!([
                {
                    "Name": "LocationUri",
                    "Values": ["efs://us-east-2.fs-08e04cd4/", "s3://alpha-bucket/in/"],
                    "Operator": "In",
                }
            ]),
            vec![0, 3],
        ),
        (
            "CreationTime GreaterThan an instant before every create",
            serde_json::json!([
                {
                    "Name": "CreationTime",
                    "Values": [before_any.to_string()],
                    "Operator": "GreaterThan",
                }
            ]),
            all.clone(),
        ),
        (
            "CreationTime LessThan an instant before every create",
            serde_json::json!([
                {
                    "Name": "CreationTime",
                    "Values": [before_any.to_string()],
                    "Operator": "LessThan",
                }
            ]),
            vec![],
        ),
        (
            "CreationTime GreaterThanOrEqual an RFC 3339 instant",
            serde_json::json!([
                {
                    "Name": "CreationTime",
                    "Values": ["2020-01-01T00:00:00Z"],
                    "Operator": "GreaterThanOrEqual",
                }
            ]),
            all.clone(),
        ),
        (
            "two filters at once, both of which must hold",
            serde_json::json!([
                { "Name": "LocationType", "Values": ["S3"], "Operator": "Equals" },
                { "Name": "LocationUri", "Values": ["s3://beta"], "Operator": "BeginsWith" },
            ]),
            vec![5],
        ),
        (
            "two filters at once that no location satisfies together",
            serde_json::json!([
                { "Name": "LocationType", "Values": ["EFS"], "Operator": "Equals" },
                { "Name": "LocationUri", "Values": ["s3://"], "Operator": "BeginsWith" },
            ]),
            vec![],
        ),
    ];

    for (case, filters, expected) in rows {
        let listed = list_with(&router, filters, case).await;
        let expected: Vec<String> = expected.iter().map(|index| arns[*index].clone()).collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: wrong filtered locations, got {listed:?}"
        );
    }

    // Every kind the fixtures span, each filtered by its own `LocationType`: exactly
    // the locations addressing that kind of storage, so no kind is a special case.
    for fixture in &fixtures {
        let case = format!("LocationType Equals {}", fixture.location_type);
        let expected: Vec<String> = fixtures
            .iter()
            .enumerate()
            .filter(|(_, other)| other.location_type == fixture.location_type)
            .map(|(index, _)| arns[index].clone())
            .collect();
        let listed = list_with(
            &router,
            serde_json::json!([
                {
                    "Name": "LocationType",
                    "Values": [fixture.location_type],
                    "Operator": "Equals",
                }
            ]),
            &case,
        )
        .await;
        assert_eq!(
            listed, expected,
            "case {case:?}: wrong filtered locations, got {listed:?}"
        );
    }
}

/// A filter `ListLocations` cannot evaluate is refused, with the message naming what
/// could not be evaluated.
#[tokio::test]
async fn list_locations_refuses_filters_it_cannot_evaluate() {
    let router = service();
    let fixtures = fixtures();
    let _ = create_fixtures(&router, &fixtures).await;

    for (case, filter, message) in [
        (
            "an operator that only orders numbers, on text",
            serde_json::json!(
                { "Name": "LocationUri", "Values": ["s3://alpha-bucket/in/"], "Operator": "GreaterThan" }
            ),
            "Operator GreaterThan cannot be used with filter name LocationUri.",
        ),
        (
            "an operator that only compares text, on an instant",
            serde_json::json!(
                { "Name": "CreationTime", "Values": ["1700000000"], "Operator": "BeginsWith" }
            ),
            "Operator BeginsWith cannot be used with filter name CreationTime.",
        ),
        (
            "a CreationTime value that is not an instant",
            serde_json::json!(
                { "Name": "CreationTime", "Values": ["yesterday"], "Operator": "GreaterThan" }
            ),
            "Filter CreationTime value yesterday is not a timestamp.",
        ),
    ] {
        invalid_request(
            &router,
            "ListLocations",
            serde_json::json!({ "Filters": [filter] }),
            message,
            case,
        )
        .await;
    }

    // A filter name outside the modelled `LocationFilterName` enum (`LocationUri`,
    // `LocationType`, `CreationTime`) is refused by that constraint, so no request
    // carrying one reaches the filtering above.
    let case = "a filter name the model does not admit";
    let (status, parsed) = call(
        &router,
        "ListLocations",
        serde_json::json!({
            "Filters": [
                { "Name": "LocationId", "Values": ["s3://alpha-bucket/in/"], "Operator": "Equals" }
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected ListLocations to answer 400, got {status} with {parsed}"
    );
    assert!(
        parsed["message"]
            .as_str()
            .unwrap_or_default()
            .contains("LocationId"),
        "case {case:?}: expected the refusal to name the filter, got {parsed}"
    );
}
