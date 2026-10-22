// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the repository-association -> code-review lifecycle.
//!
//! Every case drives the service through the SAME front door the emulator serves on: a
//! `MultiServiceRouter` with this one service mounted, fed real `restJson1` requests
//! (`POST /associations`, `GET /associations/{AssociationArn}`, `GET /associations`,
//! `POST /codereviews`, `GET /codereviews/{CodeReviewArn}`,
//! `DELETE /associations/{AssociationArn}`). Nothing reaches into the state struct: a
//! create is only believed once a read of the same resource agrees with it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codegurureviewer_awsgurufrontendservice::EmulatedService;

/// The account every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;
/// The region the test signs for, so the ARNs it expects name it.
const REGION: &str = "eu-west-1";

/// Longer than the service's own settle span, so a status the clock owes has come due.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// One service behind the real router: requests arrive as HTTP and are routed by their
/// SigV4 credential scope, exactly as a signed SDK call would be.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, path_and_query: &str, body: Option<Value>) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTCODEGURU/20260101/{REGION}/codeguru-reviewer/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://codeguru-reviewer.{REGION}.amazonaws.com{path_and_query}"
            ))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(
                body.map(|b| b.to_string()).unwrap_or_default(),
            ))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn associate(&self, body: Value) -> (u16, Value) {
        self.send("POST", "/associations", Some(body)).await
    }

    async fn describe_association(&self, arn: &str) -> (u16, Value) {
        self.send("GET", &format!("/associations/{}", path_segment(arn)), None)
            .await
    }

    async fn disassociate(&self, arn: &str) -> (u16, Value) {
        self.send(
            "DELETE",
            &format!("/associations/{}", path_segment(arn)),
            None,
        )
        .await
    }

    async fn list_associations(&self, query: &str) -> (u16, Value) {
        self.send("GET", &format!("/associations{query}"), None).await
    }

    async fn create_code_review(&self, body: Value) -> (u16, Value) {
        self.send("POST", "/codereviews", Some(body)).await
    }

    async fn describe_code_review(&self, arn: &str) -> (u16, Value) {
        self.send("GET", &format!("/codereviews/{}", path_segment(arn)), None)
            .await
    }
}

/// An ARN inside one URI path segment: `:` is percent-encoded the way an AWS SDK encodes a
/// non-greedy `@httpLabel`.
fn path_segment(arn: &str) -> String {
    arn.replace(':', "%3A")
}

/// `a.b.c` lookup, so an assertion can name the member it is about.
fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for key in path.split('.') {
        current = &current[key];
    }
    current
}

fn text(value: &Value, path: &str) -> String {
    at(value, path)
        .as_str()
        .unwrap_or_else(|| panic!("{path} is not a string in {value}"))
        .to_string()
}

/// The wire error code restJson1 puts in the error envelope's `__type`.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("no __type in error body {body}"))
        .to_string()
}

/// True for a lower-case v4 UUID — the form the model's `AssociationArn` pattern
/// (`…:association:[a-f0-9]{8}-[a-f0-9]{4}-…`) requires of an association id.
fn is_uuid_v4(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        && groups[2].starts_with('4')
}

fn association_arn(id: &str) -> String {
    format!("arn:aws:codeguru-reviewer:{REGION}:{ACCOUNT}:association:{id}")
}

/// The CodeStar Connections connection a third-party repository is reached through.
fn connection_arn() -> String {
    format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT}:connection/4a0b1c2d-3e4f-5a6b-7c8d-9e0f1a2b3c4d")
}

/// One row of the provider table: a `Repository` input and everything the association it
/// mints must then report.
struct ProviderCase {
    label: &'static str,
    repository: Value,
    provider_type: &'static str,
    name: &'static str,
    owner: String,
    connection_arn: Option<String>,
    bucket_name: Option<&'static str>,
}

/// Every `Repository` member the model declares, each paired with the `ProviderType` it
/// determines. The member names come from `smithy_query(shape="…#Repository")`; the provider
/// names are the `ProviderType` enum's own values.
fn provider_cases() -> Vec<ProviderCase> {
    vec![
        ProviderCase {
            label: "CodeCommit",
            repository: json!({"CodeCommit": {"Name": "ledger-service"}}),
            provider_type: "CodeCommit",
            name: "ledger-service",
            owner: ACCOUNT.to_string(),
            connection_arn: None,
            bucket_name: None,
        },
        ProviderCase {
            label: "Bitbucket",
            repository: json!({"Bitbucket": {
                "Name": "payments.api",
                "Owner": "finance-team",
                "ConnectionArn": connection_arn(),
            }}),
            provider_type: "Bitbucket",
            name: "payments.api",
            owner: "finance-team".to_string(),
            connection_arn: Some(connection_arn()),
            bucket_name: None,
        },
        ProviderCase {
            label: "GitHubEnterpriseServer",
            repository: json!({"GitHubEnterpriseServer": {
                "Name": "internal_tools",
                "Owner": "platform",
                "ConnectionArn": connection_arn(),
            }}),
            provider_type: "GitHubEnterpriseServer",
            name: "internal_tools",
            owner: "platform".to_string(),
            connection_arn: Some(connection_arn()),
            bucket_name: None,
        },
        ProviderCase {
            label: "S3Bucket",
            repository: json!({"S3Bucket": {
                "Name": "archive-repo",
                "BucketName": "codeguru-reviewer-archive",
            }}),
            provider_type: "S3Bucket",
            name: "archive-repo",
            owner: ACCOUNT.to_string(),
            connection_arn: None,
            bucket_name: Some("codeguru-reviewer-archive"),
        },
    ]
}

