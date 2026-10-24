// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CodeStar connections repository-link surface.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front door builds (`POST /` + `X-Amz-Target`, awsJson1_0 body), reusing the
//! `connection_lifecycle` suite's harness, so what is asserted here is the service's
//! HTTP behaviour and not a handler called directly.
//!
//! RULE 1 (`DeleteRepositoryLink`) — a delete states the POST-STATE of the link its
//! `RepositoryLinkId` names: afterwards this service holds no link under that id, and
//! holds every other link unchanged. It answers HTTP 200 with an empty body whether or
//! not a link was there to remove, because the post-state already holds for an id that
//! names nothing (the sibling CodeConnections service — the same API under its current
//! name — has exactly that recorded for an id naming no link).
//!
//! RULE 2 (`CreateRepositoryLink`) — a repository link IS the
//! (`ConnectionArn`, `OwnerId`, `RepositoryName`) triple: a create registers that triple
//! under a freshly minted id and `repository-link/<id>` ARN in the region the caller
//! signed for, reports `ProviderType` off the CONNECTION the link names, and refuses a
//! triple already registered with the modelled `ResourceAlreadyExistsException` — at the
//! HTTP 400 this service's recorded wire answers that refusal with, rather than the
//! `@httpError 409` the shape declares (the crate's
//! `scaffolded::Scaffolded::modeled_with_status` channel).
//!
//! RULE 3 (routing) — a request naming a modelled operation in `X-Amz-Target` reaches
//! that operation whatever the target prefix in front of the name, so the published
//! dotted prefix an SDK sends reaches the same handler the short prefix does.
//!
//! The tables below span the classes those rules generalise over — every modelled
//! `ProviderType`, both published bounds of `OwnerId` and `RepositoryName`, a link with
//! and without an encryption key, several tag sets, several ids naming nothing, every
//! target-prefix form — rather than one recorded request.
//!
//! The reader each assertion reads state back through is `ListTagsForResource` on the
//! link's own ARN (a link is a resource of this service, so it is taggable) together
//! with `CreateRepositoryLink`'s duplicate-triple refusal: `GetRepositoryLink` and
//! `ListRepositoryLinks` are still scaffold stubs with their own open issues
//! (`20261023T122922Z-codestar-connections-getrepositorylink-stub.md`,
//! `20261023T122922Z-codestar-connections-listrepositorylinks-stub.md`).

use serde_json::{Value, json};

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use super::connection_lifecycle::{ACCOUNT_ID, Emulator, REGION, TARGET_PREFIX, error_code};

/// Every value the model's `ProviderType` enum accepts.
///
/// Referenced, not copied: this is the generated catalog
/// (`src/ops/types/shared_types.rs::PROVIDER_TYPE_VALUES`, from
/// `smithy_query(query="shape", shape="ProviderType")`), so the table below widens with
/// the model instead of drifting from it.
const PROVIDER_TYPES: &[&str] = codestar::ops::types::shared_types::PROVIDER_TYPE_VALUES;

/// The published bounds of `CreateRepositoryLink.OwnerId` (`length 1..255`) and of
/// `CreateRepositoryLink.RepositoryName` (`length 1..128`), from
/// `smithy_query(query="operation", operation="CreateRepositoryLink")`.
const OWNER_ID_MIN_LEN: usize = 1;
const OWNER_ID_MAX_LEN: usize = 255;
const REPOSITORY_NAME_MIN_LEN: usize = 1;
const REPOSITORY_NAME_MAX_LEN: usize = 128;

/// One row of the create/delete table: a repository link to register, differing from the
/// other rows along what the rules generalise over.
struct Row {
    case: &'static str,
    /// The `ProviderType` the backing connection is created with — what
    /// `RepositoryLinkInfo.ProviderType` must report, since it is the provider type of
    /// the connection the link names.
    provider_type: &'static str,
    owner_id: String,
    repository_name: String,
    /// `EncryptionKeyArn`, for the rows that supply one.
    encryption_key_arn: Option<String>,
    /// Tags supplied at create, as (key, value) pairs.
    tags: Vec<(String, String)>,
}

