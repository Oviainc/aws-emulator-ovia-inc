// SPDX-License-Identifier: CC0-1.0

//! The resource-policy slice — `PutResourcePolicy` and `GetResourcePolicy` — driven
//! over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! RULE: a resource-based policy is state keyed by the ARN of the project or report
//! group it is attached to. `PutResourcePolicy` stores the caller's document VERBATIM
//! against a resource this account holds, replacing whatever document that ARN already
//! carried (the document is the whole grant, so the last writer wins), and answers that
//! ARN; `GetResourcePolicy` hands the stored document back byte for byte and refuses an
//! ARN carrying no policy with `ResourceNotFoundException`; an ARN that names no
//! resource of this account is refused with `ResourceNotFoundException`, and a value
//! that is not the ARN of a resource a policy attaches to, or a policy that is not a
//! JSON document, with `InvalidInputException`; and the policy lives and dies with the
//! resource it is attached to.
//!
//! The harness (`call`, `create_project`, `emulator`, `emulator_on`, `error_type`,
//! `REGION`) is the one `project_build_lifecycle` already drives this service with, and
//! the report-group request builder is `report_group_lifecycle`'s.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{call, create_project, emulator, emulator_on, error_type, REGION};
use super::report_group_lifecycle::create_report_group_body;

/// The two resource kinds CodeBuild attaches a resource-based policy to —
/// `PutResourcePolicy` "Stores a resource policy for the ARN of a Project or
/// ReportGroup object"
/// (`smithy_query(query="operation",operation="PutResourcePolicy")` plus that
/// operation's doc), so these two are the whole class the rule generalises over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Project,
    ReportGroup,
}

const KINDS: [Kind; 2] = [Kind::Project, Kind::ReportGroup];

/// Create a resource of `kind` named `name` and hand back the ARN the service stored it
/// with, read from the service's own answer rather than assembled here.
async fn create_target(router: &MultiServiceRouter, kind: Kind, name: &str) -> String {
    match kind {
        Kind::Project => {
            let project = create_project(router, name, json!({})).await;
            project["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateProject {name:?} must report an ARN, got {project}"))
                .to_string()
        }
        Kind::ReportGroup => {
            let (status, body) = call(
                router,
                "CreateReportGroup",
                create_report_group_body(name, "TEST", json!({"exportConfigType": "NO_EXPORT"}), None),
            )
            .await;
            assert_eq!(
                status, 200,
                "CreateReportGroup {name:?}: expected 200, got {status} with {body}"
            );
            body["reportGroup"]["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateReportGroup {name:?} must report an ARN, got {body}"))
                .to_string()
        }
    }
}

/// Store `policy` against `arn`, asserting the call succeeded and answered that ARN.
async fn put(router: &MultiServiceRouter, case: &str, arn: &str, policy: &str) {
    let (status, body) = call(
        router,
        "PutResourcePolicy",
        json!({"resourceArn": arn, "policy": policy}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutResourcePolicy must succeed, got {status} with {body}"
    );
    assert_eq!(
        body["resourceArn"].as_str(),
        Some(arn),
        "case {case:?}: the answer states the ARN the policy was stored against, got {body}"
    );
}

/// Read the policy stored against `arn`, asserting the call succeeded, and hand back the
/// document it reported.
async fn get(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let (status, body) = call(router, "GetResourcePolicy", json!({"resourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetResourcePolicy must succeed, got {status} with {body}"
    );
    body["policy"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the answer states the policy document, got {body}"))
        .to_string()
}

/// Call `operation`, assert it was refused with HTTP 400 and `code`, and hand back the
/// message it stated.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    code: &str,
) -> String {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from {operation}, got {status} with {answer}"
    );
    assert_eq!(
        error_type(&answer),
        code,
        "case {case:?}: expected {code} from {operation}, got {answer}"
    );
    answer["message"].as_str().unwrap_or_default().to_string()
}

/// A sharing policy document whose `Sid` is `label` — one of the grants the CodeBuild
/// user guide's "Sharing a Project" describes. Built with `serde_json`, so the rows that
/// vary the LABEL vary only the document's content, not its shape.
fn sharing_policy(arn: &str, label: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": label,
            "Effect": "Allow",
            "Principal": {"AWS": "arn:aws:iam::222222222222:root"},
            "Action": ["codebuild:BatchGetProjects"],
            "Resource": arn,
        }],
    })
    .to_string()
}