/// RULE: an association's `ProviderType` is the `Repository` member the request set, its
/// `Name`/`Owner` come from that member (the account id where the provider has no owner of
/// its own), and every field the create returns is what a later read returns.
///
/// The table spans all four `Repository` members; the recorded case (`CodeCommit`) is one
/// row.
#[tokio::test]
async fn an_association_reports_the_repository_the_request_named() {
    let emulator = Emulator::new();

    for case in provider_cases() {
        let label = case.label;
        let (status, created) = emulator
            .associate(json!({
                "Repository": case.repository,
                "Tags": {"Env": "dev", "Team": label},
            }))
            .await;
        assert_eq!(status, 200, "case {label}: expected 200, got {status} {created}");

        let association = &created["RepositoryAssociation"];
        let id = text(association, "AssociationId");
        assert!(
            is_uuid_v4(&id),
            "case {label}: AssociationId must be a v4 UUID, got {id:?}"
        );
        assert_eq!(
            text(association, "AssociationArn"),
            association_arn(&id),
            "case {label}: AssociationArn must name the association id and the signed region"
        );
        assert_eq!(
            text(association, "ProviderType"),
            case.provider_type,
            "case {label}: ProviderType must be the Repository member that was set, got {association}"
        );
        assert_eq!(
            text(association, "Name"),
            case.name,
            "case {label}: Name must come from the Repository member, got {association}"
        );
        assert_eq!(
            text(association, "Owner"),
            case.owner,
            "case {label}: Owner must come from the Repository member, got {association}"
        );
        assert_eq!(
            text(association, "State"),
            "Associating",
            "case {label}: a just-created association is still setting up, got {association}"
        );
        assert_eq!(
            at(association, "CreatedTimeStamp"),
            at(association, "LastUpdatedTimeStamp"),
            "case {label}: nothing has changed yet, so the two timestamps must agree"
        );
        assert!(
            !at(association, "CreatedTimeStamp").is_null(),
            "case {label}: CreatedTimeStamp must be set, got {association}"
        );
        match &case.connection_arn {
            Some(expected) => assert_eq!(
                text(association, "ConnectionArn"),
                *expected,
                "case {label}: a third-party repository reports its connection"
            ),
            None => assert!(
                at(association, "ConnectionArn").is_null(),
                "case {label}: only a third-party repository has a connection, got {association}"
            ),
        }
        match case.bucket_name {
            Some(expected) => assert_eq!(
                text(association, "S3RepositoryDetails.BucketName"),
                expected,
                "case {label}: an S3 repository reports its bucket"
            ),
            None => assert!(
                at(association, "S3RepositoryDetails").is_null(),
                "case {label}: only an S3 repository has bucket details, got {association}"
            ),
        }
        assert_eq!(
            created["Tags"],
            json!({"Env": "dev", "Team": label}),
            "case {label}: the created association reports the tags it stored"
        );

        // ROUND TRIP: the read must agree with the create, field for field.
        let arn = text(association, "AssociationArn");
        let (status, described) = emulator.describe_association(&arn).await;
        assert_eq!(
            status, 200,
            "case {label}: describing {arn} must succeed, got {status} {described}"
        );
        for member in [
            "AssociationId",
            "AssociationArn",
            "Name",
            "Owner",
            "ProviderType",
            "ConnectionArn",
            "CreatedTimeStamp",
            "S3RepositoryDetails",
        ] {
            assert_eq!(
                at(&described["RepositoryAssociation"], member),
                at(association, member),
                "case {label}: {member} read back must equal what the create stored"
            );
        }
        assert_eq!(
            described["Tags"], created["Tags"],
            "case {label}: the tags read back must equal the stored tags"
        );

        // And the listing projects the same stored resource.
        let (status, listed) = emulator
            .list_associations(&format!("?Name={}&MaxResults=10", case.name))
            .await;
        assert_eq!(
            status, 200,
            "case {label}: listing must succeed, got {status} {listed}"
        );
        let summaries = listed["RepositoryAssociationSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: no summaries in {listed}"));
        assert_eq!(
            summaries.len(),
            1,
            "case {label}: the Name filter must select exactly this association, got {listed}"
        );
        assert_eq!(
            text(&summaries[0], "AssociationArn"),
            arn,
            "case {label}: the summary must name the stored association"
        );
        assert_eq!(
            text(&summaries[0], "ProviderType"),
            case.provider_type,
            "case {label}: the summary must carry the stored provider type"
        );
    }
}

