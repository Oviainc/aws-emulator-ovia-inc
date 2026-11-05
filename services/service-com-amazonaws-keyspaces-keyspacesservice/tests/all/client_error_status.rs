// SPDX-License-Identifier: CC0-1.0

//! The HTTP status line, and the refusal sentence, Amazon Keyspaces puts on a modelled
//! client-side exception — driven end to end over the service's own `awsJson1_0` wire
//! protocol through the shared [`crate::keyspace_table_lifecycle::Emulator`] harness, so
//! what is pinned is the emulator's observable behaviour rather than its internals.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. On this RPC service's wire the error TYPE in the body discriminates a failure, not
//!    the transport status: EVERY client-side exception travels with the protocol's own
//!    client-error status (`scaffolded::CLIENT_ERROR_STATUS`), whatever per-shape
//!    `@httpError` the model annotates that exception with. So the rows below span the
//!    exception shapes this service models with a 4xx `@httpError`
//!    (`ConflictException` 409, `ResourceNotFoundException` 404,
//!    `ServiceQuotaExceededException` 402, `ValidationException` 400 — the catalog from
//!    `smithy_query(query="shape",shape="ConflictException,ResourceNotFoundException,\
//!    ServiceQuotaExceededException,AccessDeniedException,ValidationException",\
//!    model_path="services/service-com-amazonaws-keyspaces-keyspacesservice/smithy-model/\
//!    com-amazonaws-keyspaces-keyspacesservice.json")`), the operations that answer them,
//!    and both the handler path and the boundary validator's own refusal.
//! 2. A refusal sentence is the service's own fixed template plus the names the REQUEST
//!    supplied — with nothing taken from the caller's credential scope — one template per
//!    fault, so the operation that DROPS a resource has its own (pinned in
//!    `refusal_sentences`) rather than the one an operation that READS it answers with.
//!    The rows span the resource kind the sentence is about
//!    (keyspace, table, user-defined type), the fault (a name already held, a name nothing
//!    holds) and the form of the name (shortest, longest, mixed case, double-quoted).
//! 3. A request the BOUNDARY validator refuses is stated as AWS's own validation envelope
//!    — `N validation error(s) detected: Value '<value>' at '<member>' failed to satisfy
//!    constraint: <requirement>`, with a `@pattern` quoted without the anchors a
//!    whole-value pattern wraps it in — and the clauses about ONE member come out in the
//!    order that member's own refusal is recorded in. The rows span the number of
//!    violations (one, two) and the two members whose recorded order differs.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::scaffolded;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_ok, error_code, minimal_schema, settle_wait, Emulator,
};
use crate::regional_partitioning::RegionalEmulator;

/// One refusal to drive: the call to make, the exception the body must name, and the
/// per-shape `@httpError` the model annotates that exception with — which is exactly what
/// must NOT reach the wire.
struct Refusal {
    /// The case name, used in every assertion message.
    case: &'static str,
    /// The operation to call.
    operation: &'static str,
    /// Its request body.
    request: Value,
    /// The wire error code the body must carry.
    code: &'static str,
    /// The status the exception's `@httpError` trait declares.
    modelled_http_error: u16,
}

/// The `message` member of a failure body, as a string.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message member>")
        .to_string()
}

