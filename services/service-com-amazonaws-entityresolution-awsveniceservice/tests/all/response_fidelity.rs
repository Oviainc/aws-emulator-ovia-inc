// SPDX-License-Identifier: CC0-1.0

//! What an Entity Resolution response LOOKS like on the wire, beside the state it
//! reports: the success STATUS of an operation that models an empty output, the exact
//! text of a lifecycle `message`, and the SHAPE of the policy document a composed
//! statement is written into.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the `schema_mapping_workflow_lifecycle` suite owns — so what is asserted is
//! what an AWS SDK client would see, not an internal function's return value.
//!
//! Three rules are under test:
//!
//! 1. An operation whose output structure models NO members has nothing to serialize,
//!    so its success is HTTP 204 with no body; an operation that models output members
//!    answers 200 and that body. In this service the empty-output operations are
//!    `TagResource` and `UntagResource`
//!    (`smithy_query(query="shape",shape="TagResourceOutput,UntagResourceOutput")`).
//! 2. A message is that operation's own text as the service words it: a `Delete*`
//!    success message is its fixed string, and a `Delete*` not-found refusal is a
//!    template naming the resource type the API spells and the identifier the request
//!    asked for — `<ResourceType> <name> does not exist` — while a READER of the same
//!    resource names nothing.
//! 3. A statement composed into a resource-based policy document is NORMALIZED into
//!    what an IAM document holds: an account-id principal becomes that account's IAM
//!    root-user ARN, and a member composed from a one-element list is a scalar, not a
//!    one-element array. The write stores that rendering, so `GetPolicy` serves it
//!    back unchanged.
//!
//! Each table row differs along what its rule generalises over rather than repeating
//! one recorded request: the resource TYPE over every type a `VeniceGlobalArn` names,
//! the tag map over the empty map, one pair, several pairs and the modelled `TagKey`
//! (1..128) and `TagValue` (0..256) bounds, the resource NAME over the character
//! classes the modelled `EntityName` pattern `^[a-zA-Z_0-9-]*$` admits at its length
//! minimum of 1 and above it, the delete operation over all four of this service's
//! deletes, and a statement's `action` and `principal` over one value and several, in
//! both branches of the modelled `StatementPrincipal` alternation
//! (`^((\d{12})|([a-z0-9\.]+))$`).

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::id_mapping_workflow_delete::delete_id_mapping_workflow;
use crate::policy_statements::{add_policy_statement, create_resource, RESOURCE_TYPES};
use crate::resource_policy::get_policy;
use crate::resource_tagging::{list_tags, tag_resource, untag_resource};
use crate::schema_mapping_workflow_lifecycle::{
    account_id, assert_error_shape, string_member, Emulator, Reply,
};

/// The success text `DeleteIdNamespace` answers, as a literal here so this suite pins
/// the wording — `deleted` lowercase — independently of the constant the service crate
/// declares.
const ID_NAMESPACE_DELETED: &str = "IdNamespace deleted.";

/// The not-found text the ID-namespace READERS answer: a constant that names no
/// namespace, which is what makes the delete's templated text a different string.
const ID_NAMESPACE_NOT_FOUND: &str = "IdNamespace Not Found";

/// A second account id, so a row can carry a principal that is not this emulator's own
/// account.
const OTHER_ACCOUNT_ID: &str = "123456789012";

/// Assert `reply` is an empty-output success: HTTP 204, no error code, and no member
/// for a client to read.
fn assert_no_content(case: &str, operation: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 204,
        "case {case:?}: {operation} models an empty output, so it must answer HTTP 204, \
         got {reply:?}"
    );
    assert_eq!(
        reply.error_code, None,
        "case {case:?}: {operation} succeeded, so it must carry no error code, got {:?}",
        reply.error_code
    );
    let empty = match &reply.body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case:?}: a no-content success carries no member to read, got {:?}",
        reply.body
    );
}

