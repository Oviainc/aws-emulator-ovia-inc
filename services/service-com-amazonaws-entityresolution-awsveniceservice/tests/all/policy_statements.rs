// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Entity Resolution policy STATEMENT loop:
//! `AddPolicyStatement` and `DeletePolicyStatement`, read back through `GetPolicy` and
//! composed on top of what `PutPolicy` wrote (the sibling `resource_policy` suite owns
//! those two).
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the `schema_mapping_workflow_lifecycle` suite owns — so what is asserted is
//! what an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: `AddPolicyStatement` PERSISTS the statement it composes from
//! the request into the `Statement` list of the policy standing on the resource its
//! `arn` names, leaves every other member of that document untouched, and answers the
//! resulting document with a fresh `token` for the revision the add produced;
//! `GetPolicy` serves that same document and token back. `statementId` is the
//! statement's `Sid`, so an add naming one the document already holds is refused and
//! changes nothing. `DeletePolicyStatement` removes the statement its `statementId`
//! names and answers what is left as another revision; removing nothing is no revision,
//! so the standing token stands and a repeated delete is idempotent, and removing the
//! last statement deletes the policy. A resource that does not exist, or that carries
//! no policy, has nothing to change.
//!
//! Each table row differs along what the rule generalises over rather than repeating
//! one recorded request: the resource type over the whole catalogue the modelled
//! `VeniceGlobalArn` admits, `effect` over the whole `StatementEffect` catalogue, the
//! number of actions and principals over the modelled `length` minimum and above it,
//! each principal over BOTH branches of the modelled `StatementPrincipal` alternation
//! (a 12-digit account id and a dotted service name) and a mix of the two,
//! `statementId` over the character classes its pattern admits at both length bounds,
//! and `condition` over absent, one condition key and several.

use serde_json::{json, Map, Value};

use crate::schema_mapping_workflow_lifecycle::{
    account_id, assert_error_shape, expected_arn, glue_table_arn, role_arn, string_member,
    Emulator, Reply, REGION,
};

/// The policy-language `Version` an IAM policy document carries, which is what a
/// document this service COMPOSES starts from.
const POLICY_VERSION: &str = "2012-10-17";

/// The modelled `PolicyDocument` maximum (`length 1..40960`,
/// `smithy_query(query="shape",shape="PolicyDocument")`), which is the most one policy
/// can grow to.
const MAX_POLICY_DOCUMENT: usize = 40960;

/// The `ResourceNotFoundException$message` AWS answers for a resource that carries no
/// policy — recorded on a `GetPolicy` whose policy had just been emptied.
const NO_POLICY_MESSAGE: &str = "The resource specified in the request does not exist.";

/// The `ValidationException$message` AWS answers for a `statementId` the document
/// already holds — recorded.
const DUPLICATE_STATEMENT_MESSAGE: &str = "Statement ID already exists in document";

/// A second account's id, so a row can mix the two principal forms.
const OTHER_ACCOUNT_ID: &str = "123456789012";

/// The resource types a `VeniceGlobalArn` can name
/// (`smithy_query(query="operation",operation="AddPolicyStatement")`): one policy API
/// serves all four, so the table below spans them.
const RESOURCE_TYPES: &[&str] = &[
    "schemamapping",
    "matchingworkflow",
    "idmappingworkflow",
    "idnamespace",
];

/// Percent-encode one `@httpLabel`: a label is ONE path segment, so the `:` and `/` of
/// an ARN arrive encoded, exactly as an SDK sends them.
fn label(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// `POST /policies/{arn}/{statementId}` — `AddPolicyStatement`'s modelled HTTP binding.
async fn add_policy_statement(
    emulator: &Emulator,
    arn: &str,
    statement_id: &str,
    request: &Value,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/policies/{}/{}", label(arn), label(statement_id)),
            Some(request),
        )
        .await
}

/// `DELETE /policies/{arn}/{statementId}` — `DeletePolicyStatement`'s binding.
async fn delete_policy_statement(emulator: &Emulator, arn: &str, statement_id: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/policies/{}/{}", label(arn), label(statement_id)),
            None,
        )
        .await
}

/// `GET /policies/{arn}` — `GetPolicy`'s binding.
async fn get_policy(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/policies/{}", label(arn)), None)
        .await
}