/// RULE 1: every modelled client-side exception reaches the client on this protocol's
/// client-error status, with the exception type in the body telling the failures apart.
#[tokio::test]
async fn every_modelled_client_exception_travels_with_the_protocols_client_error_status() {
    let emulator = Emulator::start();
    let keyspace = "status_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    assert_ok(
        "create the table",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": "held_tbl",
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    assert_ok(
        "create the type",
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": keyspace,
                    "typeName": "held_udt",
                    "fieldDefinitions": [{"name": "f", "type": "text"}],
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    // One field list past the per-type cap, which is what earns the quota exception.
    let over_the_field_cap: Vec<Value> = (0..=keyspaces::MAX_FIELDS_PER_TYPE)
        .map(|index| json!({"name": format!("f{index}"), "type": "int"}))
        .collect();
    let absent_arn = format!(
        "arn:aws:cassandra:us-west-2:{}:/keyspace/no_such_ks/",
        engine::emulator_account_id()
    );

    let refusals = vec![
        Refusal {
            case: "CreateKeyspace on a name already held",
            operation: "CreateKeyspace",
            request: json!({"keyspaceName": keyspace}),
            code: "ConflictException",
            modelled_http_error: 409,
        },
        Refusal {
            case: "CreateTable on a name already held",
            operation: "CreateTable",
            request: json!({
                "keyspaceName": keyspace,
                "tableName": "held_tbl",
                "schemaDefinition": minimal_schema("pk"),
            }),
            code: "ConflictException",
            modelled_http_error: 409,
        },
        Refusal {
            case: "CreateType on a name already held",
            operation: "CreateType",
            request: json!({
                "keyspaceName": keyspace,
                "typeName": "held_udt",
                "fieldDefinitions": [{"name": "f", "type": "text"}],
            }),
            code: "ConflictException",
            modelled_http_error: 409,
        },
        Refusal {
            case: "GetKeyspace on a keyspace nothing holds",
            operation: "GetKeyspace",
            request: json!({"keyspaceName": "no_such_ks"}),
            code: "ResourceNotFoundException",
            modelled_http_error: 404,
        },
        Refusal {
            case: "GetTable on a table nothing holds",
            operation: "GetTable",
            request: json!({"keyspaceName": keyspace, "tableName": "no_such_tbl"}),
            code: "ResourceNotFoundException",
            modelled_http_error: 404,
        },
        Refusal {
            case: "GetType on a type nothing holds",
            operation: "GetType",
            request: json!({"keyspaceName": keyspace, "typeName": "no_such_udt"}),
            code: "ResourceNotFoundException",
            modelled_http_error: 404,
        },
        Refusal {
            case: "ListTypes in a keyspace nothing holds",
            operation: "ListTypes",
            request: json!({"keyspaceName": "no_such_ks"}),
            code: "ResourceNotFoundException",
            modelled_http_error: 404,
        },
        Refusal {
            case: "DeleteKeyspace on a keyspace nothing holds",
            operation: "DeleteKeyspace",
            request: json!({"keyspaceName": "no_such_ks"}),
            code: "ResourceNotFoundException",
            modelled_http_error: 404,
        },
        Refusal {
            case: "GetTableAutoScalingSettings on a table nothing holds",
            operation: "GetTableAutoScalingSettings",
            request: json!({"keyspaceName": keyspace, "tableName": "no_such_tbl"}),
            code: "ResourceNotFoundException",
            modelled_http_error: 404,
        },
        Refusal {
            case: "ListTagsForResource on an ARN naming no stored resource",
            operation: "ListTagsForResource",
            request: json!({"resourceArn": absent_arn}),
            code: "ResourceNotFoundException",
            modelled_http_error: 404,
        },
        Refusal {
            case: "CreateType past the per-type field cap",
            operation: "CreateType",
            request: json!({
                "keyspaceName": keyspace,
                "typeName": "over_the_cap",
                "fieldDefinitions": over_the_field_cap,
            }),
            code: "ServiceQuotaExceededException",
            modelled_http_error: 402,
        },
        Refusal {
            // A rule the model cannot state, refused by the handler: a service-managed
            // keyspace takes none of the caller's resources.
            case: "CreateType in a service-managed keyspace",
            operation: "CreateType",
            request: json!({
                "keyspaceName": "system",
                "typeName": "intruder",
                "fieldDefinitions": [{"name": "f", "type": "text"}],
            }),
            code: "ValidationException",
            modelled_http_error: 400,
        },
        Refusal {
            // And one the model DOES state, refused by the boundary validator before any
            // handler runs: the same status, so the two paths agree.
            case: "CreateKeyspace with a name outside the modelled pattern",
            operation: "CreateKeyspace",
            request: json!({"keyspaceName": "dash-name"}),
            code: "ValidationException",
            modelled_http_error: 400,
        },
    ];

    for refusal in &refusals {
        let (status, body) = emulator
            .call(refusal.operation, refusal.request.clone())
            .await;
        let case = refusal.case;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, refusal.code),
            "case {case:?}: expected HTTP {} {} (the modelled @httpError {} is not what \
             this protocol's wire carries), got {status} {body}",
            scaffolded::CLIENT_ERROR_STATUS,
            refusal.code,
            refusal.modelled_http_error
        );
        // The per-shape annotation is the one status that must NOT reach the wire, for
        // every shape whose annotation differs from this protocol's own.
        if refusal.modelled_http_error != scaffolded::CLIENT_ERROR_STATUS {
            assert_ne!(
                status, refusal.modelled_http_error,
                "case {case:?}: the per-shape @httpError must not reach the wire, got \
                 {status} {body}"
            );
        }
        assert!(
            !message(&body).is_empty(),
            "case {case:?}: a refusal must say why, got {body}"
        );
    }
}

