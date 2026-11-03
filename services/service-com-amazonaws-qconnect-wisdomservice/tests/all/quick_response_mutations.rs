// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MUTATIONS of the quick-response slice
//! (`UpdateQuickResponse` + `DeleteQuickResponse`) and for the author every projection of
//! a quick response reports (`lastModifiedBy`).
//!
//! Every assertion goes over the wire, through the same [`Emulator`] harness the tagging
//! suite drives the production router with: only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected, so a response that is not computed from
//! persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * an update of a stored resource writes the members the request supplies — and ONLY
//!   those, each `remove…` flag clearing the member it names — so every later read of the
//!   record, by any label form and through any projection of it, reports the updated
//!   values; a request that both sets and clears one member is refused and writes
//!   nothing, as is one that claims a value another live record of the same parent holds;
//! * a delete removes the record, so every later read of that label is the modelled
//!   `ResourceNotFoundException` (HTTP 404), the listing no longer carries it, and the
//!   values that identified it inside its parent are free again;
//! * every projection of a record reports the identity that last WROTE it, which a write
//!   stamps on the record — so the value survives a read and changes only when something
//!   writes.
//!
//! This model serves the shared `wisdom` endpoint identity (see `src/service.rs`), which is
//! what every request below is signed for.

use serde_json::{json, Value};

use super::resource_tags::{label, Emulator, Reply, ABSENT_UUID, REGION};

/// Both values the modelled `QuickResponseType` pattern admits
/// (`smithy_query(query="operation",operation="UpdateQuickResponse")`: `contentType`
/// matches `^(application/x\.quickresponse;format=(plain|markdown))$`).
pub(crate) const CONTENT_TYPE_PLAIN: &str = "application/x.quickresponse;format=plain";
const CONTENT_TYPE_MARKDOWN: &str = "application/x.quickresponse;format=markdown";

/// The `QuickResponseStatus` a stored quick response reports, which an accepted update
/// leaves where it was (`smithy_query(query="shape",shape="QuickResponseStatus")` declares
/// it beside the transient `CREATE_IN_PROGRESS`, `UPDATE_IN_PROGRESS` and the rest).
const STATUS_CREATED: &str = "CREATED";

/// The fixed sentences this endpoint answers a resource it does not hold with, written out
/// here rather than imported from the implementation: the one naming the quick response and
/// the one naming its parent knowledge base.
pub(crate) const MESSAGE_QUICK_RESPONSE_NOT_FOUND: &str = "QuickResponse does not exist.";
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";

/// The ARN `lastModifiedBy` carries: "The Amazon Resource Name (ARN) of the user who last
/// updated the quick response data" (`smithy_query(query="shape",shape="QuickResponseData")`
/// — a `GenericArn`, pattern
/// `^arn:[a-z-]+?:[a-z-]+?:[a-z0-9-]*?:([0-9]{12})?:[a-zA-Z0-9-:/]+$`).
///
/// The emulator publishes ONE long-term access key, and on AWS a long-term key belongs to an
/// IAM user of the account: that user — the identity `sts:GetCallerIdentity` answers with —
/// is the caller every write here is attributed to. Written out rather than taken from the
/// implementation's helper, so the assertion pins the value.
fn caller_principal_arn() -> String {
    format!("arn:aws:iam::{}:user/emulator", engine::EMULATOR_ACCOUNT_ID)
}

/// A knowledge base to hold quick responses, answering the id its create minted.
pub(crate) async fn knowledge_base(emulator: &Emulator, name: &str) -> String {
    let reply = emulator
        .send(
            "POST",
            "/knowledgeBases",
            Some(json!({ "name": name, "knowledgeBaseType": "QUICK_RESPONSES" })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: the knowledge base this suite nests under must be created, got HTTP \
         {} with body {}",
        reply.status, reply.body
    );
    reply.string("/knowledgeBase/knowledgeBaseId")
}

pub(crate) async fn create_quick_response(
    emulator: &Emulator,
    knowledge_base: &str,
    body: Value,
) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/knowledgeBases/{}/quickResponses", label(knowledge_base)),
            Some(body),
        )
        .await
}

