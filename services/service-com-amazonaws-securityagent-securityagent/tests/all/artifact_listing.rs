// SPDX-License-Identifier: CC0-1.0

//! What `ListArtifacts` reports, end to end over the wire: the artifacts the store holds
//! for ONE agent space, paged.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name; the harness is `crate::support`. So nothing in this file can pass
//! against a handler that answers a canned body: a summary must come back out of the
//! shared store, under the artifacts `AddArtifact` persisted there.
//!
//! The rules this suite owns:
//!
//! * a listing reports every artifact the named agent space holds and nothing else, each
//!   summary carrying the three members the shape declares, read off the stored record;
//! * it tracks the store, so an upload appears in it and a delete disappears from it;
//! * a page holds at most `maxResults` summaries and `nextToken` resumes exactly where
//!   the page ended, so paging reaches every artifact once whatever the page size;
//! * a space the account does not hold has no artifacts to list, and a page request the
//!   service cannot act on is refused rather than served.
//!
//! Each case asserts a rule over a TABLE of inputs: the rows differ along the number of
//! artifacts uploaded, the page size asked for (one, several, the exact count, past the
//! count, absent), which space is listed, and which artifact is deleted.

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    MAX_ARTIFACTS_PER_AGENT_SPACE, MAX_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member, ok};

/// The sentence a refusal of an agent space the account does not hold states: wholly
/// FIXED text, naming the resource class under the name the service's own records use for
/// an agent space and carrying nothing the request sent.
const AGENT_SPACE_NOT_FOUND: &str = "The specified agent instance does not exist.";

/// One artifact as a summary reports it: the three members `ArtifactSummary` declares
/// (`smithy_query(query="shape",shape="ArtifactSummary")` — `artifactId`, `fileName`,
/// `artifactType`, each `@required`).
#[derive(Debug, PartialEq, Eq)]
struct Summary {
    artifact_id: String,
    file_name: String,
    artifact_type: String,
}

/// Which of a space's artifacts a delete case removes, so the rule is asserted wherever
/// in the listing the removed record sat.
#[derive(Debug, Clone, Copy)]
enum Position {
    First,
    Last,
    Middle,
}

impl Position {
    /// The index this position names among `len` artifacts.
    fn index(self, len: usize) -> usize {
        match self {
            Self::First => 0,
            Self::Last => len - 1,
            Self::Middle => len / 2,
        }
    }
}

/// `CreateAgentSpace`, as the id of the record it minted.
async fn create_agent_space(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = ok(router, "CreateAgentSpace", &json!({"name": name}), case).await;
    member(&created, "agentSpaceId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateAgentSpace minted no id: {created}"))
}

/// `AddArtifact`, as the minted `artifactId`.
async fn add_artifact(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    file_name: &str,
    artifact_type: &str,
    case: &str,
) -> String {
    let answer = call(
        router,
        "AddArtifact",
        &json!({
            "agentSpaceId": agent_space_id,
            "fileName": file_name,
            "artifactType": artifact_type,
            // A blob travels as base64 on this protocol, with the codec the wire
            // serializer uses.
            "artifactContent": aws_smithy_types::base64::encode(file_name.as_bytes()),
        }),
    )
    .await;
    // The operation's own success status (`smithy_query(query="operation",
    // operation="AddArtifact")`: http code 201).
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected AddArtifact to answer 201, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    member(&answer.body, "artifactId")
        .unwrap_or_else(|| panic!("case {case:?}: AddArtifact minted no id: {}", answer.body))
}

/// One page of `ListArtifacts`, as the whole answer it gave.
async fn list_page(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    page_size: Option<i32>,
    token: Option<&str>,
) -> crate::support::Answer {
    let mut request = json!({"agentSpaceId": agent_space_id});
    if let Some(size) = page_size {
        request["maxResults"] = json!(size);
    }
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    call(router, "ListArtifacts", &request).await
}

/// The summaries of one page, in the order the page carried them.
fn summaries(page: &Value, case: &str) -> Vec<Summary> {
    page.get("artifactSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: the page carries no artifactSummaries: {page}"))
        .iter()
        .map(|summary| Summary {
            artifact_id: member(summary, "artifactId").unwrap_or_else(|| {
                panic!("case {case:?}: a summary carries no artifactId: {summary}")
            }),
            file_name: member(summary, "fileName").unwrap_or_else(|| {
                panic!("case {case:?}: a summary carries no fileName: {summary}")
            }),
            artifact_type: member(summary, "artifactType").unwrap_or_else(|| {
                panic!("case {case:?}: a summary carries no artifactType: {summary}")
            }),
        })
        .collect()
}