/// RULE 1, on the mapping itself: a client error's status collapses onto this protocol's
/// own, and every other status — a server fault, the scaffold channel's 501 — is left
/// alone, so a retry classifier can still tell a server fault from a client one.
#[test]
fn the_client_error_statuses_collapse_and_every_other_status_is_left_alone() {
    // The 4xx column holds every client `@httpError` this service's model carries, plus
    // the ends of the class, so the rule is a range and not a list of shapes.
    for client_status in [400u16, 402, 403, 404, 409, 429, 499] {
        let got = scaffolded::wire_status(client_status);
        assert_eq!(
            got,
            scaffolded::CLIENT_ERROR_STATUS,
            "case {client_status}: a client error travels with HTTP {}, got {got}",
            scaffolded::CLIENT_ERROR_STATUS
        );
    }
    for untouched in [200u16, 301, 500, 501, 503] {
        let got = scaffolded::wire_status(untouched);
        assert_eq!(
            got, untouched,
            "case {untouched}: only the client-error class collapses, got {got}"
        );
    }
}

/// RULE 2, for a name already held: the sentence is the fixed template plus the name the
/// request supplied.
#[tokio::test]
async fn a_name_already_held_is_refused_with_the_requested_name_and_nothing_else() {
    let emulator = Emulator::start();

    // The rows span the modelled `keyspaceName` shape (`len 1..48`, pattern
    // `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`): the shortest name, a digit-led one, the longest,
    // one carrying upper case, and one carrying underscores.
    let longest = "k".repeat(48);
    for name in ["a", "0", "demo_ks", "Mixed_Case_9", &longest, "ks__double"] {
        assert_ok(
            name,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": name}))
                .await,
        );
        let (status, body) = emulator
            .call("CreateKeyspace", json!({"keyspaceName": name}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ConflictException"),
            "case {name:?}: expected the modelled conflict, got {status} {body}"
        );
        assert_eq!(
            message(&body),
            format!("Cannot add existing keyspace {name}"),
            "case {name:?}: unexpected refusal sentence, got {body}"
        );
    }

    // A user-defined type whose name is already held, over the two forms a `typeName`
    // takes — an unquoted name, which the service folds, and a double-quoted one, which it
    // preserves — in two keyspaces, so the sentence names the keyspace of the request.
    for keyspace in ["udt_ks_one", "udt_ks_two"] {
        assert_ok(
            keyspace,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
                .await,
        );
        for submitted in ["addr", "AddrMixed", "\"Addr\"", "a"] {
            let case = format!("{keyspace}/{submitted}");
            let request = json!({
                "keyspaceName": keyspace,
                "typeName": submitted,
                "fieldDefinitions": [{"name": "f", "type": "text"}],
            });
            assert_ok(&case, emulator.call("CreateType", request.clone()).await);
            let (status, body) = emulator.call("CreateType", request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (scaffolded::CLIENT_ERROR_STATUS, "ConflictException"),
                "case {case:?}: expected the modelled conflict, got {status} {body}"
            );
            // The type's name as the service resolved it from the request — the identifier
            // the keyspace holds it under — and the keyspace name the request supplied.
            let formatted = keyspaces::format_type_name(submitted)
                .expect("every submitted name above is a form a typeName may take");
            assert_eq!(
                message(&body),
                format!("Cannot add already existing UDT {formatted} to keyspace {keyspace}"),
                "case {case:?}: unexpected refusal sentence, got {body}"
            );
        }
    }
}

/// RULE 2: the sentence carries the names of the REQUEST and nothing of the caller's
/// credential scope, so the same request refused from two Regions is refused in the same
/// words.
#[tokio::test]
async fn a_refusal_sentence_carries_nothing_of_the_credential_scope() {
    let emulator = RegionalEmulator::start();
    let name = "scope_free_ks";
    for region in ["us-west-2", "eu-central-1", "ap-southeast-1"] {
        assert_ok(
            region,
            emulator
                .call(region, "CreateKeyspace", json!({"keyspaceName": name}))
                .await,
        );
        let (status, body) = emulator
            .call(region, "CreateKeyspace", json!({"keyspaceName": name}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ConflictException"),
            "case {region:?}: expected the modelled conflict, got {status} {body}"
        );
        assert_eq!(
            message(&body),
            format!("Cannot add existing keyspace {name}"),
            "case {region:?}: the sentence must not depend on the Region, got {body}"
        );
        let (status, body) = emulator
            .call(region, "GetKeyspace", json!({"keyspaceName": "absent_ks"}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ResourceNotFoundException"),
            "case {region:?}: expected the modelled not-found, got {status} {body}"
        );
        assert_eq!(
            message(&body),
            "Keyspace absent_ks does not exist",
            "case {region:?}: the sentence must not depend on the Region, got {body}"
        );
    }
}

/// RULE 2, for a name nothing holds: one sentence per resource kind, each the fixed
/// template plus the name the request named.
#[tokio::test]
async fn a_name_nothing_holds_is_refused_with_the_requested_name_and_nothing_else() {
    let emulator = Emulator::start();
    let keyspace = "absence_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    let longest = format!("absent_{}", "n".repeat(41));
    // (case, operation, request, the sentence the service answers)
    let rows: Vec<(String, &str, Value, String)> = vec![
        // A keyspace nothing holds: the shortest and the longest well-formed names, one
        // carrying upper case, and a prefix of a name that IS held.
        ("keyspace/shortest".to_string(), "GetKeyspace", json!({"keyspaceName": "z"}),
            "Keyspace z does not exist".to_string()),
        ("keyspace/longest".to_string(), "GetKeyspace", json!({"keyspaceName": longest.clone()}),
            format!("Keyspace {longest} does not exist")),
        ("keyspace/mixed_case".to_string(), "GetKeyspace", json!({"keyspaceName": "Absent_Ks9"}),
            "Keyspace Absent_Ks9 does not exist".to_string()),
        ("keyspace/prefix_of_a_held_name".to_string(), "GetKeyspace", json!({"keyspaceName": "absence"}),
            "Keyspace absence does not exist".to_string()),
        // The same sentence whichever operation looked the keyspace up — but the DROP
        // states the fault it hit, a failed drop, which `refusal_sentences` pins over the
        // whole class.
        ("keyspace/through_list_types".to_string(), "ListTypes", json!({"keyspaceName": "z"}),
            "Keyspace z does not exist".to_string()),
        ("keyspace/through_delete".to_string(), "DeleteKeyspace", json!({"keyspaceName": "z"}),
            "Cannot drop non existing keyspace 'z'".to_string()),
        // A table nothing holds, in a keyspace that is held.
        ("table/plain".to_string(), "GetTable",
            json!({"keyspaceName": keyspace, "tableName": "absent_tbl"}),
            "unconfigured table absent_tbl".to_string()),
        ("table/mixed_case".to_string(), "GetTable",
            json!({"keyspaceName": keyspace, "tableName": "Absent_Tbl9"}),
            "unconfigured table Absent_Tbl9".to_string()),
        // A user-defined type nothing holds, in both name forms.
        ("type/plain".to_string(), "GetType",
            json!({"keyspaceName": keyspace, "typeName": "absent_udt"}),
            "UDT absent_udt does not exist".to_string()),
        ("type/folded".to_string(), "GetType",
            json!({"keyspaceName": keyspace, "typeName": "AbsentUdt"}),
            "UDT absentudt does not exist".to_string()),
        ("type/quoted".to_string(), "GetType",
            json!({"keyspaceName": keyspace, "typeName": "\"Absent\""}),
            "UDT \"Absent\" does not exist".to_string()),
    ];

    for (case, operation, request, expected) in &rows {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ResourceNotFoundException"),
            "case {case:?}: expected the modelled not-found, got {status} {body}"
        );
        assert_eq!(
            &message(&body),
            expected,
            "case {case:?}: unexpected refusal sentence, got {body}"
        );
    }
}

/// RULE 3: a boundary refusal is AWS's validation envelope, with the `@pattern` quoted
/// unanchored and each member's clauses in the order that member's refusal is recorded in.
#[tokio::test]
async fn a_boundary_refusal_is_stated_as_the_aws_validation_envelope() {
    let emulator = Emulator::start();
    let keyspace = "envelope_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );

    // `KeyspaceName` and `TableName` are both modelled `len 1..48` with the pattern below
    // (`smithy_query(query="shape",shape="KeyspaceName,TableName",…)`), so the clauses the
    // envelope carries are the same pair for either member — only their ORDER differs.
    const NAME_PATTERN: &str = "[a-zA-Z0-9][a-zA-Z0-9_]{0,47}";
    const LENGTH_MAXIMUM: usize = 48;
    let over_long = "a".repeat(67);

    // (case, operation, request, the message stated)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            // Both constraints broken: `keyspaceName` states the pattern clause first.
            "keyspaceName over its maximum",
            "CreateKeyspace",
            json!({"keyspaceName": over_long}),
            format!(
                "2 validation errors detected: Value '{over_long}' at 'keyspaceName' failed \
                 to satisfy constraint: Member must satisfy regular expression pattern: \
                 {NAME_PATTERN}; Value '{over_long}' at 'keyspaceName' failed to satisfy \
                 constraint: Member must have length less than or equal to {LENGTH_MAXIMUM}"
            ),
        ),
        (
            // The same pair on `tableName`, stated the other way round.
            "tableName over its maximum",
            "CreateTable",
            json!({
                "keyspaceName": keyspace,
                "tableName": over_long,
                "schemaDefinition": minimal_schema("pk"),
            }),
            format!(
                "2 validation errors detected: Value '{over_long}' at 'tableName' failed to \
                 satisfy constraint: Member must have length less than or equal to \
                 {LENGTH_MAXIMUM}; Value '{over_long}' at 'tableName' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {NAME_PATTERN}"
            ),
        ),
        (
            // One violation: the singular envelope, and the same unanchored pattern.
            "keyspaceName outside the pattern only",
            "CreateKeyspace",
            json!({"keyspaceName": "dash-name"}),
            format!(
                "1 validation error detected: Value 'dash-name' at 'keyspaceName' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {NAME_PATTERN}"
            ),
        ),
        (
            "tableName outside the pattern only",
            "CreateTable",
            json!({
                "keyspaceName": keyspace,
                "tableName": "dash-name",
                "schemaDefinition": minimal_schema("pk"),
            }),
            format!(
                "1 validation error detected: Value 'dash-name' at 'tableName' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {NAME_PATTERN}"
            ),
        ),
    ];

    for (case, operation, request, expected) in &rows {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (scaffolded::CLIENT_ERROR_STATUS, "ValidationException"),
            "case {case:?}: expected the modelled validation failure, got {status} {body}"
        );
        assert_eq!(
            &message(&body),
            expected,
            "case {case:?}: unexpected validation envelope, got {body}"
        );
    }
}
