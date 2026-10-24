// SPDX-License-Identifier: CC0-1.0

//! Every form of this service's `X-Amz-Target` prefix reaches the same operation.
//!
//! The rule under test: on the `awsJson` protocols a request names its operation out of
//! band, in `x-amz-target: <target prefix>.<OperationName>`. The prefix is OPAQUE text an
//! SDK copies out of its service model and MAY carry dots of its own — this service's
//! published prefix is the fully qualified Coral service name,
//! `com.amazonaws.codestar.connections.CodeStar_connections_20191201` — while an
//! operation name is a Smithy identifier (ASCII letters, digits and `_`) and so never
//! can. So the OPERATION a request names is the segment after the target's LAST dot, and
//! the SERVICE is the last dot-separated segment of what precedes it, whatever the prefix
//! in front of it.
//!
//! `connection_lifecycle.rs` and `host_deletion.rs` drive the short prefix, the one form
//! the framework's own first-dot reading already resolved. This suite asserts the rule
//! over the CLASS it generalises across: prefix forms differing in how many
//! dot-separated labels they carry (none, one, three, four, five) and in what those
//! labels are, driven over the whole modelled operation surface, over a resource round
//! trip, over a modelled refusal, and — because a request carrying no SigV4 credential
//! scope is routed by its target alone — signed and unsigned.
//!
//! Everything goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's harness (its `call_target` / `call_target_unsigned`
//! name the target verbatim), so what is asserted is what a client sees.

use serde_json::{json, Value};

use super::connection_lifecycle::{ACCOUNT_ID, Emulator, REGION, error_code};

/// The `X-Amz-Target` prefix forms that name THIS service, each `(case, prefix)`.
///
/// The service shape is `com.amazonaws.codestarconnections#CodeStar_connections_20191201`
/// (`smithy_query(query="service_info")`), so its local name
/// `CodeStar_connections_20191201` is the last label of every row; the rows differ only in
/// what stands in front of it, which is all the rule generalises over.
const PREFIXES: &[(&str, &str)] = &[
    ("the bare service shape name", "CodeStar_connections_20191201"),
    (
        // The form an AWS SDK sends for this service: the fully qualified Coral service
        // name, whose namespace carries dots of its own.
        "the fully qualified Coral name an AWS SDK sends",
        "com.amazonaws.codestar.connections.CodeStar_connections_20191201",
    ),
    (
        "one namespace label before the shape name",
        "codestarconnections.CodeStar_connections_20191201",
    ),
    (
        "the absolute shape id",
        "com.amazonaws.codestarconnections.CodeStar_connections_20191201",
    ),
    (
        "a deeper namespace than any SDK sends",
        "a.b.c.d.e.CodeStar_connections_20191201",
    ),
];

/// Every operation this service binds, from `OPERATIONS.md` (one `## <Operation>` section
/// each), which the model's `operation_count: 27` counts.
const OPERATIONS: &[&str] = &[
    "CreateConnection",
    "CreateHost",
    "CreateRepositoryLink",
    "CreateSyncConfiguration",
    "DeleteConnection",
    "DeleteHost",
    "DeleteRepositoryLink",
    "DeleteSyncConfiguration",
    "GetConnection",
    "GetHost",
    "GetRepositoryLink",
    "GetRepositorySyncStatus",
    "GetResourceSyncStatus",
    "GetSyncBlockerSummary",
    "GetSyncConfiguration",
    "ListConnections",
    "ListHosts",
    "ListRepositoryLinks",
    "ListRepositorySyncDefinitions",
    "ListSyncConfigurations",
    "ListTagsForResource",
    "TagResource",
    "UntagResource",
    "UpdateHost",
    "UpdateRepositoryLink",
    "UpdateSyncBlocker",
    "UpdateSyncConfiguration",
];

/// Targets that name NO operation of this service, each `(case, whole target)`.
///
/// Each one carries a DOTTED prefix: under the framework's first-dot reading its routing
/// key would be a `<namespace tail>.<Service>.<Operation>` string, which names no
/// operation of any service — so resolving the operation from the last dot must not turn
/// one of these into a handler call.
const UNSERVED_TARGETS: &[(&str, &str)] = &[
    (
        "another service's dotted prefix",
        "com.amazonaws.codeconnections.CodeConnections_20231201.CreateConnection",
    ),
    (
        "a last label that merely ends in this shape name's characters",
        "com.amazonaws.codestar.connections.NotCodeStar_connections_20191201.ListConnections",
    ),
    (
        "an operation this model does not bind",
        "com.amazonaws.codestar.connections.CodeStar_connections_20191201.NoSuchOperation",
    ),
    (
        "nothing after the last dot",
        "com.amazonaws.codestar.connections.CodeStar_connections_20191201.",
    ),
];