/// A JSON tag object from pairs.
fn tag_map(pairs: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// One tagging round trip: the pairs written and the keys taken away again.
struct TagCase {
    case: &'static str,
    /// Which resource type the row tags.
    resource_type: &'static str,
    pairs: Vec<(String, String)>,
    removed: Vec<String>,
}

/// The tagging table. The key and value bounds are the ones the sibling
/// `resource_tagging` suite names for the modelled `TagMap` (`TagKey` 1..128,
/// `TagValue` 0..256).
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "one pair on a schema mapping",
            resource_type: "schemamapping",
            pairs: vec![("env".to_string(), "test".to_string())],
            removed: vec!["env".to_string()],
        },
        TagCase {
            case: "three pairs on a matching workflow, two of them removed",
            resource_type: "matchingworkflow",
            pairs: vec![
                ("env".to_string(), "test".to_string()),
                ("team".to_string(), "identity".to_string()),
                ("cost-center".to_string(), "42".to_string()),
            ],
            removed: vec!["env".to_string(), "cost-center".to_string()],
        },
        TagCase {
            case: "the empty tag map the modelled TagMap admits, and an untag of a key \
                   that was never set",
            resource_type: "idnamespace",
            pairs: Vec::new(),
            removed: vec!["never-set".to_string()],
        },
        TagCase {
            case: "a key at the modelled TagKey maximum of 128 and a value at the \
                   TagValue maximum of 256",
            resource_type: "idmappingworkflow",
            pairs: vec![("k".repeat(128), "v".repeat(256))],
            removed: vec!["k".repeat(128)],
        },
        TagCase {
            case: "a value at the modelled TagValue minimum of 0, left in place by an \
                   untag that names another key",
            resource_type: "schemamapping",
            pairs: vec![("empty".to_string(), String::new())],
            removed: vec!["other".to_string()],
        },
    ]
}

#[tokio::test]
async fn the_tagging_writers_answer_204_with_no_body_and_still_persist_the_write() {
    let emulator = Emulator::new();

    for (index, case) in tag_cases().into_iter().enumerate() {
        let label = case.case;
        let resource_arn =
            create_resource(&emulator, case.resource_type, &format!("tagged-{index}")).await;

        let tagged = tag_resource(&emulator, &resource_arn, &tag_map(&case.pairs)).await;
        assert_no_content(label, "TagResource", &tagged);

        // The 204 is a SUCCESS and not a refusal that answered no body: the reader —
        // which models output members, so it answers 200 and a body — reports exactly
        // what the write persisted.
        let listed = list_tags(&emulator, &resource_arn).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: ListTagsForResource models output members, so it answers \
             HTTP 200, got {listed:?}"
        );
        assert_eq!(
            listed.body.get("tags"),
            Some(&tag_map(&case.pairs)),
            "case {label:?}: the tags the 204 persisted must read back, got {:?}",
            listed.body
        );

        let untagged = untag_resource(&emulator, &resource_arn, &case.removed).await;
        assert_no_content(label, "UntagResource", &untagged);

        // Untagging removed exactly the named keys, and repeating it is idempotent —
        // and still a 204.
        let remaining: Vec<(String, String)> = case
            .pairs
            .iter()
            .filter(|(key, _)| !case.removed.contains(key))
            .cloned()
            .collect();
        let listed = list_tags(&emulator, &resource_arn).await;
        assert_eq!(
            listed.body.get("tags"),
            Some(&tag_map(&remaining)),
            "case {label:?}: exactly the named keys must be gone, got {:?}",
            listed.body
        );
        let repeated = untag_resource(&emulator, &resource_arn, &case.removed).await;
        assert_no_content(label, "a repeated UntagResource", &repeated);
    }
}

#[tokio::test]
async fn a_refused_tagging_call_is_not_a_204() {
    let emulator = Emulator::new();

    // The 204 belongs to the SUCCESS path only: a refusal carries the modelled error
    // status and a body a client can read the code and message from. One ARN that names
    // no resource, over every resource type the ARN admits.
    for resource_type in RESOURCE_TYPES {
        // A resource of this type does exist, so the refusal is a lookup miss and not
        // an empty store.
        create_resource(&emulator, resource_type, "present").await;
        let absent = format!(
            "arn:aws:entityresolution:us-east-1:{}:{resource_type}/never-created",
            account_id()
        );

        for (operation, reply) in [
            (
                "TagResource",
                tag_resource(&emulator, &absent, &json!({ "env": "test" })).await,
            ),
            (
                "UntagResource",
                untag_resource(&emulator, &absent, &["env".to_string()]).await,
            ),
        ] {
            assert_error_shape(
                &format!("{resource_type} [{operation}]"),
                &reply,
                404,
                "ResourceNotFoundException",
            );
            assert!(
                reply.body.get("message").is_some(),
                "case {resource_type:?}: {operation}'s refusal must carry a message, got \
                 {reply:?}"
            );
        }
    }
}