/// RULE: a repository with no association, and an association that was never minted, are
/// both absent — a read of an absent resource is the operation's own 404, not an empty
/// success.
#[tokio::test]
async fn reading_an_association_that_was_never_created_is_not_found() {
    let emulator = Emulator::new();

    // Well-formed ARNs (they satisfy the modelled pattern) that name nothing.
    let cases = [
        ("all zeroes", "00000000-0000-4000-8000-000000000000"),
        ("all f", "ffffffff-ffff-4fff-bfff-ffffffffffff"),
        ("mixed", "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"),
    ];
    for (label, id) in cases {
        let arn = association_arn(id);
        let (status, body) = emulator.describe_association(&arn).await;
        assert_eq!(
            status, 404,
            "case {label}: expected 404 for {arn}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label}: DescribeRepositoryAssociation models NotFoundException, got {body}"
        );

        let (status, body) = emulator.disassociate(&arn).await;
        assert_eq!(
            status, 404,
            "case {label}: disassociating {arn} must be 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label}: DisassociateRepository models NotFoundException, got {body}"
        );
    }

    let (status, body) = emulator.list_associations("").await;
    assert_eq!(status, 200, "listing an empty store must succeed, got {body}");
    assert_eq!(
        body["RepositoryAssociationSummaries"],
        json!([]),
        "an empty store lists no associations, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "a single page must not offer a continuation, got {body}"
    );
}

/// RULE: a retry carrying the ClientRequestToken of an earlier AssociateRepository yields
/// the SAME association; a fresh request for a repository that already has a live
/// association conflicts.
#[tokio::test]
async fn repeating_an_association_is_idempotent_by_token_and_conflicts_without_one() {
    let emulator = Emulator::new();

    for case in provider_cases() {
        let label = case.label;
        let token = format!("token-{label}");
        let request = json!({
            "Repository": case.repository,
            "ClientRequestToken": token,
            "KMSKeyDetails": {
                "EncryptionOption": "CUSTOMER_MANAGED_CMK",
                "KMSKeyId": "1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d",
            },
        });

        let (status, first) = emulator.associate(request.clone()).await;
        assert_eq!(status, 200, "case {label}: first associate, got {status} {first}");
        let first_id = text(&first["RepositoryAssociation"], "AssociationId");
        assert_eq!(
            text(&first["RepositoryAssociation"], "KMSKeyDetails.EncryptionOption"),
            "CUSTOMER_MANAGED_CMK",
            "case {label}: KMSKeyDetails must be stored as supplied, got {first}"
        );
        assert!(
            first["Tags"].is_null(),
            "case {label}: an association with no tags reports no Tags member, got {first}"
        );

        let (status, retried) = emulator.associate(request).await;
        assert_eq!(status, 200, "case {label}: retry, got {status} {retried}");
        assert_eq!(
            text(&retried["RepositoryAssociation"], "AssociationId"),
            first_id,
            "case {label}: the same ClientRequestToken must not mint a second association"
        );

        // A second request for the same repository WITHOUT that token conflicts with the
        // live association.
        let (status, conflict) = emulator
            .associate(json!({"Repository": case.repository}))
            .await;
        assert_eq!(
            status, 409,
            "case {label}: a live association must conflict, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {label}: AssociateRepository models ConflictException, got {conflict}"
        );
    }

    // Each of the four repositories is stored exactly once.
    let (status, listed) = emulator.list_associations("?MaxResults=100").await;
    assert_eq!(status, 200, "listing must succeed, got {listed}");
    assert_eq!(
        listed["RepositoryAssociationSummaries"]
            .as_array()
            .map(Vec::len),
        Some(4),
        "four repositories were associated, each exactly once, got {listed}"
    );
}