pub(crate) async fn get_quick_response(
    emulator: &Emulator,
    knowledge_base: &str,
    quick_response: &str,
) -> Reply {
    emulator
        .send(
            "GET",
            &format!(
                "/knowledgeBases/{}/quickResponses/{}",
                label(knowledge_base),
                label(quick_response)
            ),
            None,
        )
        .await
}

async fn update_quick_response(
    emulator: &Emulator,
    knowledge_base: &str,
    quick_response: &str,
    body: Value,
) -> Reply {
    emulator
        .send(
            "POST",
            &format!(
                "/knowledgeBases/{}/quickResponses/{}",
                label(knowledge_base),
                label(quick_response)
            ),
            Some(body),
        )
        .await
}

pub(crate) async fn delete_quick_response(
    emulator: &Emulator,
    knowledge_base: &str,
    quick_response: &str,
) -> Reply {
    emulator
        .send(
            "DELETE",
            &format!(
                "/knowledgeBases/{}/quickResponses/{}",
                label(knowledge_base),
                label(quick_response)
            ),
            None,
        )
        .await
}

pub(crate) async fn list_quick_responses(emulator: &Emulator, knowledge_base: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/knowledgeBases/{}/quickResponses", label(knowledge_base)),
            None,
        )
        .await
}

/// A create body with every optional member the operation accepts populated, so an update
/// row can be read as "what this request changed" against a record that had everything.
fn baseline_body(name: &str, shortcut_key: &str) -> Value {
    json!({
        "name": name,
        "content": { "content": "The baseline reply" },
        "contentType": CONTENT_TYPE_PLAIN,
        "description": "the baseline description",
        "shortcutKey": shortcut_key,
        "isActive": true,
        "channels": ["Chat"],
        "language": "en_US",
        "groupingConfiguration": {
            "criteria": "RoutingProfileArn",
            "values": ["sales"],
        },
        "tags": { "env": "test" },
    })
}