/// The resource names a message row is asserted over: every character class the
/// modelled `EntityName` pattern `^[a-zA-Z_0-9-]*$` admits, at the length minimum of 1
/// and above it.
fn names() -> Vec<(&'static str, String)> {
    vec![
        ("a single-letter name at the modelled length minimum", "a".to_string()),
        ("mixed case letters", "Customers".to_string()),
        ("underscore, hyphen and digits together", "crm_people-1".to_string()),
        ("a digits-only name", "0".to_string()),
        (
            "every character class the modelled pattern admits",
            "UPPER_lower-123".to_string(),
        ),
        ("a 200-character name", "n".repeat(200)),
    ]
}

#[tokio::test]
async fn delete_id_namespace_answers_the_services_own_success_text() {
    let emulator = Emulator::new();
    let mut seen = BTreeSet::new();

    for (label, name) in names() {
        let created = emulator
            .create_id_namespace(&json!({ "idNamespaceName": name, "type": "SOURCE" }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}"
        );

        let deleted = emulator.delete_id_namespace(&name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteIdNamespace models an output member, so it answers \
             HTTP 200, got {deleted:?}"
        );
        let message = string_member(&deleted.body, "message");
        assert_eq!(
            message, ID_NAMESPACE_DELETED,
            "case {label:?}: expected the success message {ID_NAMESPACE_DELETED:?}, got \
             {message:?}"
        );
        seen.insert(message);
    }

    assert_eq!(
        seen.len(),
        1,
        "the success text is fixed, so it must not depend on the namespace that was \
         deleted, saw {seen:?}"
    );
}

/// One of this service's four delete operations: the resource type the API spells in
/// its messages, how a resource of it is created, and how it is deleted.
struct DeletePath {
    operation: &'static str,
    /// The resource type as the API spells it in a message (`IdNamespace`, not
    /// `idnamespace`).
    resource_type: &'static str,
    /// The same type as a `VeniceGlobalArn` segment, which is how
    /// [`create_resource`] names it.
    arn_segment: &'static str,
}

/// The four delete paths. Each one addresses its resource by an `EntityName` label.
const DELETE_PATHS: &[DeletePath] = &[
    DeletePath {
        operation: "DeleteSchemaMapping",
        resource_type: "SchemaMapping",
        arn_segment: "schemamapping",
    },
    DeletePath {
        operation: "DeleteMatchingWorkflow",
        resource_type: "MatchingWorkflow",
        arn_segment: "matchingworkflow",
    },
    DeletePath {
        operation: "DeleteIdMappingWorkflow",
        resource_type: "IdMappingWorkflow",
        arn_segment: "idmappingworkflow",
    },
    DeletePath {
        operation: "DeleteIdNamespace",
        resource_type: "IdNamespace",
        arn_segment: "idnamespace",
    },
];

/// Call `path`'s delete for `name`.
async fn delete(emulator: &Emulator, path: &DeletePath, name: &str) -> Reply {
    match path.operation {
        "DeleteSchemaMapping" => emulator.delete_schema_mapping(name).await,
        "DeleteMatchingWorkflow" => emulator.delete_matching_workflow(name).await,
        "DeleteIdMappingWorkflow" => delete_id_mapping_workflow(emulator, name).await,
        "DeleteIdNamespace" => emulator.delete_id_namespace(name).await,
        other => panic!("no delete is known for {other}"),
    }
}

#[tokio::test]
async fn a_delete_that_finds_nothing_names_the_resource_it_could_not_delete() {
    let emulator = Emulator::new();

    for path in DELETE_PATHS {
        for (label, name) in names() {
            let case = format!("{} / {label}", path.operation);
            let expected = format!("{} {name} does not exist", path.resource_type);

            let refused = delete(&emulator, path, &name).await;
            assert_error_shape(&case, &refused, 404, "ResourceNotFoundException");
            assert_eq!(
                string_member(&refused.body, "message"),
                expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                refused.body
            );
        }
    }
}

#[tokio::test]
async fn the_same_delete_succeeds_once_the_resource_is_there() {
    let emulator = Emulator::new();

    // The 404 above is decided by what the store holds and not by the request's shape:
    // the identical call goes through once the resource exists. The names here stay
    // short because both workflow types reference a schema mapping whose name is
    // derived from this one, and an `EntityName` is at most 255 characters.
    for path in DELETE_PATHS {
        for (label, name) in [
            ("a lowercase name with a digit", "probe0"),
            ("letters, digits, _ and -", "UPPER_lower-123"),
        ] {
            let case = format!("{} / {label}", path.operation);
            create_resource(&emulator, path.arn_segment, name).await;

            let deleted = delete(&emulator, path, name).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: the delete of an existing resource must succeed, got \
                 {deleted:?}"
            );
            assert!(
                !string_member(&deleted.body, "message").contains(name),
                "case {case:?}: the SUCCESS text is fixed, so it carries no name, got {:?}",
                deleted.body
            );

            // ...and the refusal that follows names it again.
            let again = delete(&emulator, path, name).await;
            assert_eq!(
                string_member(&again.body, "message"),
                format!("{} {name} does not exist", path.resource_type),
                "case {case:?}: the repeated delete must name the resource, got {:?}",
                again.body
            );
        }
    }
}