/// A KMS key ARN of the shape `CreateRepositoryLink.EncryptionKeyArn`'s modelled pattern
/// requires (`^arn:aws(-[\w]+)*:kms:[a-z\-0-9]+:\d{12}:key/[a-zA-Z0-9\-]+$`).
fn encryption_key_arn(key_id: &str) -> String {
    format!("arn:aws:kms:{REGION}:{ACCOUNT_ID}:key/{key_id}")
}

/// The rows: every modelled provider type, both published length bounds of `OwnerId` and
/// `RepositoryName`, a link with an encryption key and links with 0, 1 and several tags.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = PROVIDER_TYPES
        .iter()
        .map(|provider_type| Row {
            case: "every modelled provider type",
            provider_type,
            owner_id: format!("owner-{}", provider_type.to_ascii_lowercase()),
            repository_name: format!("repo-{}", provider_type.to_ascii_lowercase()),
            encryption_key_arn: None,
            tags: Vec::new(),
        })
        .collect();
    rows.push(Row {
        case: "OwnerId and RepositoryName at their shortest published length",
        provider_type: "GitHub",
        owner_id: "o".repeat(OWNER_ID_MIN_LEN),
        repository_name: "r".repeat(REPOSITORY_NAME_MIN_LEN),
        encryption_key_arn: None,
        tags: Vec::new(),
    });
    rows.push(Row {
        case: "OwnerId and RepositoryName at their longest published length",
        provider_type: "GitLab",
        owner_id: "o".repeat(OWNER_ID_MAX_LEN),
        repository_name: "r".repeat(REPOSITORY_NAME_MAX_LEN),
        encryption_key_arn: None,
        tags: Vec::new(),
    });
    rows.push(Row {
        case: "a link with a customer-managed encryption key",
        provider_type: "Bitbucket",
        owner_id: "owner-with-key".to_string(),
        repository_name: "repo-with-key".to_string(),
        encryption_key_arn: Some(encryption_key_arn(
            "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d",
        )),
        tags: Vec::new(),
    });
    rows.push(Row {
        case: "a link carrying one tag",
        provider_type: "GitHubEnterpriseServer",
        owner_id: "owner-one-tag".to_string(),
        repository_name: "repo-one-tag".to_string(),
        encryption_key_arn: None,
        tags: vec![("Env".to_string(), "prod".to_string())],
    });
    rows.push(Row {
        case: "a link carrying several tags",
        provider_type: "GitLabSelfManaged",
        owner_id: "owner-many-tags".to_string(),
        repository_name: "repo-many-tags".to_string(),
        encryption_key_arn: Some(encryption_key_arn("alias-free-key-0000")),
        tags: vec![
            ("Env".to_string(), "staging".to_string()),
            ("Owner".to_string(), "git-sync".to_string()),
            ("Team".to_string(), "platform".to_string()),
        ],
    });
    rows
}