/// Dispatch one signed call to `operation` behind `prefix`.
async fn call(emulator: &Emulator, prefix: &str, operation: &str, body: Value) -> (u16, Value) {
    emulator
        .call_target(&format!("{prefix}.{operation}"), body)
        .await
}

/// The message an error envelope carries, under either casing the protocols use.
fn error_message(response: &Value) -> String {
    response["message"]
        .as_str()
        .or_else(|| response["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The framework's unrouted-501 wording for `target`: it names the routing key it could
/// not resolve, which is everything after the target's FIRST dot. That message is the
/// marker of a request that reached no operation at all — the scaffold channel's own 501,
/// for an operation nobody has implemented yet, names the operation by its short name and
/// says it is `scaffolded but not implemented`.
fn unrouted_message(target: &str) -> String {
    let key = target
        .split_once('.')
        .map_or_else(|| target.to_string(), |(_, rest)| rest.to_string());
    format!("operation {key} is not implemented")
}

/// RULE: EVERY modelled operation is reached behind EVERY prefix form that names this
/// service.
///
/// Whatever the operation then answers — its output, a modelled failure, or the request
/// walk refusing an empty body — it is the operation answering, not the dispatcher
/// refusing a key it could not resolve. The one 501 a routed request may get is the
/// scaffold channel's, which names the operation by its short name.
#[tokio::test]
async fn every_modelled_operation_is_reached_behind_every_prefix_form() {
    assert_eq!(
        OPERATIONS.len(),
        27,
        "the operation table must cover the service's whole surface (model operation_count \
         27), got {}",
        OPERATIONS.len()
    );

    for (form, prefix) in PREFIXES {
        let emulator = Emulator::new();
        for operation in OPERATIONS {
            let target = format!("{prefix}.{operation}");
            let (status, body) = call(&emulator, prefix, operation, json!({})).await;
            let message = error_message(&body);
            assert!(
                !message.contains(&unrouted_message(&target)),
                "case {form:?}, {operation}: the target must reach the operation, got \
                 {status} {body}"
            );
            if error_code(&body) == "NotImplemented" {
                assert!(
                    message.contains(&format!("operation {operation} is scaffolded")),
                    "case {form:?}, {operation}: a 501 here may only be the scaffold \
                     channel's, got {status} {body}"
                );
            }
        }
    }
}

/// RULE: the prefix form changes neither the operation a request reaches nor the state it
/// reads — a connection created behind one form reads back behind every other, out of the
/// one store, and the list reports every connection whichever form enumerates it.
#[tokio::test]
async fn one_store_lies_behind_every_prefix_form() {
    let emulator = Emulator::new();

    let mut created: Vec<(&str, String)> = Vec::new();
    for (index, (form, prefix)) in PREFIXES.iter().enumerate() {
        // One connection per writing form, named after it, so a read can tell which form
        // stored which row.
        let (status, body) = call(
            &emulator,
            prefix,
            "CreateConnection",
            json!({ "ConnectionName": format!("prefix-form-{index}"), "ProviderType": "GitHub" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {form:?}: expected 200 from CreateConnection, got {status} {body}"
        );
        let arn = body["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {form:?}: no ConnectionArn: {body}"))
            .to_string();
        assert!(
            arn.starts_with(&format!(
                "arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:connection/"
            )),
            "case {form:?}: expected a connection ARN minted in {REGION}, got {arn:?}"
        );
        created.push((form, arn));
    }

    // Read every connection back behind EVERY form, including the ones that did not
    // write it, and in the status the create left it in.
    for (writing_form, arn) in &created {
        for (reading_form, prefix) in PREFIXES {
            let (status, body) = call(
                &emulator,
                prefix,
                "GetConnection",
                json!({ "ConnectionArn": arn }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {writing_form:?} written, read behind {reading_form:?}: expected 200 \
                 from GetConnection, got {status} {body}"
            );
            assert_eq!(
                (
                    body["Connection"]["ConnectionArn"].as_str(),
                    body["Connection"]["ConnectionStatus"].as_str(),
                ),
                (Some(arn.as_str()), Some("PENDING")),
                "case {writing_form:?} written, read behind {reading_form:?}: expected the \
                 pending connection {arn:?}, got {body}"
            );
        }
    }

    // One store behind all the forms: each enumeration reports every connection written.
    let expected: Vec<&str> = {
        let mut arns: Vec<&str> = created.iter().map(|(_, arn)| arn.as_str()).collect();
        arns.sort_unstable();
        arns
    };
    for (form, prefix) in PREFIXES {
        let (status, body) = call(&emulator, prefix, "ListConnections", json!({})).await;
        assert_eq!(
            status, 200,
            "case {form:?}: expected 200 from ListConnections, got {status} {body}"
        );
        let mut listed: Vec<&str> = body["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {form:?}: no Connections list: {body}"))
            .iter()
            .filter_map(|connection| connection["ConnectionArn"].as_str())
            .collect();
        listed.sort_unstable();
        assert_eq!(
            listed, expected,
            "case {form:?}: expected every connection written behind every form, got {body}"
        );
    }
}

/// RULE: a modelled refusal is the handler's, behind every prefix form — a tag operation
/// whose `ResourceArn` names no resource this service holds is the modelled
/// `ResourceNotFoundException` (404) and never a claim that the operation does not exist.
///
/// The rows are well-shaped `AmazonResourceName`s (modeled `len 1..1011`, pattern
/// `^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$`) that resolve to nothing: a connection id no
/// create minted, another region, another account, another partition, and another
/// vendor's resource.
#[tokio::test]
async fn an_unknown_resource_arn_is_refused_as_absent_behind_every_prefix_form() {
    let rows: Vec<(&str, String)> = vec![
        (
            "a connection id nothing minted",
            format!(
                "arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:connection/00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "another region",
            format!("arn:aws:codestar-connections:eu-central-1:{ACCOUNT_ID}:connection/absent"),
        ),
        (
            "another account",
            format!("arn:aws:codestar-connections:{REGION}:111122223333:connection/absent"),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:codestar-connections:{REGION}:{ACCOUNT_ID}:connection/absent"),
        ),
        (
            "another vendor's resource",
            format!("arn:aws:codepipeline:{REGION}:{ACCOUNT_ID}:pipeline/absent"),
        ),
    ];

    for (form, prefix) in PREFIXES {
        let emulator = Emulator::new();
        for (case, arn) in &rows {
            let (status, body) = call(
                &emulator,
                prefix,
                "TagResource",
                json!({ "ResourceArn": arn, "Tags": [{ "Key": "env", "Value": "dev" }] }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {form:?}/{case:?}: expected the modelled absence for {arn:?}, got \
                 {status} {body}"
            );
            assert!(
                error_message(&body).contains(arn.as_str()),
                "case {form:?}/{case:?}: the refusal must name the ARN supplied, got {body}"
            );

            // The same ARN is just as absent for the other two tag operations.
            let (status, body) = call(
                &emulator,
                prefix,
                "ListTagsForResource",
                json!({ "ResourceArn": arn }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {form:?}/{case:?}: ListTagsForResource must refuse {arn:?} as absent, \
                 got {status} {body}"
            );
            let (status, body) = call(
                &emulator,
                prefix,
                "UntagResource",
                json!({ "ResourceArn": arn, "TagKeys": ["env"] }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {form:?}/{case:?}: UntagResource must refuse {arn:?} as absent, got \
                 {status} {body}"
            );
        }
    }
}

/// RULE: a request whose ONLY routable signal is its `X-Amz-Target` reaches this service,
/// for every prefix form — and only when the target names THIS service.
///
/// With no credential scope the framework's routing key is the target's FIRST label,
/// which for a dotted prefix is a namespace label (`com`) naming no service, so such a
/// request would otherwise reach nobody and answer `404 UnknownService`.
#[tokio::test]
async fn an_unsigned_request_is_routed_by_the_service_its_target_names() {
    let emulator = Emulator::new();
    for (form, prefix) in PREFIXES {
        let (status, body) = emulator
            .call_target_unsigned(&format!("{prefix}.ListConnections"), json!({}))
            .await;
        assert_eq!(
            status, 200,
            "case {form:?}, unsigned: expected 200 from ListConnections, got {status} {body}"
        );
        assert!(
            body["Connections"].is_array(),
            "case {form:?}, unsigned: expected a Connections list, got {body}"
        );
    }

    for (case, target) in UNSERVED_TARGETS {
        let (status, body) = emulator.call_target_unsigned(target, json!({})).await;
        assert_ne!(
            status, 200,
            "case {case:?}, unsigned: this service must not serve {target:?}, got {body}"
        );
    }
}

/// RULE: resolving the operation from the last dot invents no handlers — a target that
/// names another service, or an operation this model does not bind, is still refused
/// before any handler runs, and nothing is created.
#[tokio::test]
async fn a_target_naming_no_operation_of_this_service_is_refused() {
    let emulator = Emulator::new();
    for (case, target) in UNSERVED_TARGETS {
        let (status, body) = emulator
            .call_target(
                target,
                json!({ "ConnectionName": "not-created", "ProviderType": "GitHub" }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (501, "NotImplemented"),
            "case {case:?}: expected the framework's routing refusal for {target:?}, got \
             {status} {body}"
        );
    }

    // Nothing reached a handler, so the store is untouched.
    let (status, body) = call(&emulator, PREFIXES[0].1, "ListConnections", json!({})).await;
    assert_eq!(
        (status, body["Connections"].as_array().map(Vec::len)),
        (200, Some(0)),
        "an unrouted target must create nothing, got {status} {body}"
    );
}