/// `PUT /policies/{arn}` — `PutPolicy`'s binding, used here to write a whole document
/// that an add then has to preserve the rest of.
async fn put_policy(emulator: &Emulator, arn: &str, document: &str) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/policies/{}", label(arn)),
            Some(&json!({ "policy": document })),
        )
        .await
}

/// Create the resource `resource_type`/`name` through its own create operation, and
/// answer the ARN the create minted.
async fn create_resource(emulator: &Emulator, resource_type: &str, name: &str) -> String {
    let schema = format!("{name}-schema");
    let created = match resource_type {
        "schemamapping" => {
            emulator
                .create_schema_mapping(&json!({
                    "schemaName": name,
                    "mappedInputFields": [
                        { "fieldName": "id", "type": "UNIQUE_ID" },
                        { "fieldName": "email", "type": "EMAIL_ADDRESS" },
                    ],
                }))
                .await
        }
        "matchingworkflow" => {
            seed_schema(emulator, &schema).await;
            emulator
                .create_matching_workflow(&json!({
                    "workflowName": name,
                    "inputSourceConfig": [
                        { "inputSourceARN": glue_table_arn(name), "schemaName": schema },
                    ],
                    "outputSourceConfig": [{
                        "outputS3Path": "s3://er-output/statements",
                        "output": [{ "name": "id" }],
                    }],
                    "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                    "roleArn": role_arn(),
                }))
                .await
        }
        "idmappingworkflow" => {
            seed_schema(emulator, &schema).await;
            emulator
                .call(
                    "POST",
                    "/idmappingworkflows",
                    Some(&json!({
                        "workflowName": name,
                        "inputSourceConfig": [{
                            "inputSourceARN": glue_table_arn(name),
                            "schemaName": schema,
                            "type": "SOURCE",
                        }],
                        "idMappingTechniques": {
                            "idMappingType": "RULE_BASED",
                            "ruleBasedProperties": {
                                "ruleDefinitionType": "SOURCE",
                                "attributeMatchingModel": "MANY_TO_MANY",
                                "recordMatchingModel": "ONE_SOURCE_TO_ONE_TARGET",
                                "rules": [{ "ruleName": "by email", "matchingKeys": ["email"] }],
                            },
                        },
                        "roleArn": role_arn(),
                    })),
                )
                .await
        }
        "idnamespace" => {
            emulator
                .create_id_namespace(&json!({ "idNamespaceName": name, "type": "SOURCE" }))
                .await
        }
        other => panic!("no create is known for the resource type {other:?}"),
    };
    assert_eq!(
        created.status, 200,
        "creating the {resource_type} {name:?} failed: {created:?}"
    );
    expected_arn(resource_type, name)
}

/// The schema mapping a workflow row references. Idempotent: a conflict on the SCHEMA
/// is not what any test here is about.
async fn seed_schema(emulator: &Emulator, schema: &str) {
    let reply = emulator
        .create_schema_mapping(&json!({
            "schemaName": schema,
            "mappedInputFields": [
                { "fieldName": "id", "type": "UNIQUE_ID" },
                { "fieldName": "email", "type": "EMAIL_ADDRESS" },
            ],
        }))
        .await;
    assert!(
        reply.status == 200 || reply.error_code.as_deref() == Some("ConflictException"),
        "seeding the schema mapping {schema:?} failed: {reply:?}"
    );
}

/// One `AddPolicyStatement` request, held as its PIECES rather than as a literal body:
/// the request and the statement the assertions expect are built from the same values,
/// so the two cannot drift.
struct StatementCase {
    case: &'static str,
    /// `StatementId`, modelled `length 1..64`, pattern `^[0-9A-Za-z]+$`.
    statement_id: String,
    /// `StatementEffect`: `Allow` | `Deny`
    /// (`smithy_query(query="shape",shape="StatementEffect")`).
    effect: &'static str,
    /// `StatementActionList`, modelled `length 1..`, each member
    /// `^(entityresolution:[a-zA-Z0-9]+)$`, `length 3..64`.
    action: Vec<&'static str>,
    /// `StatementPrincipalList`, modelled `length 1..`, each member
    /// `^((\d{12})|([a-z0-9\.]+))$`, `length 12..64`.
    principal: Vec<String>,
    /// The JSON text the request sends for `condition` (a `StatementCondition` is a
    /// plain string on the wire), or none at all.
    condition: Option<String>,
    /// The principals that must be written under `Principal.AWS` — the 12-digit
    /// account-id branch of the modelled alternation.
    expected_aws: Vec<String>,
    /// The principals that must be written under `Principal.Service` — the dotted
    /// service-name branch of it.
    expected_service: Vec<String>,
}