/// RULE: an idempotency token replays only the request it was first used for — the same
/// token naming a different resource is a conflict, never a success for the first one.
#[tokio::test]
async fn an_idempotency_token_does_not_replay_a_different_request() {
    let emulator = Emulator::new();
    let token = "shared-token";

    // The token is claimed by the first row; every later row is a different request under
    // the same token and must be refused rather than answered with the first row's resource.
    let mut rows = provider_cases();
    let first = rows.remove(0);
    let (status, created) = emulator
        .associate(json!({"Repository": first.repository, "ClientRequestToken": token}))
        .await;
    assert_eq!(status, 200, "claiming the token, got {status} {created}");
    let claimed = text(&created["RepositoryAssociation"], "AssociationId");

    for case in rows {
        let label = case.label;
        let (status, refused) = emulator
            .associate(json!({"Repository": case.repository, "ClientRequestToken": token}))
            .await;
        assert_eq!(
            status, 409,
            "case {label}: the token belongs to another repository, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {label}: a reused token is a ConflictException, got {refused}"
        );
    }

    // The request the token WAS used for still replays.
    let (status, replayed) = emulator
        .associate(json!({"Repository": first.repository, "ClientRequestToken": token}))
        .await;
    assert_eq!(status, 200, "replaying the claimed request, got {replayed}");
    assert_eq!(
        text(&replayed["RepositoryAssociation"], "AssociationId"),
        claimed,
        "the token must replay the association it claimed, got {replayed}"
    );

    // The same rule on code reviews: a token is bound to one review name + association.
    let association_arn = text(&created["RepositoryAssociation"], "AssociationArn");
    let review_body = |name: &str| {
        json!({
            "Name": name,
            "RepositoryAssociationArn": association_arn,
            "Type": {"RepositoryAnalysis": {"RepositoryHead": {"BranchName": "main"}}},
            "ClientRequestToken": token,
        })
    };
    let (status, review) = emulator.create_code_review(review_body("first")).await;
    assert_eq!(status, 200, "claiming the review token, got {review}");
    let review_arn = text(&review["CodeReview"], "CodeReviewArn");

    for name in ["second", "third"] {
        let (status, refused) = emulator.create_code_review(review_body(name)).await;
        assert_eq!(
            status, 409,
            "case {name}: the token belongs to another code review, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {name}: a reused token is a ConflictException, got {refused}"
        );
    }
    let (status, replayed) = emulator.create_code_review(review_body("first")).await;
    assert_eq!(status, 200, "replaying the claimed review, got {replayed}");
    assert_eq!(
        text(&replayed["CodeReview"], "CodeReviewArn"),
        review_arn,
        "the token must replay the code review it claimed, got {replayed}"
    );
}

/// RULE: a request the model cannot constrain is refused as a 400 ValidationException —
/// `Repository` names exactly one provider, and an S3 repository's bucket carries the
/// documented `codeguru-reviewer-` prefix.
#[tokio::test]
async fn an_unsatisfiable_repository_is_refused_as_a_validation_error() {
    let emulator = Emulator::new();

    let cases = vec![
        ("no member set", json!({"Repository": {}})),
        (
            "two members set",
            json!({"Repository": {
                "CodeCommit": {"Name": "one"},
                "S3Bucket": {"Name": "two", "BucketName": "codeguru-reviewer-two"},
            }}),
        ),
        (
            "bucket without the documented prefix",
            json!({"Repository": {"S3Bucket": {"Name": "three", "BucketName": "my-own-bucket"}}}),
        ),
    ];
    for (label, body) in cases {
        let (status, refused) = emulator.associate(body).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {label}: a bad request is a ValidationException, got {refused}"
        );
    }
}

/// RULE: a listing returns the stored associations in creation order, `MaxResults` per page,
/// with an opaque `NextToken` that resumes exactly after the last item returned; a token
/// this service did not issue is refused.
#[tokio::test]
async fn listing_pages_over_every_stored_association_exactly_once() {
    let emulator = Emulator::new();

    let total = 7;
    let mut created = Vec::new();
    for index in 0..total {
        let (status, body) = emulator
            .associate(json!({"Repository": {"CodeCommit": {"Name": format!("repo-{index}")}}}))
            .await;
        assert_eq!(status, 200, "creating repo-{index}, got {status} {body}");
        created.push(text(&body["RepositoryAssociation"], "AssociationArn"));
    }

    // One row per page size, including 1 (every page holds a single item), a size that does
    // not divide the total, and a size larger than the whole store.
    for page_size in [1usize, 2, 3, 7, 100] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("?MaxResults={page_size}"),
                Some(t) => format!("?MaxResults={page_size}&NextToken={t}"),
            };
            let (status, body) = emulator.list_associations(&query).await;
            assert_eq!(
                status, 200,
                "case page_size={page_size}: {query} must succeed, got {status} {body}"
            );
            let page = body["RepositoryAssociationSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case page_size={page_size}: no summaries in {body}"))
                .clone();
            assert!(
                page.len() <= page_size,
                "case page_size={page_size}: a page returned {} items",
                page.len()
            );
            for summary in &page {
                seen.push(text(summary, "AssociationArn"));
            }
            pages += 1;
            assert!(
                pages <= total + 1,
                "case page_size={page_size}: pagination did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, created,
            "case page_size={page_size}: paging must yield every association once, in creation order"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            total,
            "case page_size={page_size}: paging must not repeat an association"
        );
    }

    for (label, token) in [
        ("not hex", "zzzz"),
        ("hex but not a cursor", "6e6f7065"),
        ("hex of a negative number", "2d31"),
    ] {
        let (status, refused) = emulator
            .list_associations(&format!("?NextToken={token}"))
            .await;
        assert_eq!(
            status, 400,
            "case {label}: a token this service never issued must be refused, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {label}: a bad NextToken is a ValidationException, got {refused}"
        );
    }

    // `MaxResults` is modelled `range 1..100`: each published bound is served, and a value
    // past either bound is refused rather than adjusted into range.
    for (label, max_results, expected) in [
        ("lower bound", "1", 200),
        ("upper bound", "100", 200),
        ("below the lower bound", "0", 400),
        ("negative", "-1", 400),
        ("above the upper bound", "101", 400),
    ] {
        let (status, body) = emulator
            .list_associations(&format!("?MaxResults={max_results}"))
            .await;
        assert_eq!(
            status, expected,
            "case {label}: MaxResults={max_results} expected {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label}: an out-of-range MaxResults is a ValidationException, got {body}"
            );
        }
    }
}