/// The documents the round-trip rows store: every class of text a JSON policy document
/// can be, so the assertion is about the bytes rather than about one recorded policy.
/// The labels span ASCII, scripts outside the basic plane (CJK, Cyrillic, Greek), a
/// right-to-left script, combining marks, an astral-plane emoji, text that carries
/// `\u` ESCAPES rather than the characters themselves (which only survive a store that
/// does not re-serialize), and quoting that a re-serializer would normalise.
fn documents(arn: &str) -> Vec<(String, String)> {
    let labels = [
        ("ascii", "SharedWithPartner"),
        ("cjk", "共有プロジェクト-功能"),
        ("cyrillic and greek", "Общий-доступ-Δοκιμή"),
        ("right to left", "مشاركة-المشروع"),
        ("combining marks", "e\u{0301}te\u{0301}-partage\u{0301}"),
        ("astral plane", "share-🚀-🙂\u{1F1EF}\u{1F1F5}"),
        ("escaped scalars", r"share-\u00e9-\u4f53"),
        ("quotes and escapes", r#"she said \"share\" \\ it"#),
    ];
    let mut rows: Vec<(String, String)> = labels
        .into_iter()
        .map(|(case, label)| (case.to_string(), sharing_policy(arn, label)))
        .collect();
    // Formatting a re-serializer would normalise away: newlines, tabs and padding
    // between the members, and the members in an order no canonical form keeps.
    rows.push((
        "pretty printed with tabs".to_string(),
        format!(
            "{{\n\t\"Statement\" : [\n\t\t{{\n\t\t\t\"Resource\":   \"{arn}\",\n\t\t\t\
             \"Action\": \"codebuild:*\",\n\t\t\t\"Effect\" : \"Allow\",\n\t\t\t\
             \"Principal\": {{\"AWS\": \"*\"}}\n\t\t}}\n\t],\n\t\
             \"Version\":\"2012-10-17\"\n}}"
        ),
    ));
    // The smallest document that is still a JSON object, and one whose values are every
    // other JSON type.
    rows.push(("empty object".to_string(), "{}".to_string()));
    rows.push((
        "every json value type".to_string(),
        json!({
            "Version": "2012-10-17",
            "Statement": [],
            "numbers": [0, -1, 1.5e3],
            "flags": {"yes": true, "no": false},
            "nothing": Value::Null,
        })
        .to_string(),
    ));
    rows
}

/// RULE: a stored policy comes back byte for byte from the ARN it was stored against,
/// for either resource kind a policy attaches to, and the resource it is attached to is
/// what the account shares.
#[tokio::test]
async fn a_stored_policy_round_trips_byte_for_byte_for_either_resource_kind() {
    for kind in KINDS {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let arn = create_target(&router, kind, "policy-target").await;
        // A second resource of the same kind, never given a policy: the store is keyed
        // by ARN, so one resource's policy must never answer for another's.
        let untouched = create_target(&router, kind, "policy-untouched").await;

        for (case, document) in documents(&arn) {
            let case = format!("{kind:?}, {case}");
            put(&router, &case, &arn, &document).await;
            let read = get(&router, &case, &arn).await;
            assert_eq!(
                read, document,
                "case {case:?}: the stored document must come back verbatim, got {read:?}"
            );
            // The store holds exactly what the wire reported, so the read is computed
            // from state rather than from the request.
            let held = engine
                .state::<CodeBuildState>()
                .lock()
                .resource_policies
                .get(&arn)
                .map(|stored| stored.policy.clone());
            assert_eq!(
                held.as_deref(),
                Some(document.as_str()),
                "case {case:?}: the store must hold the document the write persisted"
            );

            // A second read answers the same thing: reading a policy changes nothing.
            let again = get(&router, &case, &arn).await;
            assert_eq!(
                again, read,
                "case {case:?}: a second read must answer the same document, got {again:?}"
            );

            let message = refusal(
                &router,
                &case,
                "GetResourcePolicy",
                json!({"resourceArn": untouched}),
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                message, "Resource policy does not exist",
                "case {case:?}: a sibling resource carries no policy, got {message:?}"
            );
        }

        // A project carrying a policy is what the account shares; a report group's
        // policy lists no project.
        let (status, listed) = call(&router, "ListSharedProjects", json!({})).await;
        assert_eq!(
            status, 200,
            "{kind:?}: ListSharedProjects must succeed, got {status} with {listed}"
        );
        let expected: Vec<Value> = match kind {
            Kind::Project => vec![json!(arn)],
            Kind::ReportGroup => vec![],
        };
        assert_eq!(
            listed["projects"].as_array(),
            Some(&expected),
            "{kind:?}: the shared listing must report exactly the projects carrying a \
             policy, got {listed}"
        );
    }
}

/// RULE: a policy document is the whole grant, so a later write REPLACES the one the ARN
/// already carried — the last writer wins and nothing of an earlier document survives.
#[tokio::test]
async fn a_later_write_replaces_the_document_the_arn_already_carried() {
    for kind in KINDS {
        let router = emulator();
        let arn = create_target(&router, kind, "overwritten").await;

        // The sequence spans a document growing, shrinking, changing only in one
        // character, repeating an earlier one, and ending at the smallest object — so no
        // row can pass by merging, appending to or remembering an earlier write.
        let sequence = [
            sharing_policy(&arn, "first"),
            sharing_policy(&arn, "second-🙂"),
            sharing_policy(&arn, "second-🙃"),
            json!({"Version": "2012-10-17", "Statement": []}).to_string(),
            sharing_policy(&arn, "first"),
            "{}".to_string(),
        ];

        for (index, document) in sequence.iter().enumerate() {
            let case = format!("{kind:?}, write {index}");
            put(&router, &case, &arn, document).await;
            let read = get(&router, &case, &arn).await;
            assert_eq!(
                &read, document,
                "case {case:?}: the newest document is the one in force, got {read:?}"
            );
        }

        // The ARN carries ONE policy, not a history of them.
        let (status, body) = call(&router, "GetResourcePolicy", json!({"resourceArn": arn})).await;
        assert_eq!(
            (status, body["policy"].as_str()),
            (200, Some("{}")),
            "{kind:?}: the last write is what the ARN carries, got {body}"
        );
    }
}

/// RULE: an ARN of the right shape that this account holds no resource for is refused
/// with `ResourceNotFoundException`, naming the resource kind, and nothing is stored.
#[tokio::test]
async fn an_arn_this_account_holds_no_resource_for_is_refused_as_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let project_arn = create_target(&router, Kind::Project, "held-project").await;
    let group_arn = create_target(&router, Kind::ReportGroup, "held-group").await;

    // The rows span a name never created, a held resource addressed in another region,
    // in another account and in another partition, and a name that differs from a held
    // one only in case — every way an ARN of the right SHAPE can still name nothing
    // here.
    let other_account = format!("arn:aws:codebuild:{REGION}:111122223333:project/held-project");
    let other_partition =
        format!("arn:aws-us-gov:codebuild:{REGION}:{}:project/held-project", engine::EMULATOR_ACCOUNT_ID);
    let cases: [(&str, String, &str); 7] = [
        (
            "a project never created",
            state::project_arn(REGION, "no-such-project"),
            "Project does not exist",
        ),
        (
            "a held project in another region",
            state::project_arn("eu-west-1", "held-project"),
            "Project does not exist",
        ),
        (
            "a held project in another account",
            other_account,
            "Project does not exist",
        ),
        (
            "a held project in another partition",
            other_partition,
            "Project does not exist",
        ),
        (
            "a held project in another case",
            state::project_arn(REGION, "HELD-PROJECT"),
            "Project does not exist",
        ),
        (
            "a report group never created",
            state::report_group_arn(REGION, "no-such-group"),
            "Report group does not exist",
        ),
        (
            "a held report group in another region",
            state::report_group_arn("eu-west-1", "held-group"),
            "Report group does not exist",
        ),
    ];

    for (case, arn, expected) in cases {
        let message = refusal(
            &router,
            case,
            "PutResourcePolicy",
            json!({"resourceArn": arn, "policy": sharing_policy(&arn, "grant")}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected CodeBuild's absent-resource sentence, got {message:?}"
        );
        // Reading it back reports the ABSENT POLICY rather than a stored one: the
        // refused write stored nothing.
        let message = refusal(
            &router,
            case,
            "GetResourcePolicy",
            json!({"resourceArn": arn}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, "Resource policy does not exist",
            "case {case:?}: a refused write must store nothing, got {message:?}"
        );
    }

    // The controls: the resources this account DOES hold take a policy.
    for arn in [project_arn, group_arn] {
        put(&router, "a held resource", &arn, &sharing_policy(&arn, "grant")).await;
    }
    assert_eq!(
        engine.state::<CodeBuildState>().lock().resource_policies.len(),
        2,
        "only the two held resources may carry a policy"
    );
}

/// RULE: a value that is not the ARN of a resource a policy attaches to, and a policy
/// that is not a JSON document, are refused with `InvalidInputException` — a malformed
/// request rather than a missing resource — and the sentence echoes nothing of the
/// request.
#[tokio::test]
async fn a_malformed_request_is_refused_as_invalid_input() {
    let router = emulator();
    let arn = create_target(&router, Kind::Project, "validated").await;

    // ARNs of CodeBuild resources a policy does NOT attach to, ARNs of other services,
    // and values that are not ARNs at all.
    let not_a_target = [
        ("a build ARN", state::build_arn(REGION, "validated:1111")),
        ("a fleet ARN", state::fleet_arn(REGION, "validated:1111")),
        ("a sandbox ARN", state::sandbox_arn(REGION, "validated:1111")),
        (
            "another service's ARN",
            format!("arn:aws:s3:::bucket/{}", "key"),
        ),
        (
            "a project ARN with an empty name",
            format!("arn:aws:codebuild:{REGION}:{}:project/", engine::EMULATOR_ACCOUNT_ID),
        ),
        (
            "an ARN with no partition",
            format!("arn::codebuild:{REGION}:{}:project/validated", engine::EMULATOR_ACCOUNT_ID),
        ),
        ("a bare project name", "validated".to_string()),
        ("text that is not an ARN", "not-an-arn".to_string()),
    ];

    for (case, value) in not_a_target {
        for operation in ["PutResourcePolicy", "GetResourcePolicy"] {
            let mut body = json!({"resourceArn": value});
            if operation == "PutResourcePolicy" {
                if let Value::Object(members) = &mut body {
                    members.insert("policy".to_string(), json!(sharing_policy(&arn, "grant")));
                }
            }
            let case = format!("{operation}, {case}");
            let message = refusal(&router, &case, operation, body, "InvalidInputException").await;
            assert_eq!(
                message, "Invalid resource ARN",
                "case {case:?}: expected CodeBuild's invalid-ARN sentence, got {message:?}"
            );
            assert!(
                !message.contains(&value),
                "case {case:?}: the sentence must not echo the value, got {message:?}"
            );
        }
    }

    // `policy` is documented as "A JSON-formatted resource policy", and a policy
    // document is a JSON object: every other form of text is not one.
    let not_a_document = [
        ("plain text", "this is not json".to_string()),
        ("a JSON array", json!([{"Effect": "Allow"}]).to_string()),
        ("a JSON string", json!("Allow").to_string()),
        ("a JSON number", "42".to_string()),
        ("a JSON boolean", "true".to_string()),
        ("JSON null", "null".to_string()),
        ("an unterminated object", r#"{"Version": "2012-10-17""#.to_string()),
        ("two objects", "{}{}".to_string()),
        ("a single space", " ".to_string()),
    ];

    for (case, policy) in not_a_document {
        let message = refusal(
            &router,
            case,
            "PutResourcePolicy",
            json!({"resourceArn": arn, "policy": policy}),
            "InvalidInputException",
        )
        .await;
        assert_eq!(
            message, "Invalid policy document",
            "case {case:?}: expected CodeBuild's invalid-document sentence, got {message:?}"
        );
        assert!(
            !message.contains(policy.trim()) || policy.trim().is_empty(),
            "case {case:?}: the sentence must not echo the document, got {message:?}"
        );
        // The refused write left the resource carrying no policy.
        let message = refusal(
            &router,
            case,
            "GetResourcePolicy",
            json!({"resourceArn": arn}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, "Resource policy does not exist",
            "case {case:?}: a refused write must store nothing, got {message:?}"
        );
    }

    // The control: the same ARN takes a document that IS one.
    put(&router, "a JSON object", &arn, &sharing_policy(&arn, "grant")).await;
}

/// RULE: a resource policy is meaningful only for a resource this account holds, so
/// deleting the resource drops the policy attached to it — and a resource of the same
/// name created afterwards carries no grant nobody wrote for it.
#[tokio::test]
async fn deleting_the_resource_drops_the_policy_attached_to_it() {
    // (the resource kind, the operation that deletes one, the member it addresses it by)
    let cases: [(Kind, &str, &str); 2] = [
        (Kind::Project, "DeleteProject", "name"),
        (Kind::ReportGroup, "DeleteReportGroup", "arn"),
    ];

    for (kind, operation, member) in cases {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let name = "deleted-target";
        let arn = create_target(&router, kind, name).await;
        let document = sharing_policy(&arn, "grant");
        put(&router, operation, &arn, &document).await;
        assert_eq!(
            get(&router, operation, &arn).await,
            document,
            "{operation}: the policy is in force before the delete"
        );

        let addressed = if member == "name" { name.to_string() } else { arn.clone() };
        let (status, body) = call(&router, operation, json!({member: addressed})).await;
        assert_eq!(
            status, 200,
            "{operation} must succeed, got {status} with {body}"
        );

        assert!(
            engine
                .state::<CodeBuildState>()
                .lock()
                .resource_policies
                .is_empty(),
            "{operation}: the policy must go with the resource it was attached to"
        );
        let message = refusal(
            &router,
            operation,
            "GetResourcePolicy",
            json!({"resourceArn": arn}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, "Resource policy does not exist",
            "{operation}: the deleted resource's policy must be gone, got {message:?}"
        );

        // A resource created again under the same name — so with the same ARN — starts
        // unshared.
        let again = create_target(&router, kind, name).await;
        assert_eq!(
            again, arn,
            "{operation}: recreating {name:?} must mint the same ARN"
        );
        let message = refusal(
            &router,
            operation,
            "GetResourcePolicy",
            json!({"resourceArn": again}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, "Resource policy does not exist",
            "{operation}: a recreated resource carries no earlier grant, got {message:?}"
        );
    }
}