/// One row of the update table: the request body, and the members of the stored record it
/// must leave DIFFERENT (`Some(value)`) or ABSENT (`None`). Every member not named here
/// must come back exactly as the create left it.
struct UpdateCase {
    label: &'static str,
    update: Value,
    expected: Vec<(&'static str, Option<Value>)>,
}

/// The table: one row per axis the rule generalises over — which member is set, which is
/// cleared, a flag that asks for no clear, a request that names nothing, and every member
/// at once.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "deactivated and re-described",
            update: json!({ "isActive": false, "description": "Now inactive" }),
            expected: vec![
                ("isActive", Some(json!(false))),
                ("description", Some(json!("Now inactive"))),
            ],
        },
        UpdateCase {
            label: "reactivated, which the record already was",
            update: json!({ "isActive": true }),
            expected: Vec::new(),
        },
        UpdateCase {
            label: "renamed",
            update: json!({ "name": "renamed-reply" }),
            expected: vec![("name", Some(json!("renamed-reply")))],
        },
        UpdateCase {
            label: "new plain-text content",
            update: json!({ "content": { "content": "An updated reply" } }),
            expected: vec![(
                "contents",
                Some(json!({ "plainText": { "content": "An updated reply" } })),
            )],
        },
        UpdateCase {
            label: "the media type switched to markdown, carrying the stored content over",
            update: json!({ "contentType": CONTENT_TYPE_MARKDOWN }),
            expected: vec![
                ("contentType", Some(json!(CONTENT_TYPE_MARKDOWN))),
                (
                    "contents",
                    Some(json!({ "markdown": { "content": "The baseline reply" } })),
                ),
            ],
        },
        UpdateCase {
            label: "new markdown content and its media type together",
            update: json!({
                "contentType": CONTENT_TYPE_MARKDOWN,
                "content": { "content": "# Heading\n\nthe rich reply" },
            }),
            expected: vec![
                ("contentType", Some(json!(CONTENT_TYPE_MARKDOWN))),
                (
                    "contents",
                    Some(json!({ "markdown": { "content": "# Heading\n\nthe rich reply" } })),
                ),
            ],
        },
        UpdateCase {
            label: "the description removed",
            update: json!({ "removeDescription": true }),
            expected: vec![("description", None)],
        },
        UpdateCase {
            label: "the shortcut key removed",
            update: json!({ "removeShortcutKey": true }),
            expected: vec![("shortcutKey", None)],
        },
        UpdateCase {
            label: "the grouping configuration removed",
            update: json!({ "removeGroupingConfiguration": true }),
            expected: vec![("groupingConfiguration", None)],
        },
        UpdateCase {
            label: "every remove flag asking for no removal",
            update: json!({
                "removeDescription": false,
                "removeShortcutKey": false,
                "removeGroupingConfiguration": false,
            }),
            expected: Vec::new(),
        },
        UpdateCase {
            label: "a new shortcut key",
            update: json!({ "shortcutKey": "/updated" }),
            expected: vec![("shortcutKey", Some(json!("/updated")))],
        },
        UpdateCase {
            label: "more channels",
            update: json!({ "channels": ["Chat", "Email"] }),
            expected: vec![("channels", Some(json!(["Chat", "Email"])))],
        },
        UpdateCase {
            label: "another documented language code",
            update: json!({ "language": "ja_JP" }),
            expected: vec![("language", Some(json!("ja_JP")))],
        },
        UpdateCase {
            label: "a new grouping configuration",
            update: json!({
                "groupingConfiguration": {
                    "criteria": "RoutingProfileArn",
                    "values": ["support", "sales"],
                },
            }),
            expected: vec![(
                "groupingConfiguration",
                Some(json!({
                    "criteria": "RoutingProfileArn",
                    "values": ["support", "sales"],
                })),
            )],
        },
        UpdateCase {
            label: "an update naming no member at all",
            update: json!({}),
            expected: Vec::new(),
        },
        UpdateCase {
            label: "every updatable member at once",
            update: json!({
                "name": "wholly-updated",
                "content": { "content": "# Everything" },
                "contentType": CONTENT_TYPE_MARKDOWN,
                "description": "a wholly new description",
                "shortcutKey": "/all",
                "isActive": false,
                "channels": ["Email"],
                "language": "pt_BR",
                "groupingConfiguration": {
                    "criteria": "RoutingProfileArn",
                    "values": ["escalations"],
                },
            }),
            expected: vec![
                ("name", Some(json!("wholly-updated"))),
                (
                    "contents",
                    Some(json!({ "markdown": { "content": "# Everything" } })),
                ),
                ("contentType", Some(json!(CONTENT_TYPE_MARKDOWN))),
                ("description", Some(json!("a wholly new description"))),
                ("shortcutKey", Some(json!("/all"))),
                ("isActive", Some(json!(false))),
                ("channels", Some(json!(["Email"]))),
                ("language", Some(json!("pt_BR"))),
                (
                    "groupingConfiguration",
                    Some(json!({
                        "criteria": "RoutingProfileArn",
                        "values": ["escalations"],
                    })),
                ),
            ],
        },
    ]
}

/// The record `before` with a row's expected changes applied: the shape every read after
/// the update must report, apart from `lastModifiedTime` (asserted on its own, since it
/// moves with the write rather than with the request).
fn expected_record(before: &Value, case: &UpdateCase) -> Value {
    let mut expected = before.clone();
    let members = expected
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {:?}: the created record is not an object", case.label));
    for (member, value) in &case.expected {
        match value {
            Some(value) => {
                members.insert((*member).to_string(), value.clone());
            }
            None => {
                members.remove(*member);
            }
        }
    }
    expected
}

/// One reported record without its `lastModifiedTime`, for the whole-record comparison.
fn without_last_modified_time(record: &Value) -> Value {
    let mut copy = record.clone();
    if let Some(members) = copy.as_object_mut() {
        members.remove("lastModifiedTime");
    }
    copy
}

/// The members the `QuickResponseSummary` shape declares
/// (`smithy_query(query="shape",shape="QuickResponseSummary")`), which a listing must report
/// with the same values the full record carries.
const SUMMARY_MEMBERS: [&str; 13] = [
    "quickResponseArn",
    "quickResponseId",
    "knowledgeBaseArn",
    "knowledgeBaseId",
    "name",
    "contentType",
    "status",
    "createdTime",
    "lastModifiedTime",
    "description",
    "lastModifiedBy",
    "isActive",
    "channels",
];