impl StatementCase {
    /// The request body, assembled so an absent `condition` really is absent.
    fn request(&self) -> Value {
        let mut request = Map::new();
        request.insert("effect".to_string(), json!(self.effect));
        request.insert("action".to_string(), json!(self.action));
        request.insert("principal".to_string(), json!(self.principal));
        if let Some(condition) = &self.condition {
            request.insert("condition".to_string(), json!(condition));
        }
        Value::Object(request)
    }

    /// The statement this case must appear as inside the policy document of
    /// `resource_arn`.
    fn expected_statement(&self, resource_arn: &str) -> Value {
        let mut principal = Map::new();
        if !self.expected_aws.is_empty() {
            principal.insert("AWS".to_string(), json!(self.expected_aws));
        }
        if !self.expected_service.is_empty() {
            principal.insert("Service".to_string(), json!(self.expected_service));
        }

        let mut statement = Map::new();
        statement.insert("Sid".to_string(), json!(self.statement_id));
        statement.insert("Effect".to_string(), json!(self.effect));
        statement.insert("Principal".to_string(), Value::Object(principal));
        statement.insert("Action".to_string(), json!(self.action));
        // A statement added through this API governs the policy's OWN resource, so
        // `Resource` is that ARN and never a value the request carried.
        statement.insert("Resource".to_string(), json!(resource_arn));
        if let Some(condition) = &self.condition {
            statement.insert(
                "Condition".to_string(),
                serde_json::from_str(condition).expect("this case's condition is JSON"),
            );
        }
        Value::Object(statement)
    }
}

/// The table. Rows span the catalogues, bounds and forms the module doc describes.
fn statement_cases() -> Vec<StatementCase> {
    vec![
        StatementCase {
            case: "the modelled minimum of one action and one account principal, Allow, \
                   no condition, a one-character statementId",
            statement_id: "a".to_string(),
            effect: "Allow",
            action: vec!["entityresolution:GetMatchingJob"],
            principal: vec![account_id().to_string()],
            condition: None,
            expected_aws: vec![account_id().to_string()],
            expected_service: Vec::new(),
        },
        StatementCase {
            case: "Deny, three actions, an account and a service principal, and a \
                   condition carrying two condition keys",
            statement_id: "DenyCrossAccount0".to_string(),
            effect: "Deny",
            action: vec![
                "entityresolution:GetIdMappingJob",
                "entityresolution:StartMatchingJob",
                "entityresolution:GetPolicy",
            ],
            principal: vec![
                OTHER_ACCOUNT_ID.to_string(),
                "entityresolution.amazonaws.com".to_string(),
            ],
            condition: Some(
                json!({
                    "StringEquals": { "aws:PrincipalOrgID": "o-example" },
                    "Bool": { "aws:SecureTransport": "true" },
                })
                .to_string(),
            ),
            expected_aws: vec![OTHER_ACCOUNT_ID.to_string()],
            expected_service: vec!["entityresolution.amazonaws.com".to_string()],
        },
        StatementCase {
            case: "only service principals, with a statementId at the modelled length \
                   maximum of 64",
            statement_id: "S".repeat(64),
            effect: "Allow",
            action: vec!["entityresolution:GetSchemaMapping"],
            principal: vec![
                "entityresolution.amazonaws.com".to_string(),
                "glue.amazonaws.com".to_string(),
            ],
            condition: Some(
                json!({ "ArnLike": { "aws:SourceArn": expected_arn("schemamapping", "any") } })
                    .to_string(),
            ),
            expected_aws: Vec::new(),
            expected_service: vec![
                "entityresolution.amazonaws.com".to_string(),
                "glue.amazonaws.com".to_string(),
            ],
        },
        StatementCase {
            case: "two account principals and a digits-and-letters statementId",
            statement_id: "sid0123456789".to_string(),
            effect: "Allow",
            action: vec![
                "entityresolution:GetIdNamespace",
                "entityresolution:ListSchemaMappings",
            ],
            principal: vec![account_id().to_string(), OTHER_ACCOUNT_ID.to_string()],
            condition: None,
            expected_aws: vec![account_id().to_string(), OTHER_ACCOUNT_ID.to_string()],
            expected_service: Vec::new(),
        },
        StatementCase {
            case: "Deny with the shortest action the modelled member admits and an \
                   empty condition block",
            statement_id: "0".to_string(),
            effect: "Deny",
            action: vec!["entityresolution:Get"],
            principal: vec!["s3.amazonaws.com".to_string()],
            condition: Some(json!({}).to_string()),
            expected_aws: Vec::new(),
            expected_service: vec!["s3.amazonaws.com".to_string()],
        },
        StatementCase {
            case: "a mixed principal list of both forms",
            statement_id: "Mixed9".to_string(),
            effect: "Allow",
            action: vec!["entityresolution:GetMatchingWorkflow"],
            principal: vec![
                "entityresolution.amazonaws.com".to_string(),
                account_id().to_string(),
                OTHER_ACCOUNT_ID.to_string(),
            ],
            condition: None,
            expected_aws: vec![account_id().to_string(), OTHER_ACCOUNT_ID.to_string()],
            expected_service: vec!["entityresolution.amazonaws.com".to_string()],
        },
    ]
}