/// RULE: a listing filter selects the stored associations whose member equals one of the
/// filter's values; separate filters all have to match.
#[tokio::test]
async fn listing_filters_select_on_the_stored_members() {
    let emulator = Emulator::new();
    for case in provider_cases() {
        let (status, body) = emulator
            .associate(json!({"Repository": case.repository}))
            .await;
        assert_eq!(status, 200, "case {}: associate, got {body}", case.label);
    }

    // Each row: a query, and the association names it must select.
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("provider", "?ProviderType=CodeCommit", vec!["ledger-service"]),
        (
            "two providers",
            "?ProviderType=Bitbucket&ProviderType=S3Bucket",
            vec!["payments.api", "archive-repo"],
        ),
        ("owner", "?Owner=platform", vec!["internal_tools"]),
        (
            "two states, one of which every new association is in",
            "?State=Associating&State=Associated",
            vec![
                "ledger-service",
                "payments.api",
                "internal_tools",
                "archive-repo",
            ],
        ),
        ("state nothing is in", "?State=Failed", vec![]),
        (
            "name and provider together",
            "?Name=payments.api&ProviderType=Bitbucket",
            vec!["payments.api"],
        ),
        (
            "name and a provider it is not",
            "?Name=payments.api&ProviderType=CodeCommit",
            vec![],
        ),
    ];
    for (label, query, expected) in cases {
        let (status, body) = emulator.list_associations(query).await;
        assert_eq!(
            status, 200,
            "case {label}: {query} must succeed, got {status} {body}"
        );
        let names: Vec<String> = body["RepositoryAssociationSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: no summaries in {body}"))
            .iter()
            .map(|summary| text(summary, "Name"))
            .collect();
        assert_eq!(
            names, expected,
            "case {label}: {query} selected the wrong associations"
        );
    }
}

