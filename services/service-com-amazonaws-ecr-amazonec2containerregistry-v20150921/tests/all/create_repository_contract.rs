// SPDX-License-Identifier: CC0-1.0

//! Two rules of this registry's input and output contract, end to end over the wire.
//!
//! 1. A request refused against a constraint the model declares is answered with the
//!    client-input error this service models — `InvalidParameterException`, HTTP 400,
//!    naming the member at fault and the constraint it failed — never with an error code
//!    the called operation does not declare.
//! 2. Every repository reports an `imageScanningConfiguration`, because its `scanOnPush`
//!    member is modelled `@default(false)`: a create that names none stores scanning off
//!    rather than storing nothing, and the create and the describe report the same value.
//!
//! Every case goes through the router, so each exercises routing, the boundary
//! validator, the handler, the persisted state and serialization — the path a client
//! takes.

use serde_json::{json, Value};

use crate::wire::{at, call, emulator, ok};

/// The error code ECR states an input it will not accept with: the client-input error
/// 57 of this model's 58 operations declare, and the only one `CreateRepository`,
/// `DescribeRepositories`, `ListImages` and `PutImage` declare at all.
///
/// `smithy_query(query="digest", model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")`
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The clause a refused `repositoryName` states: the `@pattern` the member declares, as
/// AWS states it — without the `^` / `$` anchoring the model spells it with (AWS states
/// the expression a value must satisfy, not how the model makes the match total) and with
/// the namespace separator bare, because `/` is no regular-expression metacharacter and
/// `\/` matches exactly what `/` does. Spelled out literally rather than computed, so a
/// change in how the service renders an expression shows up here as a failure.
///
/// `smithy_query(query="shape", shape="RepositoryName", model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")`
/// gives `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$`.
const REPOSITORY_NAME_CLAUSE: &str = r"must satisfy regular expression '[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*'";

/// The same clause for `RegistryId`, whose `@pattern` is `^[0-9]{12}$` (same
/// `smithy_query` call, `shape="RegistryId"`).
const REGISTRY_ID_CLAUSE: &str = "must satisfy regular expression '[0-9]{12}'";

/// The clause a value outside an enumerated type states: the whole value set the type
/// declares, in the order it declares them.
///
/// `smithy_query(query="shape", shape="ImageTagMutability", model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")`
/// gives `["MUTABLE", "IMMUTABLE", "IMMUTABLE_WITH_EXCLUSION", "MUTABLE_WITH_EXCLUSION"]`.
const IMAGE_TAG_MUTABILITY_CLAUSE: &str = "Member must satisfy enum value set: \
     [MUTABLE, IMMUTABLE, IMMUTABLE_WITH_EXCLUSION, MUTABLE_WITH_EXCLUSION]";

/// One refusal row: the case name, the operation called, the request, the dotted member
/// path the answer must name, and the constraint clause it must state.
type RefusalCase<'a> = (&'a str, &'a str, Value, &'a str, String);