#[tokio::test]
async fn an_id_namespace_reader_does_not_name_the_namespace_the_delete_does() {
    let emulator = Emulator::new();
    // The whole configuration `UpdateIdNamespace` models is optional, so an empty body
    // is a legitimate update and the 404 is about the namespace.
    let update = json!({});

    for (label, name) in names() {
        for (operation, reply) in [
            ("GetIdNamespace", emulator.get_id_namespace(&name).await),
            (
                "UpdateIdNamespace",
                emulator.update_id_namespace(&name, &update).await,
            ),
        ] {
            let case = format!("{operation} / {label}");
            assert_error_shape(&case, &reply, 404, "ResourceNotFoundException");
            assert_eq!(
                string_member(&reply.body, "message"),
                ID_NAMESPACE_NOT_FOUND,
                "case {case:?}: a reader's refusal names no namespace, got {:?}",
                reply.body
            );
        }
    }
}

/// One `AddPolicyStatement` request plus the shape the document must hold it in.
struct PolicyCase {
    case: &'static str,
    statement_id: &'static str,
    effect: &'static str,
    /// `StatementActionList`, modelled `length 1..`.
    action: Vec<&'static str>,
    /// `StatementPrincipalList`, modelled `length 1..`, each member
    /// `^((\d{12})|([a-z0-9\.]+))$`: a 12-digit account id or a dotted service name.
    principal: Vec<String>,
    /// The account principals, each as the IAM root-user ARN it is expanded to.
    expected_aws: Vec<String>,
    /// The service principals, which a document holds as they stand.
    expected_service: Vec<String>,
}

/// The IAM identity an account-id principal names in a policy document: that account's
/// root user.
fn iam_root(account: &str) -> String {
    format!("arn:aws:iam::{account}:root")
}

/// One statement member composed from a modelled LIST input, in the shape a policy
/// document holds it: a single value as a SCALAR, several as an array in order.
fn statement_member(values: &[String]) -> Value {
    match values {
        [only] => Value::String(only.clone()),
        several => json!(several),
    }
}

/// The policy table: one and several values, in both branches of the principal
/// alternation and in a mix of them.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "the modelled minimum of one action and one account principal: both \
                   scalars, the principal an IAM root ARN",
            statement_id: "OneOfEach",
            effect: "Allow",
            action: vec!["entityresolution:GetIdNamespace"],
            principal: vec![account_id().to_string()],
            expected_aws: vec![iam_root(account_id())],
            expected_service: Vec::new(),
        },
        PolicyCase {
            case: "two account principals and one action: the principals an array of \
                   root ARNs, the action still a scalar",
            statement_id: "TwoAccounts",
            effect: "Allow",
            action: vec!["entityresolution:GetPolicy"],
            principal: vec![account_id().to_string(), OTHER_ACCOUNT_ID.to_string()],
            expected_aws: vec![iam_root(account_id()), iam_root(OTHER_ACCOUNT_ID)],
            expected_service: Vec::new(),
        },
        PolicyCase {
            case: "one service principal and three actions: the service name as it \
                   stands and the actions an array",
            statement_id: "ServiceOnly",
            effect: "Deny",
            action: vec![
                "entityresolution:GetMatchingWorkflow",
                "entityresolution:StartMatchingJob",
                "entityresolution:GetMatchingJob",
            ],
            principal: vec!["entityresolution.amazonaws.com".to_string()],
            expected_aws: Vec::new(),
            expected_service: vec!["entityresolution.amazonaws.com".to_string()],
        },
        PolicyCase {
            case: "both principal branches at once and two actions: each key a scalar, \
                   the actions an array",
            statement_id: "Mixed0",
            effect: "Allow",
            action: vec![
                "entityresolution:GetSchemaMapping",
                "entityresolution:ListSchemaMappings",
            ],
            principal: vec![
                OTHER_ACCOUNT_ID.to_string(),
                "glue.amazonaws.com".to_string(),
            ],
            expected_aws: vec![iam_root(OTHER_ACCOUNT_ID)],
            expected_service: vec!["glue.amazonaws.com".to_string()],
        },
        PolicyCase {
            case: "two service principals and one action: the services an array, the \
                   action a scalar",
            statement_id: "TwoServices",
            effect: "Allow",
            action: vec!["entityresolution:GetIdMappingWorkflow"],
            principal: vec![
                "entityresolution.amazonaws.com".to_string(),
                "s3.amazonaws.com".to_string(),
            ],
            expected_aws: Vec::new(),
            expected_service: vec![
                "entityresolution.amazonaws.com".to_string(),
                "s3.amazonaws.com".to_string(),
            ],
        },
    ]
}