/// The `CreateRepositoryLink` request body for one row, against `connection_arn`.
fn create_body(row: &Row, connection_arn: &str) -> Value {
    let mut body = json!({
        "ConnectionArn": connection_arn,
        "OwnerId": row.owner_id,
        "RepositoryName": row.repository_name,
    });
    if let Some(key_arn) = row.encryption_key_arn.as_deref() {
        body["EncryptionKeyArn"] = json!(key_arn);
    }
    if !row.tags.is_empty() {
        body["Tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        );
    }
    body
}

/// Whether `candidate` is a `RepositoryLinkId`: the modelled pattern
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`
/// (`smithy_query(query="shape", shape="RepositoryLinkId")`), written out as a
/// group-length check so the test needs no regex crate.
fn is_repository_link_id(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8usize, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Assert the `RepositoryLinkInfo` one answer carries reports the row and the connection
/// it was created against, and return `(RepositoryLinkId, RepositoryLinkArn)`.
fn assert_link_info(case: &str, row: &Row, connection_arn: &str, info: &Value) -> (String, String) {
    let id = info["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinkId in {info}"))
        .to_string();
    assert!(
        is_repository_link_id(&id),
        "case {case:?}: expected a RepositoryLinkId matching the modelled 8-4-4-4-12 hex \
         pattern, got {id:?}"
    );
    let arn = info["RepositoryLinkArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinkArn in {info}"))
        .to_string();
    let expected_arn = format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:repository-link/{id}");
    assert_eq!(
        arn, expected_arn,
        "case {case:?}: expected the link ARN to be minted in the signed region for the \
         reported id, got {arn:?}"
    );
    assert_eq!(
        info["ConnectionArn"].as_str(),
        Some(connection_arn),
        "case {case:?}: expected the link to report the connection it was created \
         against, got {info}"
    );
    assert_eq!(
        info["OwnerId"].as_str(),
        Some(row.owner_id.as_str()),
        "case {case:?}: expected OwnerId {:?}, got {info}",
        row.owner_id
    );
    assert_eq!(
        info["RepositoryName"].as_str(),
        Some(row.repository_name.as_str()),
        "case {case:?}: expected RepositoryName {:?}, got {info}",
        row.repository_name
    );
    assert_eq!(
        info["ProviderType"].as_str(),
        Some(row.provider_type),
        "case {case:?}: expected the connection's provider type {:?}, got {info}",
        row.provider_type
    );
    assert_eq!(
        info["EncryptionKeyArn"].as_str(),
        row.encryption_key_arn.as_deref(),
        "case {case:?}: expected EncryptionKeyArn {:?}, got {info}",
        row.encryption_key_arn
    );
    (id, arn)
}

/// The tags `ListTagsForResource` reports for `resource_arn`, asserting the read itself
/// succeeded — the reader that proves a link is held in state.
async fn tags_of(emulator: &Emulator, case: &str, resource_arn: &str) -> Vec<(String, String)> {
    let (status, body) = emulator
        .call(
            "ListTagsForResource",
            json!({ "ResourceArn": resource_arn }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource to report the link's tags, got \
         {status} {body}"
    );
    let mut tags: Vec<(String, String)> = body["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Tags list in {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    tags.sort();
    tags
}

/// Assert one answer is an empty 200 — what a delete of this service answers: the
/// modelled `DeleteRepositoryLinkOutput` has no members, so the body is an empty JSON
/// object and carries nothing else.
fn assert_empty_ok(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteRepositoryLink to answer 200, got {status} {body}"
    );
    assert_eq!(
        body,
        &json!({}),
        "case {case:?}: expected an empty DeleteRepositoryLink body, got {body}"
    );
}

/// RULE 1 + RULE 2, over the whole table: a created link is held in state (its tags read
/// back under its own ARN, its triple refused a second time), a delete removes it (the
/// reader answers for an absent resource and the triple is free again), and the id minted
/// for the re-created link is a NEW one — a deleted link's id never comes back.
#[tokio::test]
async fn a_created_repository_link_is_held_until_a_delete_removes_it() {
    for row in rows() {
        let emulator = Emulator::new();
        let case = format!("{} ({})", row.case, row.provider_type);
        let connection_arn = emulator
            .create(
                &case,
                json!({
                    "ConnectionName": "link-backing-connection",
                    "ProviderType": row.provider_type,
                }),
            )
            .await;

        let (status, body) = emulator
            .call("CreateRepositoryLink", create_body(&row, &connection_arn))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateRepositoryLink to answer 200, got {status} {body}"
        );
        let (id, arn) = assert_link_info(&case, &row, &connection_arn, &body["RepositoryLinkInfo"]);

        // The link is in state: its create-time tags are readable under its own ARN.
        let mut expected_tags = row.tags.clone();
        expected_tags.sort();
        assert_eq!(
            tags_of(&emulator, &case, &arn).await,
            expected_tags,
            "case {case:?}: expected the link to hold exactly the tags it was created with"
        );

        // …and the triple it registered is taken: a second create naming it is refused
        // with the modelled error, whose message names the ARN already holding it.
        let (status, body) = emulator
            .call("CreateRepositoryLink", create_body(&row, &connection_arn))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a triple already registered, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceAlreadyExistsException",
            "case {case:?}: expected ResourceAlreadyExistsException, got {body}"
        );
        let message = body["Message"]
            .as_str()
            .or_else(|| body["message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(&arn),
            "case {case:?}: expected the duplicate refusal to name the stored link ARN \
             {arn:?}, got {message:?}"
        );

        // The delete.
        let (status, body) = emulator
            .call("DeleteRepositoryLink", json!({ "RepositoryLinkId": id }))
            .await;
        assert_empty_ok(&case, status, &body);

        // Afterwards the link is not there: the reader answers for an absent resource…
        let (status, body) = emulator
            .call("ListTagsForResource", json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted link's ARN to name no resource, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException after the delete, got {body}"
        );

        // …and its triple is free again, under a NEW id and ARN.
        let (status, body) = emulator
            .call("CreateRepositoryLink", create_body(&row, &connection_arn))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deleted triple to be registrable again, got \
             {status} {body}"
        );
        let (new_id, new_arn) =
            assert_link_info(&case, &row, &connection_arn, &body["RepositoryLinkInfo"]);
        assert_ne!(
            new_id, id,
            "case {case:?}: a deleted link's id must never be handed out again, got {new_id:?}"
        );
        assert_ne!(
            new_arn, arn,
            "case {case:?}: a deleted link's ARN must never be reused, got {new_arn:?}"
        );
    }
}

/// RULE 2's refusal wording: the duplicate refusal states the ARN that already holds the
/// triple and carries the framework tail, whose stated status and error code are the
/// response's own; its request id is minted per call, so two refusals of the same
/// duplicate differ in exactly that field.
#[tokio::test]
async fn the_duplicate_refusal_states_the_response_it_is_carried_on() {
    let emulator = Emulator::new();
    let connection_arn = emulator
        .create(
            "tail",
            json!({ "ConnectionName": "tail-connection", "ProviderType": "GitHub" }),
        )
        .await;
    let row = Row {
        case: "a link refused twice",
        provider_type: "GitHub",
        owner_id: "owner-tail".to_string(),
        repository_name: "repo-tail".to_string(),
        encryption_key_arn: None,
        tags: Vec::new(),
    };
    let (status, body) = emulator
        .call("CreateRepositoryLink", create_body(&row, &connection_arn))
        .await;
    assert_eq!(
        status, 200,
        "the first create must succeed, got {status} {body}"
    );
    let arn = body["RepositoryLinkInfo"]["RepositoryLinkArn"]
        .as_str()
        .expect("the created link reports its ARN")
        .to_string();

    let mut messages = Vec::new();
    for attempt in 1..=2 {
        let case = format!("duplicate attempt {attempt}");
        let (status, body) = emulator
            .call("CreateRepositoryLink", create_body(&row, &connection_arn))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the recorded client-error status, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceAlreadyExistsException",
            "case {case:?}: expected ResourceAlreadyExistsException, got {body}"
        );
        let message = body["Message"]
            .as_str()
            .or_else(|| body["message"].as_str())
            .unwrap_or_default()
            .to_string();
        for needle in [
            format!("RepoConfiguration already exists. Arn: {arn}").as_str(),
            "(Service: CodeConnections;",
            // The tail states the status and the code the response itself carries.
            "Status Code: 400;",
            "Error Code: ResourceAlreadyExistsException;",
            "Proxy: null)",
        ] {
            assert!(
                message.contains(needle),
                "case {case:?}: expected the refusal message to carry {needle:?}, got \
                 {message:?}"
            );
        }
        messages.push(message);
    }
    assert_ne!(
        messages[0], messages[1],
        "the tail's request id is minted per call, so two refusals must not report the \
         same one: {messages:?}"
    );
}

/// RULE 1, the post-state half: whether or not the id names a link this service holds,
/// the delete answers an empty 200 and the id names no link afterwards. Repeating a
/// delete is therefore the same answer as the first one.
#[tokio::test]
async fn deleting_an_id_that_names_no_link_is_the_same_empty_200() {
    let emulator = Emulator::new();
    let connection_arn = emulator
        .create(
            "idempotence",
            json!({ "ConnectionName": "idempotent-delete", "ProviderType": "GitHub" }),
        )
        .await;
    let row = Row {
        case: "a link to delete twice",
        provider_type: "GitHub",
        owner_id: "owner-idempotent".to_string(),
        repository_name: "repo-idempotent".to_string(),
        encryption_key_arn: None,
        tags: Vec::new(),
    };
    let (_, created) = emulator
        .call("CreateRepositoryLink", create_body(&row, &connection_arn))
        .await;
    let live_id = created["RepositoryLinkInfo"]["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRepositoryLink returned no id: {created}"))
        .to_string();

    // (case, an id this store holds no link for) — every one of them satisfying the
    // modelled `RepositoryLinkId` pattern, so the handler, not the framework's
    // validation, is what answers.
    let absent_ids = [
        (
            "an all-zero id",
            "00000000-0000-0000-0000-000000000000",
        ),
        (
            "an id this emulator never minted",
            "9f8e7d6c-5b4a-4938-8271-605f4e3d2c1b",
        ),
        (
            "the same id written in upper-case hex, which the pattern also admits",
            "9F8E7D6C-5B4A-4938-8271-605F4E3D2C1B",
        ),
    ];

    for (case, id) in absent_ids {
        let (status, body) = emulator
            .call("DeleteRepositoryLink", json!({ "RepositoryLinkId": id }))
            .await;
        assert_empty_ok(case, status, &body);
        // And deleting an id that names nothing removes nothing: the link this store
        // does hold still holds its triple.
        let (status, body) = emulator
            .call("CreateRepositoryLink", create_body(&row, &connection_arn))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting an id naming no link must leave the stored link \
             alone, got {status} {body}"
        );
    }

    // Repeating the delete of a link this store DID hold is the same answer as the
    // first: after the first one the id names nothing, which is the state every row
    // above started in.
    for attempt in 1..=3 {
        let case = format!("delete attempt {attempt} of the same live id");
        let (status, body) = emulator
            .call(
                "DeleteRepositoryLink",
                json!({ "RepositoryLinkId": live_id.clone() }),
            )
            .await;
        assert_empty_ok(&case, status, &body);
    }

    // The post-state the repeated deletes state: no link is held for the triple, so it
    // is registrable again.
    let (status, body) = emulator
        .call("CreateRepositoryLink", create_body(&row, &connection_arn))
        .await;
    assert_eq!(
        status, 200,
        "after the deletes no link may be held for the triple, got {status} {body}"
    );
}

/// RULE 1, the "every other link unchanged" half: a delete removes exactly the link its
/// id names. Driven over which link of a set is deleted, so no row's survival depends on
/// its position in the store.
#[tokio::test]
async fn a_delete_removes_exactly_the_link_its_id_names() {
    const LINKS: usize = 4;

    for deleted_index in 0..LINKS {
        let emulator = Emulator::new();
        let case = format!("deleting link {deleted_index} of {LINKS}");
        let connection_arn = emulator
            .create(
                &case,
                json!({ "ConnectionName": "many-links", "ProviderType": "GitHub" }),
            )
            .await;

        // One link per repository, all on the one connection.
        let mut links: Vec<(Row, String, String)> = Vec::new();
        for index in 0..LINKS {
            let row = Row {
                case: "one of a set",
                provider_type: "GitHub",
                owner_id: format!("owner-{index}"),
                repository_name: format!("repo-{index}"),
                encryption_key_arn: None,
                tags: vec![("Index".to_string(), index.to_string())],
            };
            let (status, body) = emulator
                .call("CreateRepositoryLink", create_body(&row, &connection_arn))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected link {index} to be created, got {status} {body}"
            );
            let (id, arn) =
                assert_link_info(&case, &row, &connection_arn, &body["RepositoryLinkInfo"]);
            links.push((row, id, arn));
        }

        let (_, deleted_id, deleted_arn) = &links[deleted_index];
        let (status, body) = emulator
            .call(
                "DeleteRepositoryLink",
                json!({ "RepositoryLinkId": deleted_id }),
            )
            .await;
        assert_empty_ok(&case, status, &body);

        for (index, (row, _, arn)) in links.iter().enumerate() {
            if index == deleted_index {
                let (status, body) = emulator
                    .call("ListTagsForResource", json!({ "ResourceArn": arn }))
                    .await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: the deleted link {index} must be gone, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException for the deleted \
                     link, got {body}"
                );
                continue;
            }
            assert_eq!(
                tags_of(&emulator, &case, arn).await,
                vec![("Index".to_string(), index.to_string())],
                "case {case:?}: link {index} must keep its tags across the delete of \
                 {deleted_arn:?}"
            );
            let (status, body) = emulator
                .call("CreateRepositoryLink", create_body(row, &connection_arn))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: link {index} must still hold its triple, got {status} {body}"
            );
        }
    }
}

/// A `RepositoryLinkId` outside the modelled pattern is refused by the framework's own
/// validation BEFORE the handler runs — a 400, never a delete of something else and
/// never the scaffold's 501. Each row differs in how it breaks the pattern.
#[tokio::test]
async fn a_repository_link_id_outside_the_modeled_pattern_is_refused() {
    let emulator = Emulator::new();

    // (case, id) — the modelled pattern is
    // `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`.
    let cases = [
        ("an empty id", ""),
        ("a label that is not a UUID at all", "not-a-uuid"),
        ("a group too short", "9f8e7d6-5b4a-4938-8271-605f4e3d2c1b"),
        ("a non-hex character", "9f8e7d6g-5b4a-4938-8271-605f4e3d2c1b"),
        ("the hyphens dropped", "9f8e7d6c5b4a49388271605f4e3d2c1b"),
        (
            "trailing text after a well-formed id",
            "9f8e7d6c-5b4a-4938-8271-605f4e3d2c1b-extra",
        ),
    ];

    for (case, id) in cases {
        let (status, body) = emulator
            .call("DeleteRepositoryLink", json!({ "RepositoryLinkId": id }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 for an id outside the modelled pattern, got \
             {status} {body}"
        );
        assert_ne!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: a refused request must not be answered as unimplemented: {body}"
        );
    }
}

/// RULE 3: the repository-link operations are reachable under every target-prefix form a
/// client writes — the short shape name the in-process tests use and the fully qualified
/// dotted prefix an AWS SDK of this service sends — so the answer is the operation's own
/// and never the framework's "operation <routing key> is not implemented in this
/// emulator", which means the target reached no operation at all.
#[tokio::test]
async fn the_repository_link_operations_are_reachable_under_every_target_prefix_form() {
    // The prefix forms: the service shape's local name (what `OPERATIONS.md` documents),
    // the absolute shape id (`smithy_query(query="service_info")` reports the service
    // shape `com.amazonaws.codestarconnections#CodeStar_connections_20191201`), and the
    // fully qualified Coral name an SDK of this service sends, which is the form the
    // recorded wire traffic carries.
    let prefixes = [
        ("the short shape name", TARGET_PREFIX.to_string()),
        (
            "the absolute shape id",
            format!("com.amazonaws.codestarconnections.{TARGET_PREFIX}"),
        ),
        (
            "the fully qualified Coral name an SDK sends",
            format!("com.amazonaws.codestar.connections.{TARGET_PREFIX}"),
        ),
    ];

    for (case, prefix) in prefixes {
        let emulator = Emulator::new();

        // A connection to anchor the link, through the same prefix form.
        let (status, body) = emulator
            .call_target(
                &format!("{prefix}.CreateConnection"),
                json!({ "ConnectionName": "prefix-form", "ProviderType": "GitHub" }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnection must be reachable under this prefix, got \
             {status} {body}"
        );
        let connection_arn = body["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ConnectionArn in {body}"))
            .to_string();

        let (status, body) = emulator
            .call_target(
                &format!("{prefix}.CreateRepositoryLink"),
                json!({
                    "ConnectionArn": connection_arn,
                    "OwnerId": "owner-prefix",
                    "RepositoryName": "repo-prefix",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateRepositoryLink must be reachable under this prefix, got \
             {status} {body}"
        );
        let id = body["RepositoryLinkInfo"]["RepositoryLinkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinkId in {body}"))
            .to_string();

        let (status, body) = emulator
            .call_target(
                &format!("{prefix}.DeleteRepositoryLink"),
                json!({ "RepositoryLinkId": id }),
            )
            .await;
        assert_empty_ok(case, status, &body);
    }
}