/// A value refused against a constraint the model declares is refused as
/// `InvalidParameterException` (400), whatever the constraint, the member or the
/// operation — and the refused call changes nothing.
///
/// The rows differ along what the rule generalises over: which constraint failed
/// (`@pattern`, `@length` on a string and on a list, `@range`, `@enum`, `@required`),
/// where the member sits (top level, or an element of a list), and which operation was
/// called. Each bound is exercised on both of its published ends.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_as_an_invalid_parameter() {
    let too_long_name = "a".repeat(257);
    let cases: Vec<RefusalCase<'_>> = vec![
        (
            "an uppercase letter in the name",
            "CreateRepository",
            json!({"repositoryName": "sim2real-behav-ecr-repo-INVALID-Upper"}),
            "repositoryName",
            REPOSITORY_NAME_CLAUSE.to_string(),
        ),
        (
            "a space in the name",
            "CreateRepository",
            json!({"repositoryName": "my app"}),
            "repositoryName",
            REPOSITORY_NAME_CLAUSE.to_string(),
        ),
        (
            "a name ending in a separator",
            "CreateRepository",
            json!({"repositoryName": "my-app-"}),
            "repositoryName",
            REPOSITORY_NAME_CLAUSE.to_string(),
        ),
        (
            "a name opening with the namespace separator",
            "CreateRepository",
            json!({"repositoryName": "/my-app"}),
            "repositoryName",
            REPOSITORY_NAME_CLAUSE.to_string(),
        ),
        (
            "one character — one below the modelled minimum length",
            "CreateRepository",
            json!({"repositoryName": "a"}),
            "repositoryName",
            "Member must have length greater than or equal to 2".to_string(),
        ),
        (
            "257 characters — one above the modelled maximum length",
            "CreateRepository",
            json!({"repositoryName": too_long_name}),
            "repositoryName",
            "Member must have length less than or equal to 256".to_string(),
        ),
        (
            "no repositoryName at all",
            "CreateRepository",
            json!({}),
            "repositoryName",
            "Member must not be null".to_string(),
        ),
        (
            "an eleven-digit registryId",
            "CreateRepository",
            json!({"repositoryName": "well-formed", "registryId": "00000000000"}),
            "registryId",
            REGISTRY_ID_CLAUSE.to_string(),
        ),
        (
            "an imageTagMutability outside the modelled enum",
            "CreateRepository",
            json!({"repositoryName": "well-formed", "imageTagMutability": "SOMETIMES"}),
            "imageTagMutability",
            IMAGE_TAG_MUTABILITY_CLAUSE.to_string(),
        ),
        (
            "six exclusion filters — one above the modelled maximum of five",
            "CreateRepository",
            json!({
                "repositoryName": "well-formed",
                "imageTagMutability": "MUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": (1..=6)
                    .map(|n| json!({"filterType": "WILDCARD", "filter": format!("tag-{n}-*")}))
                    .collect::<Vec<Value>>(),
            }),
            "imageTagMutabilityExclusionFilters",
            "Member must have length less than or equal to 5".to_string(),
        ),
        (
            // The refusal names the parameter the REQUEST carries, not the validating
            // walk's path to the element that carried the value, so a refused element is
            // reported against its list: an element is one value of that same parameter,
            // and ECR validates a repository name itself (the recorded clause for a
            // top-level `repositoryName` is ECR's own, naming no path). A member of a
            // structure NESTED in the request is the other half of the rule and keeps the
            // walk's path — see `parameter_constraint_clauses.rs`. The element step is
            // recognised through `service_common::validation::list_of_element`, whose own
            // documentation states the same precedent — "a service that states a clause
            // at the LIST member when one of its ELEMENTS failed".
            "an invalid name inside a describe's name list",
            "DescribeRepositories",
            json!({"repositoryNames": ["well-formed", "ALSO-Upper"]}),
            "repositoryNames",
            REPOSITORY_NAME_CLAUSE.to_string(),
        ),
        (
            "an empty describe name list — below the modelled minimum of one",
            "DescribeRepositories",
            json!({"repositoryNames": []}),
            "repositoryNames",
            "Member must have length greater than or equal to 1".to_string(),
        ),
        (
            "maxResults below the modelled minimum of one",
            "DescribeRepositories",
            json!({"maxResults": 0}),
            "maxResults",
            "Member must have value greater than or equal to 1".to_string(),
        ),
        (
            "maxResults above the modelled maximum of a thousand",
            "DescribeRepositories",
            json!({"maxResults": 1001}),
            "maxResults",
            "Member must have value less than or equal to 1000".to_string(),
        ),
        (
            "an invalid name in a listing",
            "ListImages",
            json!({"repositoryName": "Bad"}),
            "repositoryName",
            REPOSITORY_NAME_CLAUSE.to_string(),
        ),
        (
            "a push with no imageManifest",
            "PutImage",
            json!({"repositoryName": "well-formed"}),
            "imageManifest",
            "Member must not be null".to_string(),
        ),
    ];

    let router = emulator();

    for (case, operation, request, member, requirement) in cases {
        let answer = call(&router, operation, &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            INVALID_PARAMETER,
            "case {case:?}: expected error code {INVALID_PARAMETER} from {operation}, got {:?}",
            answer.error_code()
        );
        // The whole clause, prefix and closing quote included: the rule is that the
        // refusal names the member at fault and the constraint it failed.
        let expected =
            format!("Invalid parameter at '{member}' failed to satisfy constraint: '{requirement}'");
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected the clause {expected:?}, got {:?}",
            answer.message()
        );
    }

    // A refused create stored nothing: the registry still holds no repository at all.
    let described = ok(&router, "after every refusal", "DescribeRepositories", &json!({})).await;
    assert_eq!(
        at(&described, "/repositories"),
        json!([]),
        "a refused create must not store a repository, got {:?}",
        at(&described, "/repositories")
    );

    // The positive control: a name the pattern admits is still created.
    let created = ok(
        &router,
        "the positive control",
        "CreateRepository",
        &json!({"repositoryName": "sim2real-behav-ecr-repo-lower"}),
    )
    .await;
    assert_eq!(
        at(&created, "/repository/repositoryName"),
        json!("sim2real-behav-ecr-repo-lower"),
        "a well-formed name must still be created, got {:?}",
        at(&created, "/repository/repositoryName")
    );
}

/// Every repository reports an `imageScanningConfiguration`, and the value the create
/// returns is the value the describe reports.
///
/// The rows differ along what the rule generalises over: whether the member is absent,
/// present but empty (so the modelled `scanOnPush` default decides), or present with
/// either value.
#[tokio::test]
async fn a_created_repository_always_reports_its_image_scanning_configuration() {
    // `scanOnPush` is modelled `@default(false)`: "If this parameter is not specified,
    // it will default to false and images will not be scanned unless a scan is manually
    // started." (`smithy_query(query="shape", shape="ImageScanningConfiguration", …)`)
    let cases: Vec<(&str, &str, Option<Value>, bool)> = vec![
        ("no imageScanningConfiguration at all", "scan-absent", None, false),
        (
            "an empty configuration — the modelled scanOnPush default decides",
            "scan-empty",
            Some(json!({})),
            false,
        ),
        (
            "scanning off explicitly",
            "scan-off",
            Some(json!({"scanOnPush": false})),
            false,
        ),
        (
            "scanning on",
            "scan-on",
            Some(json!({"scanOnPush": true})),
            true,
        ),
    ];

    let router = emulator();

    for (case, name, configuration, expected) in cases {
        let mut request = json!({"repositoryName": name});
        if let Some(configuration) = configuration {
            request["imageScanningConfiguration"] = configuration;
        }

        let created = ok(&router, case, "CreateRepository", &request).await;
        assert_eq!(
            at(&created, "/repository/imageScanningConfiguration"),
            json!({"scanOnPush": expected}),
            "case {case:?}: the created repository must report scanOnPush {expected}, got {:?}",
            at(&created, "/repository/imageScanningConfiguration")
        );

        // The round trip: the read reports the stored record, not a value re-derived
        // from the describe request.
        let described = ok(
            &router,
            case,
            "DescribeRepositories",
            &json!({"repositoryNames": [name]}),
        )
        .await;
        assert_eq!(
            at(&described, "/repositories/0/imageScanningConfiguration"),
            json!({"scanOnPush": expected}),
            "case {case:?}: DescribeRepositories must report scanOnPush {expected}, got {:?}",
            at(&described, "/repositories/0/imageScanningConfiguration")
        );
    }
}