/// RULE: a code review is created against a stored association, copies that association's
/// repository identity, and is read back from the store unchanged; an association that is
/// absent is the operation's 404.
#[tokio::test]
async fn a_code_review_copies_its_association_and_is_read_back() {
    let emulator = Emulator::new();
    let (_, created) = emulator
        .associate(json!({"Repository": {"CodeCommit": {"Name": "ledger-service"}}}))
        .await;
    let association = &created["RepositoryAssociation"];
    let association_arn = text(association, "AssociationArn");

    // One row per shape `Type.RepositoryAnalysis` can take, with and without AnalysisTypes.
    let cases = vec![
        (
            "repository head",
            json!({"RepositoryAnalysis": {"RepositoryHead": {"BranchName": "main"}}}),
            json!({"RepositoryHead": {"BranchName": "main"}}),
            Value::Null,
        ),
        (
            "repository head with both analysis types",
            json!({
                "RepositoryAnalysis": {"RepositoryHead": {"BranchName": "release/2.0"}},
                "AnalysisTypes": ["Security", "CodeQuality"],
            }),
            json!({"RepositoryHead": {"BranchName": "release/2.0"}}),
            json!(["Security", "CodeQuality"]),
        ),
        (
            "explicit source code type",
            json!({
                "RepositoryAnalysis": {"SourceCodeType": {
                    "BranchDiff": {
                        "SourceBranchName": "feature",
                        "DestinationBranchName": "main",
                    },
                }},
                "AnalysisTypes": ["Security"],
            }),
            json!({"BranchDiff": {
                "SourceBranchName": "feature",
                "DestinationBranchName": "main",
            }}),
            json!(["Security"]),
        ),
    ];

    for (index, (label, review_type, expected_source, expected_analysis)) in
        cases.into_iter().enumerate()
    {
        let (status, body) = emulator
            .create_code_review(json!({
                "Name": format!("review-{index}"),
                "RepositoryAssociationArn": association_arn,
                "Type": review_type,
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {label}: expected 200, got {status} {body}"
        );
        let review = &body["CodeReview"];
        assert_eq!(
            text(review, "Name"),
            format!("review-{index}"),
            "case {label}: Name comes from the request, got {review}"
        );
        let review_arn = text(review, "CodeReviewArn");
        assert!(
            review_arn.starts_with(&format!(
                "arn:aws:codeguru-reviewer:{REGION}:{ACCOUNT}:code-review:"
            )),
            "case {label}: CodeReviewArn must be a code-review ARN in the signed region, got {review_arn:?}"
        );
        assert_eq!(
            text(review, "AssociationArn"),
            association_arn,
            "case {label}: the review must name the association it was created against"
        );
        assert_eq!(
            text(review, "RepositoryName"),
            text(association, "Name"),
            "case {label}: RepositoryName is copied from the association"
        );
        assert_eq!(
            text(review, "Owner"),
            text(association, "Owner"),
            "case {label}: Owner is copied from the association"
        );
        assert_eq!(
            text(review, "ProviderType"),
            text(association, "ProviderType"),
            "case {label}: ProviderType is copied from the association"
        );
        assert_eq!(
            text(review, "Type"),
            "RepositoryAnalysis",
            "case {label}: CreateCodeReview creates this type only, got {review}"
        );
        assert_eq!(
            text(review, "State"),
            "Pending",
            "case {label}: a just-created review has not run yet, got {review}"
        );
        assert_eq!(
            review["SourceCodeType"], expected_source,
            "case {label}: SourceCodeType must report what Type.RepositoryAnalysis named"
        );
        assert_eq!(
            review["AnalysisTypes"], expected_analysis,
            "case {label}: AnalysisTypes must report what the request named"
        );
        assert!(
            review["Metrics"].is_null(),
            "case {label}: no analysis has run, so there are no metrics, got {review}"
        );

        // ROUND TRIP through the implemented reader. Every member that identifies the
        // review must read back unchanged; `State`/`StateReason`/`LastUpdatedTimeStamp` are
        // the three the clock is allowed to move once the run comes due, so they are
        // checked against the states this operation can leave the review in instead of
        // against the create's response.
        let (status, described) = emulator.describe_code_review(&review_arn).await;
        assert_eq!(
            status, 200,
            "case {label}: describing {review_arn} must succeed, got {status} {described}"
        );
        let read = &described["CodeReview"];
        for member in [
            "Name",
            "CodeReviewArn",
            "AssociationArn",
            "RepositoryName",
            "Owner",
            "ProviderType",
            "Type",
            "SourceCodeType",
            "AnalysisTypes",
            "Metrics",
            "ConfigFileState",
            "PullRequestId",
            "CreatedTimeStamp",
        ] {
            assert_eq!(
                at(read, member),
                at(review, member),
                "case {label}: {member} read back must equal what the create stored"
            );
        }
        let read_state = text(read, "State");
        assert!(
            read_state == "Pending" || read_state == "Failed",
            "case {label}: a RepositoryAnalysis is Pending until its run is due and then \
             terminal, got {read_state:?}"
        );
    }

    // A second review reusing a stored name conflicts: "The name of each code review in
    // your Amazon Web Services account must be unique."
    let (status, conflict) = emulator
        .create_code_review(json!({
            "Name": "review-0",
            "RepositoryAssociationArn": association_arn,
            "Type": {"RepositoryAnalysis": {"RepositoryHead": {"BranchName": "main"}}},
        }))
        .await;
    assert_eq!(
        status, 409,
        "a duplicate code-review name must conflict, got {status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "CreateCodeReview models ConflictException, got {conflict}"
    );

    // An association that was never minted, and a code review that was never created.
    for (label, id) in [
        ("all zeroes", "00000000-0000-4000-8000-000000000000"),
        ("mixed", "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"),
    ] {
        let (status, body) = emulator
            .create_code_review(json!({
                "Name": format!("orphan-{id}").replace('-', "_"),
                "RepositoryAssociationArn": self::association_arn(id),
                "Type": {"RepositoryAnalysis": {"RepositoryHead": {"BranchName": "main"}}},
            }))
            .await;
        assert_eq!(
            status, 404,
            "case {label}: an unknown association must be 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label}: CreateCodeReview models ResourceNotFoundException, got {body}"
        );

        let unknown = format!("arn:aws:codeguru-reviewer:{REGION}:{ACCOUNT}:code-review:{id}");
        let (status, body) = emulator.describe_code_review(&unknown).await;
        assert_eq!(
            status, 404,
            "case {label}: an unknown code review must be 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label}: DescribeCodeReview models ResourceNotFoundException, got {body}"
        );
    }

    // `RepositoryAnalysis` names neither the branch nor an explicit source code type.
    let (status, refused) = emulator
        .create_code_review(json!({
            "Name": "no-source",
            "RepositoryAssociationArn": association_arn,
            "Type": {"RepositoryAnalysis": {}},
        }))
        .await;
    assert_eq!(
        status, 400,
        "a RepositoryAnalysis with no source must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "a bad request is a ValidationException, got {refused}"
    );
}

/// RULE: disassociating a stored association releases the repository at once, leaves the
/// record readable while the release is still in flight, and makes a second disassociation —
/// and any new code review against it — a conflict with the state it is now in. Once the
/// release comes due the record is gone.
#[tokio::test]
async fn disassociating_releases_the_repository_and_then_removes_the_record() {
    let emulator = Emulator::new();
    let mut released_arns = Vec::new();

    for case in provider_cases() {
        let label = case.label;
        let (_, created) = emulator
            .associate(json!({
                "Repository": case.repository,
                "Tags": {"Env": "prod"},
            }))
            .await;
        let arn = text(&created["RepositoryAssociation"], "AssociationArn");

        let (status, released) = emulator.disassociate(&arn).await;
        assert_eq!(
            status, 200,
            "case {label}: disassociating {arn} must succeed, got {status} {released}"
        );
        assert_eq!(
            text(&released["RepositoryAssociation"], "State"),
            "Disassociating",
            "case {label}: the association is being released, got {released}"
        );
        assert_eq!(
            released["Tags"],
            json!({"Env": "prod"}),
            "case {label}: DisassociateRepository returns the association's tags, got {released}"
        );

        // Until the release comes due the record stays readable in a state that no longer
        // holds the repository; once it is due the record is gone. Both outcomes are
        // accepted here because which one a follow-up read sees depends on wall-clock
        // timing: the response of the starting call above pins the transient state, and the
        // post-loop assertions below pin the settled one.
        let (status, described) = emulator.describe_association(&arn).await;
        match status {
            200 => assert_eq!(
                text(&described["RepositoryAssociation"], "State"),
                "Disassociating",
                "case {label}: a release still in flight reports Disassociating, got {described}"
            ),
            404 => assert_eq!(
                error_code(&described),
                "NotFoundException",
                "case {label}: a completed release reads as NotFoundException, got {described}"
            ),
            other => panic!(
                "case {label}: describing a releasing association must be 200 or 404, got {other} {described}"
            ),
        }

        let (status, again) = emulator.disassociate(&arn).await;
        match status {
            409 => assert_eq!(
                error_code(&again),
                "ConflictException",
                "case {label}: releasing one that is still releasing conflicts, got {again}"
            ),
            404 => assert_eq!(
                error_code(&again),
                "NotFoundException",
                "case {label}: releasing one that is already gone is not found, got {again}"
            ),
            other => panic!(
                "case {label}: releasing twice must be 409 or 404, never success, got {other} {again}"
            ),
        }

        let (status, refused) = emulator
            .create_code_review(json!({
                "Name": format!("after-release-{label}"),
                "RepositoryAssociationArn": arn,
                "Type": {"RepositoryAnalysis": {"RepositoryHead": {"BranchName": "main"}}},
            }))
            .await;
        match status {
            409 => assert_eq!(
                error_code(&refused),
                "ConflictException",
                "case {label}: a releasing association cannot be reviewed, got {refused}"
            ),
            404 => assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {label}: a removed association cannot be reviewed, got {refused}"
            ),
            other => panic!(
                "case {label}: reviewing a released association must be 409 or 404, never success, got {other} {refused}"
            ),
        }

        // It no longer appears as Associated, and the repository can be associated again.
        let (_, listed) = emulator.list_associations("?State=Associated").await;
        let arns: Vec<String> = listed["RepositoryAssociationSummaries"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|summary| text(summary, "AssociationArn"))
            .collect();
        assert!(
            !arns.contains(&arn),
            "case {label}: a released association must not be listed as Associated, got {listed}"
        );

        let (status, reassociated) = emulator
            .associate(json!({"Repository": case.repository}))
            .await;
        assert_eq!(
            status, 200,
            "case {label}: the repository is free to associate again, got {status} {reassociated}"
        );
        assert_ne!(
            text(&reassociated["RepositoryAssociation"], "AssociationId"),
            text(&created["RepositoryAssociation"], "AssociationId"),
            "case {label}: re-associating must mint a new association"
        );
        released_arns.push((label, arn));
    }

    // Once each release comes due the record is gone, so the store does not grow under
    // associate/disassociate churn.
    tokio::time::sleep(PAST_SETTLE).await;
    for (label, arn) in &released_arns {
        let (status, body) = emulator.describe_association(arn).await;
        assert_eq!(
            status, 404,
            "case {label}: a completed release removes {arn}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label}: a removed association reads as NotFoundException, got {body}"
        );
    }
    let (_, listed) = emulator.list_associations("?MaxResults=100").await;
    let remaining: Vec<String> = listed["RepositoryAssociationSummaries"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| text(summary, "AssociationArn"))
        .collect();
    assert_eq!(
        remaining.len(),
        provider_cases().len(),
        "only the re-associated repositories remain stored, got {listed}"
    );
    for (label, arn) in &released_arns {
        assert!(
            !remaining.contains(arn),
            "case {label}: {arn} must be gone from the listing, got {listed}"
        );
    }
}

/// RULE: the store is capacity-bounded — at its maximum a create is declined with a modelled
/// error instead of being accepted, and the store becomes writable again once a record is
/// released.
#[tokio::test]
async fn the_association_store_is_capacity_bounded() {
    let emulator = Emulator::new();
    let capacity = service_com_amazonaws_codegurureviewer_awsgurufrontendservice::state::MAX_ASSOCIATIONS;

    let mut first_arn = String::new();
    for index in 0..capacity {
        let (status, body) = emulator
            .associate(json!({"Repository": {"CodeCommit": {"Name": format!("repo-{index}")}}}))
            .await;
        assert_eq!(
            status, 200,
            "case index={index}: a create below the cap must succeed, got {status} {body}"
        );
        if index == 0 {
            first_arn = text(&body["RepositoryAssociation"], "AssociationArn");
        }
    }

    let (status, refused) = emulator
        .associate(json!({"Repository": {"CodeCommit": {"Name": "one-too-many"}}}))
        .await;
    assert_eq!(
        status, 429,
        "case at-capacity: the {capacity}th+1 association must be declined, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ThrottlingException",
        "case at-capacity: AssociateRepository declines with a modelled error, got {refused}"
    );

    // Releasing one makes room again, so the bound is a capacity and not a dead end.
    let (status, body) = emulator.disassociate(&first_arn).await;
    assert_eq!(status, 200, "releasing {first_arn}, got {status} {body}");
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, body) = emulator
        .associate(json!({"Repository": {"CodeCommit": {"Name": "one-too-many"}}}))
        .await;
    assert_eq!(
        status, 200,
        "case after-release: a freed slot must accept a new association, got {status} {body}"
    );
}