impl PolicyCase {
    /// The `AddPolicyStatement` body for this row.
    fn request(&self) -> Value {
        json!({
            "effect": self.effect,
            "action": self.action,
            "principal": self.principal,
        })
    }

    /// The statement the document must hold for this row.
    fn expected_statement(&self, resource_arn: &str) -> Value {
        let mut principal = Map::new();
        if !self.expected_aws.is_empty() {
            principal.insert("AWS".to_string(), statement_member(&self.expected_aws));
        }
        if !self.expected_service.is_empty() {
            principal.insert(
                "Service".to_string(),
                statement_member(&self.expected_service),
            );
        }
        let actions: Vec<String> = self.action.iter().map(|action| action.to_string()).collect();
        json!({
            "Sid": self.statement_id,
            "Effect": self.effect,
            "Principal": Value::Object(principal),
            "Action": statement_member(&actions),
            "Resource": resource_arn,
        })
    }
}

/// The `Statement` list of the `policy` document `reply` carries.
fn statements(case: &str, source: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200 from {source}, got {reply:?}"
    );
    let document = string_member(&reply.body, "policy");
    let parsed: Value = serde_json::from_str(&document).unwrap_or_else(|error| {
        panic!("case {case:?}: {source} must answer a JSON document, got {document:?} ({error})")
    });
    parsed
        .get("Statement")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {source}'s document carries no Statement list, got {parsed:?}")
        })
        .clone()
}

#[tokio::test]
async fn a_composed_statement_is_written_in_the_shape_a_policy_document_holds() {
    let emulator = Emulator::new();

    for (index, case) in policy_cases().into_iter().enumerate() {
        let label = case.case;
        // Rows cycle the resource type, so the rendering is exercised over every type
        // the modelled ARN admits.
        let resource_type = RESOURCE_TYPES[index % RESOURCE_TYPES.len()];
        let arn = create_resource(&emulator, resource_type, &format!("rendered-{index}")).await;

        let added = add_policy_statement(&emulator, &arn, case.statement_id, &case.request()).await;
        let written = statements(label, "AddPolicyStatement", &added);
        assert_eq!(
            written,
            vec![case.expected_statement(&arn)],
            "case {label:?}: the composed statement must be normalized, got {written:?}"
        );

        // The shape, stated as the rule rather than as this row's literal: a member
        // composed from one value is a scalar, from several an array.
        let statement = &written[0];
        assert_eq!(
            statement["Action"].is_string(),
            case.action.len() == 1,
            "case {label:?}: Action is a scalar exactly when one action was asked for, \
             got {:?}",
            statement["Action"]
        );
        for (key, values) in [
            ("AWS", &case.expected_aws),
            ("Service", &case.expected_service),
        ] {
            let member = &statement["Principal"][key];
            assert_eq!(
                member.is_string(),
                values.len() == 1,
                "case {label:?}: Principal.{key} is a scalar exactly when one principal \
                 of that kind was asked for, got {member:?}"
            );
        }
        // No account id is left as a bare identifier: every account principal is an ARN.
        for account in [account_id(), OTHER_ACCOUNT_ID] {
            let bare = Value::String(account.to_string());
            assert_ne!(
                statement["Principal"]["AWS"], bare,
                "case {label:?}: an account principal must be expanded to its IAM root \
                 ARN, got {:?}",
                statement["Principal"]
            );
            assert!(
                !statement["Principal"]["AWS"]
                    .as_array()
                    .is_some_and(|values| values.contains(&bare)),
                "case {label:?}: an account principal must be expanded to its IAM root \
                 ARN, got {:?}",
                statement["Principal"]
            );
        }

        // The write STORED that rendering, so the reader serves it back unchanged.
        let read = get_policy(&emulator, &arn).await;
        assert_eq!(
            read.body, added.body,
            "case {label:?}: GetPolicy must serve exactly the document the add stored, \
             got {:?} after {:?}",
            read.body, added.body
        );
    }
}