/// The listing entry for one `quickResponseId`, or `None` when the listing does not carry
/// it.
fn listed(listing: &Reply, quick_response_id: &str) -> Option<Value> {
    listing
        .field("/quickResponseSummaries")?
        .as_array()?
        .iter()
        .find(|summary| {
            summary.get("quickResponseId").and_then(Value::as_str) == Some(quick_response_id)
        })
        .cloned()
}

#[tokio::test]
async fn an_update_writes_the_members_it_supplies_and_only_those() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "quick-response-updates").await;

    for (index, case) in update_cases().into_iter().enumerate() {
        let label = case.label;
        // One record per row, so no row reads another's state.
        let created = create_quick_response(
            &emulator,
            &knowledge_base_id,
            baseline_body(&format!("baseline-{index}"), &format!("/b{index}")),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the baseline create must succeed, got HTTP {} with body {}",
            created.status, created.body
        );
        let before = created
            .field("/quickResponse")
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: the create answered no quickResponse"));
        let quick_response_id = created.string("/quickResponse/quickResponseId");
        let quick_response_arn = created.string("/quickResponse/quickResponseArn");
        assert_eq!(
            created.string("/quickResponse/lastModifiedBy"),
            caller_principal_arn(),
            "case {label:?}: a create stamps the calling identity, got {:?}",
            created.field("/quickResponse/lastModifiedBy")
        );
        let created_time = created
            .field("/quickResponse/createdTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: the create answered no createdTime"));

        let expected = expected_record(&before, &case);
        let updated = update_quick_response(
            &emulator,
            &knowledge_base_id,
            &quick_response_id,
            case.update.clone(),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from the update, got {} with body {}",
            updated.status, updated.body
        );

        // The update's own answer, then the same record read back by id, by its own ARN
        // and through the parent's ARN: a label form changes how a record is addressed,
        // never what it is.
        let knowledge_base_arn = created.string("/quickResponse/knowledgeBaseArn");
        let mut reports = vec![("the update's answer".to_string(), updated)];
        for (source, knowledge_base_label, quick_response_label) in [
            (
                "read by id",
                knowledge_base_id.as_str(),
                quick_response_id.as_str(),
            ),
            (
                "read by arn",
                knowledge_base_id.as_str(),
                quick_response_arn.as_str(),
            ),
            (
                "read through the parent's arn",
                knowledge_base_arn.as_str(),
                quick_response_id.as_str(),
            ),
        ] {
            reports.push((
                source.to_string(),
                get_quick_response(&emulator, knowledge_base_label, quick_response_label).await,
            ));
        }
        for (source, reply) in &reports {
            assert_eq!(
                reply.status, 200,
                "case {label:?} ({source}): expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            let record = reply.field("/quickResponse").cloned().unwrap_or_else(|| {
                panic!("case {label:?} ({source}): no quickResponse: {reply:?}")
            });
            assert_eq!(
                without_last_modified_time(&record),
                without_last_modified_time(&expected),
                "case {label:?} ({source}): expected the stored record to carry exactly the \
                 update's changes, got {record}"
            );
            let modified = record
                .get("lastModifiedTime")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            assert!(
                modified >= created_time,
                "case {label:?} ({source}): lastModifiedTime must not precede createdTime, got \
                 {modified} and {created_time}"
            );
            assert_eq!(
                record.get("status").and_then(Value::as_str),
                Some(STATUS_CREATED),
                "case {label:?} ({source}): an accepted update leaves the status where it was, \
                 got {record}"
            );
        }

        // The listing reports the SAME record, reduced to the members its summary shape
        // declares — so a list page cannot disagree with a read.
        let listing = list_quick_responses(&emulator, &knowledge_base_id).await;
        let summary = listed(&listing, &quick_response_id).unwrap_or_else(|| {
            panic!(
                "case {label:?}: the listing does not carry the record: {}",
                listing.body
            )
        });
        let read = reports
            .last()
            .map(|(_, reply)| reply)
            .and_then(|reply| reply.field("/quickResponse").cloned())
            .unwrap_or_default();
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                summary.get(member),
                read.get(member),
                "case {label:?}: the summary's {member} must be the stored record's, got \
                 {summary} against {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_update_writes_nothing() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "quick-response-update-refusals").await;
    let other_knowledge_base_id =
        knowledge_base(&emulator, "quick-response-update-refusals-other").await;

    // The record every row below tries to change, and a sibling whose name and shortcut
    // key are already taken.
    let target = create_quick_response(
        &emulator,
        &knowledge_base_id,
        baseline_body("update-target", "/t"),
    )
    .await;
    let target_id = target.string("/quickResponse/quickResponseId");
    let sibling = create_quick_response(
        &emulator,
        &knowledge_base_id,
        baseline_body("update-sibling", "/s"),
    )
    .await;
    assert_eq!(
        sibling.status, 200,
        "case setup: the sibling create must succeed, got HTTP {} with body {}",
        sibling.status, sibling.body
    );
    // A quick response of ANOTHER knowledge base, which this knowledge base's route does
    // not reach.
    let foreign = create_quick_response(
        &emulator,
        &other_knowledge_base_id,
        baseline_body("update-foreign", "/f"),
    )
    .await;
    let foreign_id = foreign.string("/quickResponse/quickResponseId");
    let before = target
        .field("/quickResponse")
        .cloned()
        .expect("case setup: the target create answered no quickResponse");

    // Rows: which label pair is refused, and which request body is. Each names the status,
    // the error shape and the sentence this endpoint words that class with.
    let rows: Vec<(&str, String, String, Value, u16, &str, String)> = vec![
        (
            "a quickResponseId nothing created",
            knowledge_base_id.clone(),
            ABSENT_UUID.to_string(),
            json!({ "isActive": false }),
            404,
            "ResourceNotFoundException",
            MESSAGE_QUICK_RESPONSE_NOT_FOUND.to_string(),
        ),
        (
            "another knowledge base's quick response",
            knowledge_base_id.clone(),
            foreign_id.clone(),
            json!({ "isActive": false }),
            404,
            "ResourceNotFoundException",
            MESSAGE_QUICK_RESPONSE_NOT_FOUND.to_string(),
        ),
        (
            "a knowledgeBaseId nothing created",
            ABSENT_UUID.to_string(),
            target_id.clone(),
            json!({ "isActive": false }),
            404,
            "ResourceNotFoundException",
            MESSAGE_KNOWLEDGE_BASE_NOT_FOUND.to_string(),
        ),
        (
            "a name a live sibling already holds",
            knowledge_base_id.clone(),
            target_id.clone(),
            json!({ "name": "update-sibling" }),
            409,
            "ConflictException",
            "The value for name is already in use. Please update and try again.".to_string(),
        ),
        (
            "a shortcut key a live sibling already holds",
            knowledge_base_id.clone(),
            target_id.clone(),
            json!({ "shortcutKey": "/s" }),
            409,
            "ConflictException",
            "The value for shortcut key is already in use. Please update and try again."
                .to_string(),
        ),
        (
            "a description both set and removed",
            knowledge_base_id.clone(),
            target_id.clone(),
            json!({ "description": "two minds", "removeDescription": true }),
            400,
            "ValidationException",
            "Cannot specify both description and removeDescription.".to_string(),
        ),
        (
            "a shortcut key both set and removed",
            knowledge_base_id.clone(),
            target_id.clone(),
            json!({ "shortcutKey": "/two", "removeShortcutKey": true }),
            400,
            "ValidationException",
            "Cannot specify both shortcutKey and removeShortcutKey.".to_string(),
        ),
        (
            "a grouping configuration both set and removed",
            knowledge_base_id.clone(),
            target_id.clone(),
            json!({
                "groupingConfiguration": { "criteria": "RoutingProfileArn", "values": ["x"] },
                "removeGroupingConfiguration": true,
            }),
            400,
            "ValidationException",
            "Cannot specify both groupingConfiguration and removeGroupingConfiguration."
                .to_string(),
        ),
    ];

    for (case, knowledge_base_label, quick_response_label, body, status, error, message) in rows {
        let reply =
            update_quick_response(&emulator, &knowledge_base_label, &quick_response_label, body)
                .await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains(error),
            "case {case:?}: expected {error}, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message"),
            message,
            "case {case:?}: expected the sentence this class is worded with, got {}",
            reply.body
        );
        // A refusal leaves the stored record exactly as it was.
        let read = get_quick_response(&emulator, &knowledge_base_id, &target_id).await;
        assert_eq!(
            read.field("/quickResponse"),
            Some(&before),
            "case {case:?}: a refused update must write nothing, got {}",
            read.body
        );
    }

    // Restating the record's OWN name and shortcut key is not a conflict with itself.
    let restated = update_quick_response(
        &emulator,
        &knowledge_base_id,
        &target_id,
        json!({ "name": "update-target", "shortcutKey": "/t" }),
    )
    .await;
    assert_eq!(
        restated.status, 200,
        "case \"restating the record's own name and shortcut key\": expected HTTP 200, got {} \
         with body {}",
        restated.status, restated.body
    );
    assert_eq!(
        restated.string("/quickResponse/name"),
        "update-target",
        "case \"restating the record's own name and shortcut key\": expected the name kept, got \
         {}",
        restated.body
    );
}