/// RULE: a status this service reports as non-terminal settles on its own, without a second
/// call being what moves it: an association reaches `Associated`, and a code review reaches
/// a terminal state carrying the reason it is in.
#[tokio::test]
async fn a_non_terminal_status_settles_on_the_clock() {
    let emulator = Emulator::new();
    let (_, created) = emulator
        .associate(json!({"Repository": {"CodeCommit": {"Name": "ledger-service"}}}))
        .await;
    let association = &created["RepositoryAssociation"];
    let arn = text(association, "AssociationArn");
    let (_, review) = emulator
        .create_code_review(json!({
            "Name": "settling-review",
            "RepositoryAssociationArn": arn,
            "Type": {"RepositoryAnalysis": {"RepositoryHead": {"BranchName": "main"}}},
        }))
        .await;
    let review_arn = text(&review["CodeReview"], "CodeReviewArn");

    // The span is non-zero, so the transient status is observable before it is due.
    assert_eq!(
        text(association, "State"),
        "Associating",
        "the association is transient at first, got {created}"
    );
    assert_eq!(
        text(&review["CodeReview"], "State"),
        "Pending",
        "the code review is transient at first, got {review}"
    );

    tokio::time::sleep(PAST_SETTLE).await;

    let (status, settled) = emulator.describe_association(&arn).await;
    assert_eq!(status, 200, "describing {arn}, got {status} {settled}");
    assert_eq!(
        text(&settled["RepositoryAssociation"], "State"),
        "Associated",
        "the association must settle to Associated, got {settled}"
    );
    assert_ne!(
        at(&settled["RepositoryAssociation"], "LastUpdatedTimeStamp"),
        at(&settled["RepositoryAssociation"], "CreatedTimeStamp"),
        "settling changed the association, so LastUpdatedTimeStamp must have moved: {settled}"
    );
    assert_eq!(
        text(&settled["RepositoryAssociation"], "AssociationArn"),
        arn,
        "settling must not change the association's identity, got {settled}"
    );

    let (status, settled_review) = emulator.describe_code_review(&review_arn).await;
    assert_eq!(
        status, 200,
        "describing {review_arn}, got {status} {settled_review}"
    );
    let state = text(&settled_review["CodeReview"], "State");
    assert_ne!(
        state, "Pending",
        "the code review must leave Pending once its run is due, got {settled_review}"
    );
    assert!(
        !text(&settled_review["CodeReview"], "StateReason").is_empty(),
        "a code review that did not produce recommendations must say why, got {settled_review}"
    );
    assert!(
        settled_review["CodeReview"]["Metrics"].is_null(),
        "no analysis ran, so no metrics may be reported, got {settled_review}"
    );

    // Reading twice in a row reports the same settled resource: a read is not what moves a
    // status.
    let (_, again) = emulator.describe_association(&arn).await;
    assert_eq!(
        again, settled,
        "a second read must report the same association"
    );
    let (_, review_again) = emulator.describe_code_review(&review_arn).await;
    assert_eq!(
        review_again, settled_review,
        "a second read must report the same code review"
    );
}