/// Every summary of one agent space, paged to the end with `page_size` (or the service's
/// default page size when `None`), asserting the page contract on the way: a page holds at
/// most the size asked for, a page carrying a cursor is not empty, and paging terminates.
async fn list_all(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    page_size: Option<i32>,
    case: &str,
) -> Vec<Summary> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let answer = list_page(router, agent_space_id, page_size, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListArtifacts to answer 200, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        let page = summaries(&answer.body, case);
        if let Some(size) = page_size {
            assert!(
                page.len() <= size.min(MAX_PAGE_SIZE) as usize,
                "case {case:?}: a page held {} summaries for maxResults {size}: {:?}",
                page.len(),
                page
            );
        }
        let empty = page.is_empty();
        all.extend(page);
        token = member(&answer.body, "nextToken");
        if token.is_none() {
            break;
        }
        assert!(
            !empty,
            "case {case:?}: a page that carries a nextToken must not be empty: {}",
            answer.body
        );
        assert!(
            all.len() <= MAX_ARTIFACTS_PER_AGENT_SPACE,
            "case {case:?}: pagination did not terminate, got {} summaries",
            all.len()
        );
    }
    all
}

/// The uploads every case spans: they differ along the declared type and the file name,
/// the two members a summary carries beside the minted id. The types are members of the
/// `ArtifactType` enum (`smithy_query(query="shape",shape="ArtifactType")`).
fn uploads() -> Vec<(&'static str, &'static str)> {
    vec![
        ("architecture-notes.txt", "TXT"),
        ("design.md", "MD"),
        ("api.json", "JSON"),
        ("diagram.png", "PNG"),
        ("threat-model.pdf", "PDF"),
        ("spec.yaml", "YAML"),
        ("report.docx", "DOCX"),
    ]
}

/// RULE: a listing reports the artifacts the NAMED agent space holds, each summary read
/// off the stored record, and no artifact of another space — whatever page size it is
/// walked with.
#[tokio::test]
async fn a_listing_reports_the_stored_artifacts_of_the_named_space() {
    let router = emulator();
    let space = create_agent_space(&router, "listed-space", "the space being listed").await;
    let other = create_agent_space(&router, "other-space", "another space of the account").await;

    // A fresh space holds nothing, so the listing is empty rather than refused.
    let empty = list_all(&router, &space, None, "a space holding no artifact").await;
    assert!(
        empty.is_empty(),
        "case \"a space holding no artifact\": expected an empty listing, got {empty:?}"
    );

    let mut expected = Vec::new();
    for (file_name, artifact_type) in uploads() {
        let artifact_id = add_artifact(&router, &space, file_name, artifact_type, file_name).await;
        expected.push(Summary {
            artifact_id,
            file_name: file_name.to_string(),
            artifact_type: artifact_type.to_string(),
        });
    }
    // One artifact of ANOTHER space, which no page of this space's listing may carry.
    let elsewhere = add_artifact(&router, &other, "elsewhere.txt", "TXT", "another space").await;

    // (case, the page size the walk asks for) — the rows differ along the size: one at a
    // time, several, the exact count, past the count, past the service's ceiling, and the
    // default (`maxResults` absent).
    let sizes: Vec<(&str, Option<i32>)> = vec![
        ("one summary per page", Some(1)),
        ("three summaries per page", Some(3)),
        ("the exact count", Some(expected.len() as i32)),
        ("one past the count", Some(expected.len() as i32 + 1)),
        ("past the service's page ceiling", Some(MAX_PAGE_SIZE + 10)),
        ("the default page size", None),
    ];

    for (case, page_size) in sizes {
        let listed = list_all(&router, &space, page_size, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the stored artifacts {expected:?}, got {listed:?}"
        );
        assert!(
            listed.iter().all(|summary| summary.artifact_id != elsewhere),
            "case {case:?}: another space's artifact {elsewhere:?} must not be listed, got \
             {listed:?}"
        );
    }

    // The other space reports its own one artifact, so the filter is by the named space
    // and not by anything else.
    let listed_other = list_all(&router, &other, None, "the other space's own listing").await;
    assert_eq!(
        listed_other,
        vec![Summary {
            artifact_id: elsewhere.clone(),
            file_name: "elsewhere.txt".to_string(),
            artifact_type: "TXT".to_string(),
        }],
        "case \"the other space's own listing\": expected only {elsewhere:?}, got \
         {listed_other:?}"
    );
}

/// RULE: the listing tracks the store, so an upload appears in it and a delete disappears
/// from it, whichever artifact of the space is removed.
#[tokio::test]
async fn a_listing_follows_every_upload_and_delete() {
    let router = emulator();
    let space = create_agent_space(&router, "tracked-space", "the space being tracked").await;

    let mut live: Vec<Summary> = Vec::new();
    for (file_name, artifact_type) in uploads() {
        let artifact_id = add_artifact(&router, &space, file_name, artifact_type, file_name).await;
        live.push(Summary {
            artifact_id,
            file_name: file_name.to_string(),
            artifact_type: artifact_type.to_string(),
        });
        let listed = list_all(&router, &space, None, file_name).await;
        assert_eq!(
            listed, live,
            "case {file_name:?}: expected the upload to appear in the listing: expected \
             {live:?}, got {listed:?}"
        );
    }

    // (case, which of the remaining artifacts to delete) — the rows differ along the
    // position of the removed record: the first, the last, and one in the middle.
    let positions: [(&str, Position); 3] = [
        ("the first artifact", Position::First),
        ("the last artifact", Position::Last),
        ("one in the middle", Position::Middle),
    ];

    for (case, position) in positions {
        let index = position.index(live.len());
        let removed = live.remove(index);
        let deleted = call(
            &router,
            "DeleteArtifact",
            &json!({"agentSpaceId": space, "artifactId": removed.artifact_id}),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteArtifact to answer 200, got {} ({}) with {}",
            deleted.status, deleted.error_code, deleted.body
        );
        let listed = list_all(&router, &space, Some(2), case).await;
        assert_eq!(
            listed, live,
            "case {case:?}: expected the delete to leave {live:?}, got {listed:?}"
        );
    }
}