#[tokio::test]
async fn a_delete_removes_the_record_and_releases_what_identified_it() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "quick-response-deletes").await;
    let knowledge_base_arn = {
        let probe = create_quick_response(
            &emulator,
            &knowledge_base_id,
            baseline_body("arn-probe", "/probe"),
        )
        .await;
        probe.string("/quickResponse/knowledgeBaseArn")
    };

    // Rows: the label FORM the delete and the following reads address the record by. The
    // resource a label names does not depend on how it is spelled.
    for (index, case) in ["by id", "by its own arn", "through the parent's arn"]
        .into_iter()
        .enumerate()
    {
        let name = format!("deleted-{index}");
        let shortcut_key = format!("/d{index}");
        let created = create_quick_response(
            &emulator,
            &knowledge_base_id,
            baseline_body(&name, &shortcut_key),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must succeed, got HTTP {} with body {}",
            created.status, created.body
        );
        let quick_response_id = created.string("/quickResponse/quickResponseId");
        let quick_response_arn = created.string("/quickResponse/quickResponseArn");
        let (knowledge_base_label, quick_response_label) = match case {
            "by id" => (knowledge_base_id.clone(), quick_response_id.clone()),
            "by its own arn" => (knowledge_base_id.clone(), quick_response_arn.clone()),
            _ => (knowledge_base_arn.clone(), quick_response_id.clone()),
        };

        let deleted =
            delete_quick_response(&emulator, &knowledge_base_label, &quick_response_label).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 from the delete, got {} with body {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: the delete's output declares no member, so its body carries none, \
             got {}",
            deleted.body
        );

        // Every later read of that record — by either label form — is the modelled
        // not-found.
        for (source, read_label) in [
            ("read by id", quick_response_id.clone()),
            ("read by arn", quick_response_arn.clone()),
        ] {
            let read = get_quick_response(&emulator, &knowledge_base_id, &read_label).await;
            assert_eq!(
                read.status, 404,
                "case {case:?} ({source}): expected HTTP 404 after the delete, got {} with body \
                 {}",
                read.status, read.body
            );
            assert!(
                read.error_type.contains("ResourceNotFoundException"),
                "case {case:?} ({source}): expected ResourceNotFoundException, got {:?} with \
                 body {}",
                read.error_type,
                read.body
            );
            assert_eq!(
                read.string("/message"),
                MESSAGE_QUICK_RESPONSE_NOT_FOUND,
                "case {case:?} ({source}): expected the sentence this class is worded with, got \
                 {}",
                read.body
            );
            assert_eq!(
                read.string("/resourceName"),
                read_label,
                "case {case:?} ({source}): the refusal names the label the request used, got {}",
                read.body
            );
        }
        // An update of a record that is gone writes nothing and is the same not-found.
        let updated = update_quick_response(
            &emulator,
            &knowledge_base_id,
            &quick_response_id,
            json!({ "isActive": false }),
        )
        .await;
        assert_eq!(
            updated.status, 404,
            "case {case:?}: expected HTTP 404 from an update after the delete, got {} with body \
             {}",
            updated.status, updated.body
        );
        // So is a second delete of the same label: the record is gone, not merely marked.
        let again = delete_quick_response(&emulator, &knowledge_base_id, &quick_response_id).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: expected HTTP 404 from a second delete, got {} with body {}",
            again.status, again.body
        );

        // The listing no longer carries it.
        let listing = list_quick_responses(&emulator, &knowledge_base_id).await;
        assert!(
            listed(&listing, &quick_response_id).is_none(),
            "case {case:?}: the listing still carries the deleted record: {}",
            listing.body
        );

        // The name and the shortcut key it held identified a LIVE record, so a create may
        // take both again.
        let remade = create_quick_response(
            &emulator,
            &knowledge_base_id,
            baseline_body(&name, &shortcut_key),
        )
        .await;
        assert_eq!(
            remade.status, 200,
            "case {case:?}: the deleted record's name and shortcut key must be free again, got \
             HTTP {} with body {}",
            remade.status, remade.body
        );
        assert_ne!(
            remade.string("/quickResponse/quickResponseId"),
            quick_response_id,
            "case {case:?}: the re-create must mint a new id, got {}",
            remade.body
        );
    }
}