/// Does `value` satisfy the modelled `PolicyToken` — `length 36..36`, pattern
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`
/// (`smithy_query(query="shape",shape="PolicyToken")`)?
fn is_policy_token(value: &str) -> bool {
    let widths = [8usize, 4, 4, 4, 12];
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == widths.len()
        && groups.iter().zip(widths).all(|(group, width)| {
            group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

/// Assert `reply` carries a `PolicyDocument` whose `Statement` list is exactly
/// `statements`, in that order, and whose other members are `expected_members`; answer
/// the `token` it reported.
fn assert_policy(
    case: &str,
    source: &str,
    reply: &Reply,
    statements: &[Value],
    expected_members: &[(&str, Value)],
) -> String {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200 from {source}, got {reply:?}"
    );
    let document = string_member(&reply.body, "policy");
    let parsed: Value = serde_json::from_str(&document).unwrap_or_else(|error| {
        panic!("case {case:?}: {source} must answer a JSON policy document, got {document:?} ({error})")
    });
    assert_eq!(
        parsed.get("Statement"),
        Some(&Value::Array(statements.to_vec())),
        "case {case:?}: {source}'s document must carry the stored statements, got {parsed:?}"
    );
    for (member, value) in expected_members {
        assert_eq!(
            parsed.get(*member),
            Some(value),
            "case {case:?}: {source} must carry the document's {member} through, got \
             {parsed:?}"
        );
    }

    let token = string_member(&reply.body, "token");
    assert!(
        is_policy_token(&token),
        "case {case:?}: {source} must answer a modelled PolicyToken, got {token:?}"
    );
    token
}

/// The `Version` a composed document carries, as an expected-member row.
fn composed_members() -> Vec<(&'static str, Value)> {
    vec![("Version", json!(POLICY_VERSION))]
}

/// Assert `reply` is the 404 the policy paths answer when the resource carries no
/// policy, with the recorded message.
fn assert_no_policy(case: &str, reply: &Reply) {
    assert_error_shape(case, reply, 404, "ResourceNotFoundException");
    assert_eq!(
        reply.body.get("message").and_then(Value::as_str),
        Some(NO_POLICY_MESSAGE),
        "case {case:?}: expected the recorded no-policy message, got {reply:?}"
    );
}

#[tokio::test]
async fn add_policy_statement_persists_the_statement_and_get_policy_serves_it() {
    let emulator = Emulator::new();

    for (index, case) in statement_cases().into_iter().enumerate() {
        let label = case.case;
        // Rows cycle the resource type, so the one policy API is exercised over every
        // type the modelled ARN admits rather than over one of them.
        let resource_type = RESOURCE_TYPES[index % RESOURCE_TYPES.len()];
        let arn = create_resource(&emulator, resource_type, &format!("target-{index}")).await;

        let added = add_policy_statement(&emulator, &arn, &case.statement_id, &case.request()).await;
        assert_eq!(
            string_member(&added.body, "arn"),
            arn,
            "case {label:?}: AddPolicyStatement must answer the resource's own ARN, got {:?}",
            added.body
        );
        let expected = vec![case.expected_statement(&arn)];
        let token = assert_policy(
            label,
            "AddPolicyStatement",
            &added,
            &expected,
            &composed_members(),
        );

        // The round trip: the reader serves the document and the revision the add
        // produced, byte for byte.
        let read = get_policy(&emulator, &arn).await;
        assert_eq!(
            read.body, added.body,
            "case {label:?}: GetPolicy must answer exactly what the add persisted, got \
             {:?} after {:?}",
            read.body, added.body
        );
        assert_eq!(
            string_member(&read.body, "token"),
            token,
            "case {label:?}: GetPolicy must serve the token the add minted, got {:?}",
            read.body
        );

        // A read changes nothing a later caller sees.
        let again = get_policy(&emulator, &arn).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: two consecutive GetPolicy calls must answer identically, got \
             {again:?}"
        );
    }
}

#[tokio::test]
async fn statements_accumulate_in_add_order_and_every_add_mints_a_new_token() {
    let emulator = Emulator::new();
    let arn = create_resource(&emulator, "matchingworkflow", "accumulating").await;

    let cases = statement_cases();
    let mut expected = Vec::new();
    let mut tokens: Vec<String> = Vec::new();

    for case in &cases {
        let label = case.case;
        let added = add_policy_statement(&emulator, &arn, &case.statement_id, &case.request()).await;

        // Every statement added so far is in the document, in the order they were
        // added: a new statement never replaces or reorders the earlier ones.
        expected.push(case.expected_statement(&arn));
        let token = assert_policy(
            label,
            "AddPolicyStatement",
            &added,
            &expected,
            &composed_members(),
        );
        assert!(
            !tokens.contains(&token),
            "case {label:?}: each add is a new revision, so its token must be new, got \
             {token:?} after {tokens:?}"
        );
        tokens.push(token.clone());

        let read = get_policy(&emulator, &arn).await;
        assert_eq!(
            read.body, added.body,
            "case {label:?}: GetPolicy must serve the latest revision, got {read:?}"
        );
    }

    assert_eq!(
        tokens.len(),
        cases.len(),
        "every row must have produced a revision, got {tokens:?}"
    );
}

#[tokio::test]
async fn a_statement_added_to_a_written_document_keeps_the_rest_of_that_document() {
    let emulator = Emulator::new();
    let arn = create_resource(&emulator, "idnamespace", "composed").await;

    // A document a `PutPolicy` caller wrote, with members an add has no business
    // touching and one statement of its own.
    let written = json!({
        "Version": POLICY_VERSION,
        "Id": "written-by-put-policy",
        "Statement": [{
            "Sid": "WrittenStatement",
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", account_id()) },
            "Action": ["entityresolution:GetIdNamespace"],
            "Resource": "*",
        }],
    });
    let put = put_policy(&emulator, &arn, &written.to_string()).await;
    assert_eq!(put.status, 200, "seeding the document failed: {put:?}");
    let put_token = string_member(&put.body, "token");

    let case = &statement_cases()[1];
    let added = add_policy_statement(&emulator, &arn, &case.statement_id, &case.request()).await;

    // The written statement is still first, the added one is appended, and `Id` and
    // `Version` came through untouched.
    let expected = vec![
        written["Statement"][0].clone(),
        case.expected_statement(&arn),
    ];
    let token = assert_policy(
        "a statement added to a written document",
        "AddPolicyStatement",
        &added,
        &expected,
        &[
            ("Version", json!(POLICY_VERSION)),
            ("Id", json!("written-by-put-policy")),
        ],
    );
    assert_ne!(
        token, put_token,
        "the add is a new revision, so its token must differ from the write's, got \
         {token:?} twice"
    );

    let read = get_policy(&emulator, &arn).await;
    assert_eq!(
        read.body, added.body,
        "GetPolicy must answer exactly what the add persisted, got {read:?}"
    );
}

#[tokio::test]
async fn a_duplicate_statement_id_is_refused_and_leaves_the_policy_untouched() {
    let emulator = Emulator::new();

    for (index, case) in statement_cases().into_iter().enumerate() {
        let label = case.case;
        let resource_type = RESOURCE_TYPES[index % RESOURCE_TYPES.len()];
        let arn = create_resource(&emulator, resource_type, &format!("duplicate-{index}")).await;

        let added = add_policy_statement(&emulator, &arn, &case.statement_id, &case.request()).await;
        assert_eq!(
            added.status, 200,
            "case {label:?}: the first add must succeed, got {added:?}"
        );
        let before = get_policy(&emulator, &arn).await;

        // The same identifier twice, whether or not the rest of the statement differs:
        // `statementId` is what differentiates a statement from the others in the same
        // policy, so neither add can be accepted.
        let repeats = [
            ("the identical statement again", case.request()),
            (
                "a different statement under the same statementId",
                json!({
                    "effect": "Deny",
                    "action": ["entityresolution:DeletePolicyStatement"],
                    "principal": [OTHER_ACCOUNT_ID],
                }),
            ),
        ];
        for (repeat, request) in repeats {
            let duplicate =
                add_policy_statement(&emulator, &arn, &case.statement_id, &request).await;
            assert_error_shape(
                &format!("{label} / {repeat}"),
                &duplicate,
                400,
                "ValidationException",
            );
            assert_eq!(
                duplicate.body.get("message").and_then(Value::as_str),
                Some(DUPLICATE_STATEMENT_MESSAGE),
                "case {label:?} / {repeat}: expected the recorded duplicate-statementId \
                 message, got {duplicate:?}"
            );

            let after = get_policy(&emulator, &arn).await;
            assert_eq!(
                after.body, before.body,
                "case {label:?} / {repeat}: a refused add must leave the policy and its \
                 token untouched, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn delete_policy_statement_removes_the_named_statement_and_leaves_the_rest() {
    let emulator = Emulator::new();

    for (index, resource_type) in RESOURCE_TYPES.iter().enumerate() {
        let arn = create_resource(&emulator, resource_type, &format!("deleting-{index}")).await;
        let cases = statement_cases();

        // Every row's statement on one resource, so a delete has siblings to leave
        // behind rather than emptying the policy on the first call.
        for case in &cases {
            let added =
                add_policy_statement(&emulator, &arn, &case.statement_id, &case.request()).await;
            assert_eq!(
                added.status, 200,
                "case {:?}: seeding the statement failed: {added:?}",
                case.case
            );
        }
        let seeded = get_policy(&emulator, &arn).await;

        // Removing a statement the document does NOT hold removes nothing, so it is no
        // new revision: same document, same token.
        let absent = delete_policy_statement(&emulator, &arn, "neverAdded").await;
        assert_eq!(
            absent.status, 200,
            "{resource_type}: deleting a statementId the policy never held must succeed, \
             got {absent:?}"
        );
        assert_eq!(
            absent.body, seeded.body,
            "{resource_type}: a delete that removed nothing must change nothing, got {absent:?}"
        );

        // Each delete removes exactly its own statement, leaves the others in order,
        // and mints a new revision token.
        let mut remaining: Vec<&StatementCase> = cases.iter().collect();
        let mut tokens = vec![string_member(&seeded.body, "token")];
        while !remaining.is_empty() {
            let removed = remaining.remove(remaining.len() / 2);
            let label = removed.case;
            let reply = delete_policy_statement(&emulator, &arn, &removed.statement_id).await;
            assert_eq!(
                string_member(&reply.body, "arn"),
                arn,
                "case {label:?}: DeletePolicyStatement must answer the resource's own ARN, \
                 got {:?}",
                reply.body
            );

            let expected: Vec<Value> = remaining
                .iter()
                .map(|case| case.expected_statement(&arn))
                .collect();
            let token = assert_policy(
                label,
                "DeletePolicyStatement",
                &reply,
                &expected,
                &composed_members(),
            );
            assert!(
                !tokens.contains(&token),
                "case {label:?}: a removal is a new revision, so its token must be new, got \
                 {token:?} after {tokens:?}"
            );
            tokens.push(token.clone());

            // The reader agrees with what the delete answered, as long as a statement
            // remains: the last removal deletes the policy itself, which the block
            // after this loop covers.
            if !remaining.is_empty() {
                let read = get_policy(&emulator, &arn).await;
                assert_eq!(
                    read.body, reply.body,
                    "case {label:?}: GetPolicy must serve the revision the delete \
                     produced, got {read:?}"
                );

                // Deleting the same statement again removes nothing, so it is
                // idempotent: same document, same token.
                let again = delete_policy_statement(&emulator, &arn, &removed.statement_id).await;
                assert_eq!(
                    again.body, reply.body,
                    "case {label:?}: a repeated delete must answer identically, got {again:?}"
                );
            }
        }

        // A policy is its statements, so the removal of the last one deleted the
        // policy: its readers answer not-found again, exactly as they did before the
        // first statement was added.
        assert_no_policy(
            "a policy whose last statement was deleted",
            &get_policy(&emulator, &arn).await,
        );
        assert_no_policy(
            "a delete against a policy that no longer exists",
            &delete_policy_statement(&emulator, &arn, &cases[0].statement_id).await,
        );

        // Every statementId is free again, and the first add starts a new policy.
        let case = &cases[0];
        let readded =
            add_policy_statement(&emulator, &arn, &case.statement_id, &case.request()).await;
        assert_policy(
            "the re-added statement",
            "AddPolicyStatement",
            &readded,
            &[case.expected_statement(&arn)],
            &composed_members(),
        );
    }
}

#[tokio::test]
async fn a_statement_call_naming_no_resource_or_no_policy_is_not_found() {
    let emulator = Emulator::new();
    let case = &statement_cases()[0];

    // One resource of each type EXISTS, so a miss is a real lookup miss and not an
    // empty store.
    for resource_type in RESOURCE_TYPES {
        create_resource(&emulator, resource_type, "exists").await;
    }

    let missing = [
        (
            "a name nothing was created under",
            expected_arn("matchingworkflow", "never-created"),
        ),
        (
            "a name that exists under ANOTHER resource type",
            expected_arn("idnamespace", "exists-elsewhere"),
        ),
        (
            "another account's ARN for a name that exists here",
            format!("arn:aws:entityresolution:{REGION}:{OTHER_ACCOUNT_ID}:schemamapping/exists"),
        ),
        (
            "another Region's ARN for a name that exists here",
            format!(
                "arn:aws:entityresolution:us-west-2:{}:schemamapping/exists",
                account_id()
            ),
        ),
        (
            "another partition's ARN for a name that exists here",
            format!(
                "arn:aws-us-gov:entityresolution:{REGION}:{}:schemamapping/exists",
                account_id()
            ),
        ),
    ];

    for (label, arn) in &missing {
        let added = add_policy_statement(&emulator, arn, &case.statement_id, &case.request()).await;
        assert_error_shape(label, &added, 404, "ResourceNotFoundException");
        let removed = delete_policy_statement(&emulator, arn, &case.statement_id).await;
        assert_error_shape(label, &removed, 404, "ResourceNotFoundException");
    }

    // A resource that EXISTS but carries no policy: there is no revision to report, so
    // the delete answers not-found rather than inventing a token — while the add on the
    // same ARN starts the policy.
    for (index, resource_type) in RESOURCE_TYPES.iter().enumerate() {
        let arn = create_resource(&emulator, resource_type, &format!("unpoliced-{index}")).await;
        assert_no_policy(resource_type, &delete_policy_statement(&emulator, &arn, "none").await);
        assert_no_policy(resource_type, &get_policy(&emulator, &arn).await);

        let added = add_policy_statement(&emulator, &arn, &case.statement_id, &case.request()).await;
        assert_policy(
            resource_type,
            "AddPolicyStatement",
            &added,
            &[case.expected_statement(&arn)],
            &composed_members(),
        );
    }
}

#[tokio::test]
async fn a_condition_that_is_not_a_json_object_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let arn = create_resource(&emulator, "idnamespace", "conditioned").await;

    // Values the modelled `StatementCondition` (a string of `length 1..40960`) admits
    // but a statement's `Condition` block cannot be: not JSON at all, and JSON that is
    // not an object.
    let refused = [
        ("not JSON at all", "StringEquals aws:PrincipalOrgID o-example"),
        ("a truncated object", "{\"StringEquals\":"),
        ("a JSON array", "[{\"StringEquals\": {}}]"),
        ("a JSON string", "\"StringEquals\""),
        ("a JSON number", "42"),
        ("JSON null", "null"),
        ("a JSON boolean", "true"),
    ];

    for (case, condition) in refused {
        let request = json!({
            "effect": "Allow",
            "action": ["entityresolution:GetIdNamespace"],
            "principal": [account_id()],
            "condition": condition,
        });
        let reply = add_policy_statement(&emulator, &arn, "conditioned", &request).await;
        assert_error_shape(case, &reply, 400, "ValidationException");

        // A refused add stores nothing, so the resource still has no policy at all.
        assert_no_policy(case, &get_policy(&emulator, &arn).await);
    }

    // The same request with a condition that IS an object is accepted, so what the
    // refusals above reject is the condition and not the rest of the statement.
    let accepted = json!({
        "effect": "Allow",
        "action": ["entityresolution:GetIdNamespace"],
        "principal": [account_id()],
        "condition": json!({ "StringEquals": { "aws:PrincipalOrgID": "o-example" } }).to_string(),
    });
    let reply = add_policy_statement(&emulator, &arn, "conditioned", &accepted).await;
    assert_eq!(
        reply.status, 200,
        "a condition that is a JSON object must be accepted, got {reply:?}"
    );
}

#[tokio::test]
async fn a_policy_cannot_outgrow_what_a_policy_document_can_carry() {
    let emulator = Emulator::new();
    let arn = create_resource(&emulator, "matchingworkflow", "bounded").await;

    /// One `AddPolicyStatement` body whose `condition` is `filler` characters long —
    /// `StatementCondition` is modelled `length 1..40960`, so a single statement can
    /// carry more than the whole document admits.
    fn padded(filler: usize) -> Value {
        json!({
            "effect": "Allow",
            "action": ["entityresolution:GetMatchingWorkflow"],
            "principal": [account_id()],
            "condition": json!({ "StringEquals": { "aws:PrincipalOrgID": "o".repeat(filler) } })
                .to_string(),
        })
    }

    // One statement whose condition alone is past the document maximum: refused, and
    // nothing is stored for it.
    let overflowing = add_policy_statement(&emulator, &arn, "tooBig", &padded(40800)).await;
    assert_error_shape(
        "a single statement past the document maximum",
        &overflowing,
        400,
        "ValidationException",
    );
    assert_no_policy(
        "a refused first add stores nothing",
        &get_policy(&emulator, &arn).await,
    );

    // Repeated adds under distinct statementIds do not grow the policy without end:
    // the first add that would take the document past the maximum is refused, and
    // every accepted one leaves a document inside it.
    let mut accepted = 0;
    let mut refused = None;
    for index in 0..40 {
        let statement_id = format!("padded{index}");
        let reply = add_policy_statement(&emulator, &arn, &statement_id, &padded(4000)).await;
        if reply.status == 200 {
            accepted += 1;
            let document = string_member(&reply.body, "policy");
            assert!(
                document.chars().count() <= MAX_POLICY_DOCUMENT,
                "case {statement_id:?}: an accepted add must leave a servable document, got \
                 {} characters",
                document.chars().count()
            );
        } else {
            refused = Some(reply);
            break;
        }
    }

    let refused = refused.expect("the adds must stop being accepted before the loop ends");
    assert_error_shape(
        "the add that would outgrow the document",
        &refused,
        400,
        "ValidationException",
    );
    assert!(
        accepted > 0,
        "statements inside the bound must be accepted, got {accepted} of them"
    );

    // The refusal changed nothing: the policy still reads as the statements that were
    // accepted, inside the bound.
    let read = get_policy(&emulator, &arn).await;
    assert_eq!(
        read.status, 200,
        "the accepted statements must still read back, got {read:?}"
    );
    let document = string_member(&read.body, "policy");
    assert!(
        document.chars().count() <= MAX_POLICY_DOCUMENT,
        "the stored policy must stay servable, got {} characters",
        document.chars().count()
    );
    let parsed: Value = serde_json::from_str(&document).expect("the document is JSON");
    assert_eq!(
        parsed["Statement"].as_array().map(Vec::len),
        Some(accepted),
        "the policy must hold exactly the accepted statements, got {parsed:?}"
    );
}