/// A `nextToken` each OTHER paginated listing of this service mints, paired with the
/// listing that minted it: a cursor names a resume point in the key space of the listing
/// that minted it, so no other listing may read one as its own.
///
/// The pack catalog a fresh account holds is already several records, so that listing
/// mints a cursor without creating anything; the applications are created here.
async fn foreign_cursors(router: &MultiServiceRouter) -> Vec<(&'static str, String)> {
    for index in 0..2 {
        ok(
            router,
            "CreateApplication",
            &json!({}),
            &format!("the application {index} behind a foreign cursor"),
        )
        .await;
    }
    let mut cursors = Vec::new();
    for listing in ["ListApplications", "ListSecurityRequirementPacks"] {
        let page = ok(router, listing, &json!({"maxResults": 1}), listing).await;
        let token = member(&page, "nextToken").unwrap_or_else(|| {
            panic!("case {listing:?}: expected a nextToken to resume from: {page}")
        });
        cursors.push((listing, token));
    }
    cursors
}

/// RULE: a listing of an agent space the account does not hold is refused with the
/// service's fixed sentence for that class, and a page request the service cannot act on —
/// a `nextToken` it never minted, or a page size no page can have — is refused as a bad
/// request rather than served.
#[tokio::test]
async fn a_listing_the_service_cannot_serve_is_refused() {
    let router = emulator();
    let space = create_agent_space(&router, "refusal-space", "the space that exists").await;
    let artifact = add_artifact(&router, &space, "notes.txt", "TXT", "the stored artifact").await;

    // (case, the agentSpaceId the request carries) — the rows differ along how the id
    // misses the store: never minted, a near miss of the stored id, and the stored id with
    // a suffix.
    let unknown_spaces: Vec<(&str, String)> = vec![
        ("an id no create minted", "f".repeat(space.len())),
        (
            "the stored id with its last character dropped",
            space[..space.len() - 1].to_string(),
        ),
        ("the stored id with a suffix", format!("{space}0")),
    ];

    for (case, agent_space_id) in unknown_spaces {
        let answer = list_page(&router, &agent_space_id, None, None).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        let actual = member(&answer.body, "message")
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carried no message"));
        assert_eq!(
            actual, AGENT_SPACE_NOT_FOUND,
            "case {case:?}: expected {AGENT_SPACE_NOT_FOUND:?}, got {actual:?}"
        );
    }

    // A cursor another listing of this service minted: it names a resume point in that
    // listing's own key space, so this listing must refuse it rather than page the
    // artifacts from it. Both listings are walked one record at a time so each mints one.
    let foreign_tokens = foreign_cursors(&router).await;

    // (case, the page request) — the rows differ along WHICH part of the page request the
    // service cannot act on: a token of another shape, a token that is not one at all, a
    // cursor another listing minted, and a page size that cannot hold a summary.
    let mut bad_pages: Vec<(String, Option<i32>, Option<String>)> = vec![
        (
            "a token this service never minted".to_string(),
            None,
            Some("not-a-token".to_string()),
        ),
        (
            "a token that is an artifact id".to_string(),
            None,
            Some(artifact.clone()),
        ),
        ("an empty token".to_string(), None, Some(String::new())),
        ("a page size of zero".to_string(), Some(0), None),
        ("a negative page size".to_string(), Some(-1), None),
    ];
    for (listing, token) in foreign_tokens {
        bad_pages.push((format!("a nextToken minted by {listing}"), None, Some(token)));
    }

    for (case, page_size, token) in bad_pages {
        let answer = list_page(&router, &space, page_size, token.as_deref()).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }

    // The empty `agentSpaceId` names no space and never could, so it is a bad request
    // rather than a missing resource.
    let empty = list_page(&router, "", None, None).await;
    assert_eq!(
        (empty.status, empty.error_code.as_str()),
        (400, "ValidationException"),
        "case \"the empty agentSpaceId\": expected 400 ValidationException, got {} ({}) \
         with {}",
        empty.status,
        empty.error_code,
        empty.body
    );

    // The space the account DOES hold still lists its one artifact, so the refusals above
    // are about the request and nothing else.
    let listed = list_all(&router, &space, None, "the space that exists").await;
    assert_eq!(
        listed.len(),
        1,
        "case \"the space that exists\": expected its one artifact, got {listed:?}"
    );
}