#[tokio::test]
async fn a_delete_that_names_no_record_of_the_knowledge_base_leaves_everything_alone() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "quick-response-delete-refusals").await;
    let other_knowledge_base_id =
        knowledge_base(&emulator, "quick-response-delete-refusals-other").await;
    let kept = create_quick_response(
        &emulator,
        &other_knowledge_base_id,
        baseline_body("kept-elsewhere", "/k"),
    )
    .await;
    let kept_id = kept.string("/quickResponse/quickResponseId");
    let before = kept
        .field("/quickResponse")
        .cloned()
        .expect("case setup: the create answered no quickResponse");

    // Rows: a label pair that names nothing this knowledge base holds, and the resource
    // each refusal names. The last row is an ARN of a region nothing was created in, which
    // this emulator therefore never minted.
    for (case, knowledge_base_label, quick_response_label, message) in [
        (
            "a quickResponseId nothing created",
            knowledge_base_id.clone(),
            ABSENT_UUID.to_string(),
            MESSAGE_QUICK_RESPONSE_NOT_FOUND,
        ),
        (
            "another knowledge base's quick response",
            knowledge_base_id.clone(),
            kept_id.clone(),
            MESSAGE_QUICK_RESPONSE_NOT_FOUND,
        ),
        (
            "a knowledgeBaseId nothing created",
            ABSENT_UUID.to_string(),
            kept_id.clone(),
            MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
        ),
        (
            "a quick-response ARN of another region",
            knowledge_base_id.clone(),
            format!(
                "arn:aws:wisdom:us-east-2:{}:quick-response/{knowledge_base_id}/{ABSENT_UUID}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            MESSAGE_QUICK_RESPONSE_NOT_FOUND,
        ),
    ] {
        let reply =
            delete_quick_response(&emulator, &knowledge_base_label, &quick_response_label).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message"),
            message,
            "case {case:?}: expected the sentence this class is worded with, got {}",
            reply.body
        );
        // The record that WAS stored is untouched by a refusal aimed at it.
        let read = get_quick_response(&emulator, &other_knowledge_base_id, &kept_id).await;
        assert_eq!(
            read.field("/quickResponse"),
            Some(&before),
            "case {case:?}: a refused delete must leave the record alone, got {}",
            read.body
        );
    }

    // The ARNs this suite mints name the region its requests are signed for, which is what
    // makes the row above an ARN this emulator never minted.
    assert!(
        before
            .get("quickResponseArn")
            .and_then(Value::as_str)
            .is_some_and(|arn| arn.contains(REGION)),
        "case \"the signed region\": expected the ARN of {REGION}, got {before}"
    );
}
