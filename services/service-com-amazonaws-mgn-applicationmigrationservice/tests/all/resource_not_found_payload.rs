// SPDX-License-Identifier: CC0-1.0

//! What an mgn `ResourceNotFoundException` payload says, end to end over HTTP.
//!
//! The rule under test: a refusal for an id the account holds no record of names the
//! resource KIND in `resourceType` the way the API spells it — the modelled resource
//! shape's own name — carries the requested id in `resourceId`, and words `message`
//! from that type name and that id. The ARN path segment (`application/…`,
//! `network-migration-definition/…`) is a different spelling of the same kind and
//! never appears in the payload.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives.

use serde_json::{json, Value};

use crate::application_lifecycle::{error_code, Mgn};

/// The AWS resource type names this service reports, one per modelled resource shape:
/// the shape's name without its `Resource` suffix. Source:
/// `smithy_query(query="shape",shape="ApplicationResource,NetworkMigrationDefinitionResource")`
/// — `com.amazonaws.mgn#ApplicationResource` (ARN template `application/{applicationID}`)
/// and `com.amazonaws.mgn#NetworkMigrationDefinitionResource` (ARN template
/// `network-migration-definition/{networkMigrationDefinitionID}`).
const APPLICATION_TYPE: &str = "Application";
const NETWORK_MIGRATION_DEFINITION_TYPE: &str = "NetworkMigrationDefinition";

/// One probe: an operation that looks a resource up by id, an id nothing was created
/// under, and the resource type name the refusal must name.
struct Case {
    case: &'static str,
    operation: &'static str,
    request: Value,
    resource_type: &'static str,
    requested_id: &'static str,
}

/// The class the rule spans: both implemented resource kinds (so two different type
/// names, one of them a multi-word kind whose ARN segment is kebab-cased), the three
/// operations that answer the refusal (a read, an update and a delete), and ids
/// differing in their body characters (all digits, all letters, mixed case) inside the
/// modelled id patterns `^app-[0-9a-zA-Z]{17}$` and `^nmd-[0-9a-zA-Z]{17}$`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "get an unknown definition, all-digit id",
            operation: "network-migration/GetNetworkMigrationDefinition",
            request: json!({"networkMigrationDefinitionID": "nmd-00000000000000000"}),
            resource_type: NETWORK_MIGRATION_DEFINITION_TYPE,
            requested_id: "nmd-00000000000000000",
        },
        Case {
            case: "get an unknown definition, all-letter id",
            operation: "network-migration/GetNetworkMigrationDefinition",
            request: json!({"networkMigrationDefinitionID": "nmd-zzzzzzzzzzzzzzzzz"}),
            resource_type: NETWORK_MIGRATION_DEFINITION_TYPE,
            requested_id: "nmd-zzzzzzzzzzzzzzzzz",
        },
        Case {
            case: "get an unknown definition, mixed-case id",
            operation: "network-migration/GetNetworkMigrationDefinition",
            request: json!({"networkMigrationDefinitionID": "nmd-A1b2C3d4E5f6G7h8I"}),
            resource_type: NETWORK_MIGRATION_DEFINITION_TYPE,
            requested_id: "nmd-A1b2C3d4E5f6G7h8I",
        },
        Case {
            case: "delete an unknown application, all-digit id",
            operation: "DeleteApplication",
            request: json!({"applicationID": "app-00000000000000000"}),
            resource_type: APPLICATION_TYPE,
            requested_id: "app-00000000000000000",
        },
        Case {
            case: "update an unknown application, mixed-case id",
            operation: "UpdateApplication",
            request: json!({"applicationID": "app-A1b2C3d4E5f6G7h8I", "name": "renamed"}),
            resource_type: APPLICATION_TYPE,
            requested_id: "app-A1b2C3d4E5f6G7h8I",
        },
        Case {
            case: "update an unknown application, all-letter id, description only",
            operation: "UpdateApplication",
            request: json!({"applicationID": "app-zzzzzzzzzzzzzzzzz", "description": "x"}),
            resource_type: APPLICATION_TYPE,
            requested_id: "app-zzzzzzzzzzzzzzzzz",
        },
    ]
}

fn member<'a>(body: &'a Value, name: &str) -> &'a str {
    body[name].as_str().unwrap_or("<absent>")
}

/// Every not-found refusal reports the AWS resource type name, the requested id and a
/// message built from the two — never an internal, kebab-cased spelling of the kind.
#[tokio::test]
async fn a_not_found_refusal_names_the_aws_resource_type_and_the_requested_id() {
    let mgn = Mgn::new();
    // The application operations refuse an uninitialized account before they look an
    // id up, so the account is initialized first; the network-migration operations
    // model no such refusal.
    mgn.initialize().await;

    for Case {
        case,
        operation,
        request,
        resource_type,
        requested_id,
    } in cases()
    {
        let (status, error_type, body) = mgn.call_reporting_error_type(operation, &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} with {body:?}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the x-amzn-errortype header to name the error shape, got {error_type:?}"
        );
        let actual_type = member(&body, "resourceType");
        assert_eq!(
            actual_type, resource_type,
            "case {case:?}: expected resourceType {resource_type:?}, got {actual_type:?}"
        );
        let actual_id = member(&body, "resourceId");
        assert_eq!(
            actual_id, requested_id,
            "case {case:?}: expected resourceId {requested_id:?}, got {actual_id:?}"
        );
        let expected_message = format!("{resource_type} with Id: '{requested_id}' was not found");
        let actual_message = member(&body, "message");
        assert_eq!(
            actual_message, expected_message,
            "case {case:?}: expected message {expected_message:?}, got {actual_message:?}"
        );
    }
}

/// The reported type name is the API's spelling of the kind, not the ARN path segment:
/// it is upper-camel-cased and carries none of the separators an ARN segment uses. The
/// invariant holds for every kind the service reports, so a kind added later cannot
/// quietly report a slug.
#[tokio::test]
async fn a_reported_resource_type_is_never_an_arn_path_segment() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    for Case {
        case,
        operation,
        request,
        resource_type,
        ..
    } in cases()
    {
        let (_, body) = mgn.call(operation, &request).await;
        let actual = member(&body, "resourceType");
        assert!(
            actual.chars().all(|c| c.is_ascii_alphanumeric())
                && actual.starts_with(|c: char| c.is_ascii_uppercase()),
            "case {case:?}: a resource type name is upper-camel-case ASCII with no ARN separators, got {actual:?}"
        );
        // The ARN spelling of the same kind, lower-kebab-cased, must not be what the
        // payload reports.
        let arn_segment = resource_type
            .char_indices()
            .map(|(index, c)| {
                if c.is_ascii_uppercase() && index > 0 {
                    format!("-{}", c.to_ascii_lowercase())
                } else {
                    c.to_ascii_lowercase().to_string()
                }
            })
            .collect::<String>();
        assert_ne!(
            actual, arn_segment,
            "case {case:?}: the payload must not report the ARN path segment {arn_segment:?}"
        );
    }
}
