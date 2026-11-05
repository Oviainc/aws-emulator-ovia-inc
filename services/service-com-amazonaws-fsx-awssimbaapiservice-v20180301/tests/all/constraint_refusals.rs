// SPDX-License-Identifier: CC0-1.0

//! RULE: an Amazon FSx request whose input violates a constraint the model declares is
//! refused with the service's common error `BadRequest` at HTTP 400, carrying the
//! standard constraint-violation envelope in the `Message` member that shape declares —
//! and that refusal is reached before the resources the request names are interpreted.
//! A value the member's modelled TYPE cannot be read from is refused one layer earlier,
//! by that type, and so states the generic refusal of that layer instead of a constraint.
//!
//! Once the input is accepted, the resource layer answers for itself: a `ResourceARN`
//! that names a resource the store does not hold is `ResourceNotFound`, whose `Message`
//! is built from the identifier the caller sent.
//!
//! `BadRequest` is one of the two errors the model binds to every FSx operation
//! (`smithy_query(query="digest")`: `common_errors: ["BadRequest",
//! "InternalServerError"]`), and the FSx model declares no `ValidationException` shape at
//! all, so a refusal must never answer with that code nor drop the modelled `Message`.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: AWSSimbaAPIService_v20180301.<Operation>` on an `awsJson1_1` body,
//! dispatched by the real `MultiServiceRouter`.

use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::PatternTrait;
use serde_json::{json, Value};
use service_com_amazonaws_fsx_awssimbaapiservice_v20180301::ops;
use smithy_server_runtime::MultiServiceRouter;

// ONE emulator per suite: the harness that mounts this service beside the owner of subnets
// and reads the account's network back from it comes from the lifecycle suite rather than a
// second copy of it here.
use crate::file_system_lifecycle::{emulator, Emulator};

/// The `X-Amz-Target` prefix from the model's service shape (OPERATIONS.md, "Wire
/// protocol").
const TARGET_PREFIX: &str = "AWSSimbaAPIService_v20180301";

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// A subnet the account has in [`REGION`], as Amazon EC2 — the service that owns subnets,
/// and the one `CreateFileSystem` resolves `SubnetIds` against — reports it. A create below
/// that is meant to SUCCEED must name one, and a create meant to be refused for something
/// else must not be refused for its subnet instead.
fn subnet(router: &Emulator) -> &str {
    router.subnet_a()
}

/// A `ResourceARN` that satisfies the modelled pattern but names a file system that was
/// never created: input validation runs first, so a refusal is about the request, not
/// about what the store holds.
const WELL_FORMED_ARN: &str = "arn:aws:fsx:us-west-2:000000000000:file-system/fs-00000000000000001";

/// An `AssociationId` matching the modelled `DataRepositoryAssociationId` pattern
/// `^(dra-[0-9a-f]{8,})$` with `length` 13..23.
const ASSOCIATION_ID: &str = "dra-0123456789abcdef0";

/// Dispatch one operation and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://fsx.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240102/{REGION}/fsx/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a response: `__type`, reduced to the shape name an SDK matches
/// on (an awsJson code may be namespaced).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The modelled prose member of an FSx error shape, which the model calls `Message`.
fn message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Dispatch `operation` with `body` and assert it failed with HTTP 400, the modelled
/// error `code`, and `expected` as the modelled `Message`.
async fn assert_error(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    code: &str,
    expected: &str,
) -> Value {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {answered}"
    );
    let answered_code = error_code(&answered);
    assert_eq!(
        answered_code, code,
        "case {case:?}: expected the modelled error code {code:?}, got {answered_code:?} in \
         body {answered}"
    );
    let stated = message(&answered);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected Message {expected:?}, got {stated:?}"
    );
    answered
}

/// Dispatch `operation` with `body` and assert it was refused as `BadRequest` (400) with
/// `expected` as the modelled `Message`.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected: &str,
) {
    assert_error(router, case, operation, body, "BadRequest", expected).await;
}

/// RULE's first layer: a value the member's modelled TYPE cannot be read from is refused
/// by that TYPE — before any constraint is considered and with no constraint to name — so
/// the refusal is `BadRequest` with the generic message of that layer.
///
/// The table spans the two forms that layer refuses: a value outside an enumerated type's
/// declared set, and a JSON type the member's target cannot hold.
#[tokio::test]
async fn a_value_the_modelled_type_cannot_hold_is_refused_generically() {
    let router = emulator().await;
    // (case, request) — each otherwise satisfies every constraint.
    let rows: [(&str, Value); 6] = [
        (
            // The recorded instance this issue was filed for.
            "FileSystemType outside WINDOWS | LUSTRE | ONTAP | OPENZFS",
            json!({"FileSystemType": "GLACIER", "SubnetIds": [subnet(&router)], "StorageCapacity": 1200}),
        ),
        (
            "StorageType outside HDD | INTELLIGENT_TIERING | SSD",
            json!({
                "FileSystemType": "WINDOWS",
                "SubnetIds": [subnet(&router)],
                "StorageCapacity": 1200,
                "StorageType": "TAPE",
            }),
        ),
        (
            // Layering: the unreadable value is refused before the constraint one.
            "an unreadable value alongside a constraint violation",
            json!({"FileSystemType": "GLACIER", "SubnetIds": [subnet(&router)], "StorageCapacity": -1}),
        ),
        (
            "a string where StorageCapacity is an integer",
            json!({
                "FileSystemType": "WINDOWS",
                "SubnetIds": [subnet(&router)],
                "StorageCapacity": "twelve hundred",
            }),
        ),
        (
            "a bare string where SubnetIds is a list",
            json!({"FileSystemType": "WINDOWS", "SubnetIds": subnet(&router), "StorageCapacity": 1200}),
        ),
        (
            "a list where FileSystemType is a string",
            json!({"FileSystemType": ["WINDOWS"], "SubnetIds": [subnet(&router)], "StorageCapacity": 1200}),
        ),
    ];

    for (case, request) in rows {
        assert_refused(
            &router,
            case,
            "CreateFileSystem",
            &request,
            "Request failed validation",
        )
        .await;
    }
}

/// RULE, one row per constraint class the model uses, spread over five operations: the
/// refusal names the member and the requirement it failed, in `BadRequest`'s `Message`.
#[tokio::test]
async fn every_constraint_class_is_refused_as_bad_request_naming_the_requirement() {
    let router = emulator().await;

    // (case, operation, request, expected Message)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a range minimum (StorageCapacity, range 0..2147483647)",
            "CreateFileSystem",
            json!({"FileSystemType": "WINDOWS", "SubnetIds": [subnet(&router)], "StorageCapacity": -1}),
            "1 validation error detected: Value at 'storageCapacity' failed to satisfy \
             constraint: Member must have value greater than or equal to 0"
                .to_string(),
        ),
        (
            "an absent @required member (FileSystemType)",
            "CreateFileSystem",
            json!({"SubnetIds": [subnet(&router)], "StorageCapacity": 1200}),
            "1 validation error detected: Value null at 'fileSystemType' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "a @pattern (ClientRequestToken, ^[A-za-z0-9_.-]{0,63}$)",
            "CreateFileSystem",
            json!({
                "FileSystemType": "LUSTRE",
                "SubnetIds": [subnet(&router)],
                "StorageCapacity": 1200,
                "ClientRequestToken": "token with spaces",
            }),
            "1 validation error detected: Value at 'clientRequestToken' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: \
             ^[A-za-z0-9_.-]{0,63}$"
                .to_string(),
        ),
        (
            "a @pattern on an identifier (FileSystemId, ^(fs-[0-9a-f]{8,})$)",
            "DeleteFileSystem",
            json!({"FileSystemId": "fs-ZZZZZZZZZZZZ"}),
            "1 validation error detected: Value at 'fileSystemId' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: ^(fs-[0-9a-f]{8,})$"
                .to_string(),
        ),
        (
            "a range minimum on a paging member (MaxResults, range 1..2147483647)",
            "ListTagsForResource",
            json!({"ResourceARN": WELL_FORMED_ARN, "MaxResults": 0}),
            "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: \
             Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            // The recorded instance this issue was filed for.
            "a range minimum (ImportedFileChunkSize, Megabytes range 1..512000)",
            "UpdateDataRepositoryAssociation",
            json!({"AssociationId": ASSOCIATION_ID, "ImportedFileChunkSize": 0}),
            "1 validation error detected: Value at 'importedFileChunkSize' failed to satisfy \
             constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "the same range's maximum (ImportedFileChunkSize)",
            "UpdateDataRepositoryAssociation",
            json!({"AssociationId": ASSOCIATION_ID, "ImportedFileChunkSize": 512_001}),
            "1 validation error detected: Value at 'importedFileChunkSize' failed to satisfy \
             constraint: Member must have value less than or equal to 512000"
                .to_string(),
        ),
        (
            "a collection @length maximum (Tags, length 1..50)",
            "TagResource",
            json!({
                "ResourceARN": WELL_FORMED_ARN,
                "Tags": (0..51)
                    .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
                    .collect::<Vec<Value>>(),
            }),
            "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member \
             must have length less than or equal to 50"
                .to_string(),
        ),
        (
            "a collection @length minimum (TagKeys, length 1..50)",
            "UntagResource",
            json!({"ResourceARN": WELL_FORMED_ARN, "TagKeys": []}),
            "1 validation error detected: Value at 'tagKeys' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
                .to_string(),
        ),
    ];

    for (case, operation, request, expected) in rows {
        assert_refused(&router, case, operation, &request, &expected).await;
    }
}

/// RULE: a request violating SEVERAL constraints states every one of them, counted, in
/// the one envelope.
#[tokio::test]
async fn several_violations_are_counted_in_one_envelope() {
    let router = emulator().await;
    // `fs-0` is shorter than the modelled `length` minimum of 11 AND outside the
    // `@pattern`, which wants at least eight hex digits.
    let (status, answered) = call(
        &router,
        "DeleteFileSystem",
        &json!({"FileSystemId": "fs-0"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400, got {status} with body {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "BadRequest",
        "expected BadRequest, got body {answered}"
    );
    let stated = message(&answered);
    let Some(clauses) = stated.strip_prefix("2 validation errors detected: ") else {
        panic!("expected a counted envelope for two violations, got {stated:?}");
    };
    let mut stated_clauses: Vec<&str> = clauses.split("; ").collect();
    stated_clauses.sort_unstable();
    let mut expected = vec![
        "Value at 'fileSystemId' failed to satisfy constraint: Member must have length greater \
         than or equal to 11",
        "Value at 'fileSystemId' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: ^(fs-[0-9a-f]{8,})$",
    ];
    expected.sort_unstable();
    assert_eq!(
        stated_clauses, expected,
        "expected both clauses, got {stated:?}"
    );
}

/// RULE: a member outside the modelled `@pattern` is refused as `BadRequest` naming that
/// pattern, for every operation that binds one — the value is never interpreted, so the
/// answer is never a resource-dispatch error.
///
/// The table spans the four operations that bind the look-around ARN pattern (the three
/// tagging ones through `ResourceARN`, `CopySnapshotAndUpdateVolume` through
/// `SourceSnapshotARN`) and the forms a malformed value takes: not an ARN at all, another
/// service's ARN, a bad account segment, and a resource portion the pattern refuses. The
/// expected pattern is read off the generated input schema, so this asserts the MODEL's
/// pattern rather than a copy of it.
#[tokio::test]
async fn a_member_outside_its_pattern_is_refused_before_the_value_is_interpreted() {
    let router = emulator().await;

    // Values that satisfy the ARN's @length (8..512) — so the boundary validator passes
    // them — and not its @pattern.
    let malformed: [(&str, &str); 4] = [
        ("not an arn at all", "not-an-arn-at-all"),
        (
            "another service's resource",
            "arn:aws:ec2:us-west-2:000000000000:instance/i-0123456789abcdef0",
        ),
        (
            "an account segment that is not 12 digits",
            "arn:aws:fsx:us-west-2:12345:file-system/fs-0123456789abcdef0",
        ),
        (
            "a resource portion starting with a slash",
            "arn:aws:fsx:us-west-2:000000000000://file-system/fs-0123456789abcdef0",
        ),
    ];
    // (operation, the member that binds the pattern, a request whose other members are
    // valid so the ARN is the only thing refused)
    type Request = fn(&str) -> Value;
    let operations: [(&str, &str, &'static Schema, Request); 4] = [
        (
            "ListTagsForResource",
            "ResourceARN",
            ops::types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
            |arn| json!({"ResourceARN": arn}),
        ),
        (
            "TagResource",
            "ResourceARN",
            ops::types::TAG_RESOURCE_INPUT_SCHEMA,
            |arn| json!({"ResourceARN": arn, "Tags": [{"Key": "k", "Value": "v"}]}),
        ),
        (
            "UntagResource",
            "ResourceARN",
            ops::types::UNTAG_RESOURCE_INPUT_SCHEMA,
            |arn| json!({"ResourceARN": arn, "TagKeys": ["k"]}),
        ),
        (
            // Not a tagging operation and still a 501 stub: the refusal is the request
            // layer's, so it does not depend on the handler at all.
            "CopySnapshotAndUpdateVolume",
            "SourceSnapshotARN",
            ops::types::COPY_SNAPSHOT_AND_UPDATE_VOLUME_INPUT_SCHEMA,
            |arn| json!({"VolumeId": "fsvol-01234567890123456", "SourceSnapshotARN": arn}),
        ),
    ];

    for (operation, member, schema, request) in operations {
        let expected = format!(
            "1 validation error detected: Value at '{}' failed to satisfy constraint: Member \
             must satisfy regular expression pattern: {}",
            lower_camel(member),
            modelled_pattern(schema, member)
        );
        for (form, arn) in malformed {
            let case = format!("{operation} with {form}");
            assert_refused(&router, &case, operation, &request(arn), &expected).await;
        }
    }
}

/// RULE: a request violating constraints from BOTH walks — one the framework's validator
/// enforces and one only the look-around engine can — states both clauses in the one
/// counted envelope, as a single refusal of a single request.
#[tokio::test]
async fn violations_from_both_walks_share_one_envelope() {
    let router = emulator().await;
    let pattern = modelled_pattern(ops::types::TAG_RESOURCE_INPUT_SCHEMA, "ResourceARN");
    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({
            // Outside the look-around @pattern, which only this service's engine sees …
            "ResourceARN": "not-an-arn-at-all",
            // … and past the modelled `Tags` @length maximum, which the framework sees.
            "Tags": (0..51)
                .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400, got {status} with body {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "BadRequest",
        "expected BadRequest, got body {answered}"
    );
    let stated = message(&answered);
    let Some(clauses) = stated.strip_prefix("2 validation errors detected: ") else {
        panic!("expected a counted envelope for two violations, got {stated:?}");
    };
    let mut stated_clauses: Vec<&str> = clauses.split("; ").collect();
    stated_clauses.sort_unstable();
    let tags_clause = "Value at 'tags' failed to satisfy constraint: Member must have length \
                       less than or equal to 50"
        .to_string();
    let arn_clause = format!(
        "Value at 'resourceARN' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {pattern}"
    );
    let mut expected = vec![arn_clause.as_str(), tags_clause.as_str()];
    expected.sort_unstable();
    assert_eq!(
        stated_clauses, expected,
        "expected one clause from each walk, got {stated:?}"
    );
}

/// RULE: once the ARN is well formed and names a resource type FSx owns, a resource the
/// store does not hold is `ResourceNotFound`, whose `Message` is built from the
/// IDENTIFIER the caller sent — `Resource '<resource-id>' does not exist` — so a caller
/// learns which resource was missing.
///
/// The table spans the three operations that take a `ResourceARN` and the resource types
/// an ARN may name, which is what the rule generalises over.
#[tokio::test]
async fn a_not_found_refusal_names_the_resource_the_request_asked_about() {
    let router = emulator().await;
    // (resource type, the id in the ARN) — `file-system` and `backup` are both in the
    // modelled set of FSx resource types, and neither id is in the store.
    let resources: [(&str, &str); 3] = [
        ("file-system", "fs-0123456789abcdef0"),
        ("backup", "backup-0123456789abcdef0"),
        ("volume", "fsvol-0123456789abcdef0"),
    ];
    type Request = fn(&str) -> Value;
    let operations: [(&str, Request); 3] = [
        ("ListTagsForResource", |arn| json!({"ResourceARN": arn})),
        ("TagResource", |arn| {
            json!({"ResourceARN": arn, "Tags": [{"Key": "k", "Value": "v"}]})
        }),
        ("UntagResource", |arn| {
            json!({"ResourceARN": arn, "TagKeys": ["k"]})
        }),
    ];

    for (operation, request) in operations {
        for (resource_type, identifier) in resources {
            let arn = format!("arn:aws:fsx:{REGION}:000000000000:{resource_type}/{identifier}");
            let case = format!("{operation} on an absent {resource_type}");
            let answered = assert_error(
                &router,
                &case,
                operation,
                &request(&arn),
                "ResourceNotFound",
                &format!("Resource '{identifier}' does not exist"),
            )
            .await;
            // The modelled `ResourceARN` member of the error echoes what was asked about.
            assert_eq!(
                answered["ResourceARN"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected the refusal to carry ResourceARN {arn:?}, got \
                 {answered}"
            );
        }
    }
}

/// RULE applied to the WHOLE surface: no operation of this service may answer the
/// framework's cross-service `ValidationException`, a shape the FSx model does not
/// declare — and a refusal that is a 400 always carries the modelled `Message`.
///
/// The list is every operation the model binds, which is what makes this a check on the
/// surface rather than on a sample: SOURCE
/// `smithy_query(query="digest", model_path="services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/smithy-model/com-amazonaws-fsx-awssimbaapiservice-v20180301.json")`,
/// whose `operation_count` is 48 and whose `common_errors` are `BadRequest` and
/// `InternalServerError`. An empty body is enough to reach the refusal layer: an
/// operation with a `@required` member is refused, and one without is served.
#[tokio::test]
async fn no_operation_answers_a_foreign_validation_exception() {
    let router = emulator().await;
    let operations: [&str; 48] = [
        "AssociateFileSystemAliases",
        "CancelDataRepositoryTask",
        "CopyBackup",
        "CopySnapshotAndUpdateVolume",
        "CreateAndAttachS3AccessPoint",
        "CreateBackup",
        "CreateDataRepositoryAssociation",
        "CreateDataRepositoryTask",
        "CreateFileCache",
        "CreateFileSystem",
        "CreateFileSystemFromBackup",
        "CreateSnapshot",
        "CreateStorageVirtualMachine",
        "CreateVolume",
        "CreateVolumeFromBackup",
        "DeleteBackup",
        "DeleteDataRepositoryAssociation",
        "DeleteFileCache",
        "DeleteFileSystem",
        "DeleteSnapshot",
        "DeleteStorageVirtualMachine",
        "DeleteVolume",
        "DescribeBackups",
        "DescribeDataRepositoryAssociations",
        "DescribeDataRepositoryTasks",
        "DescribeFileCaches",
        "DescribeFileSystemAliases",
        "DescribeFileSystems",
        "DescribeS3AccessPointAttachments",
        "DescribeSharedVpcConfiguration",
        "DescribeSnapshots",
        "DescribeStorageVirtualMachines",
        "DescribeVolumes",
        "DetachAndDeleteS3AccessPoint",
        "DisassociateFileSystemAliases",
        "ListTagsForResource",
        "ReleaseFileSystemNfsV3Locks",
        "RestoreVolumeFromSnapshot",
        "StartMisconfiguredStateRecovery",
        "TagResource",
        "UntagResource",
        "UpdateDataRepositoryAssociation",
        "UpdateFileCache",
        "UpdateFileSystem",
        "UpdateSharedVpcConfiguration",
        "UpdateSnapshot",
        "UpdateStorageVirtualMachine",
        "UpdateVolume",
    ];

    for operation in operations {
        let (status, answered) = call(&router, operation, &json!({})).await;
        let code = error_code(&answered);
        assert_ne!(
            code, "ValidationException",
            "case {operation:?}: the FSx model declares no ValidationException, got HTTP \
             {status} with body {answered}"
        );
        if status == 400 {
            assert_eq!(
                code, "BadRequest",
                "case {operation:?}: an empty body is refused as BadRequest, got {code:?} in \
                 body {answered}"
            );
            assert!(
                !message(&answered).is_empty(),
                "case {operation:?}: a refusal must carry the modelled Message, got body \
                 {answered}"
            );
        }
    }
}

/// RULE's other side: a request inside every modelled constraint is SERVED, not refused —
/// the tag round-trip still reads back what it wrote, and an operation that is not
/// implemented yet still answers 501 rather than a fabricated refusal.
#[tokio::test]
async fn a_request_inside_every_constraint_is_served() {
    let router = emulator().await;

    let (status, created) = call(
        &router,
        "CreateFileSystem",
        &json!({
            "FileSystemType": "WINDOWS",
            "SubnetIds": [subnet(&router)],
            "StorageCapacity": 1200,
            "ClientRequestToken": "token-1",
            "Tags": [{"Key": "Name", "Value": "one"}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a create inside every constraint must succeed, got {status} with body {created}"
    );
    let arn = created["FileSystem"]["ResourceARN"]
        .as_str()
        .expect("a created file system carries its ResourceARN")
        .to_string();

    // The ARN this service MINTS satisfies the modelled pattern: the gate above cannot
    // refuse what the service itself hands out.
    let (status, tagged) = call(
        &router,
        "TagResource",
        &json!({"ResourceARN": arn, "Tags": [{"Key": "team", "Value": "storage"}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "tagging {arn} must succeed, got {status} with body {tagged}"
    );

    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({"ResourceARN": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "listing the tags of {arn} must succeed, got {status} with body {listed}"
    );
    let mut pairs: Vec<(String, String)> = listed["Tags"]
        .as_array()
        .expect("Tags must be a list")
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    pairs.sort();
    assert_eq!(
        pairs,
        vec![
            ("Name".to_string(), "one".to_string()),
            ("team".to_string(), "storage".to_string()),
        ],
        "the tags read back must be the ones written, got {listed}"
    );

    // An operation whose handler is still scaffolded keeps answering 501 for a request
    // that violates nothing: the refusal layer states constraint violations only.
    let (status, answered) = call(
        &router,
        "UpdateDataRepositoryAssociation",
        &json!({"AssociationId": ASSOCIATION_ID, "ImportedFileChunkSize": 1024}),
    )
    .await;
    assert_eq!(
        status, 501,
        "a valid request to a scaffolded operation must stay 501, got {status} with body \
         {answered}"
    );
}

/// The `@pattern` a member of `schema` declares, read off the generated input schema, so
/// an expected message names the MODEL's pattern rather than a copy of it.
fn modelled_pattern(schema: &'static Schema, member_name: &str) -> &'static str {
    let member = schema
        .members()
        .iter()
        .find(|member| member.member_name() == Some(member_name))
        .unwrap_or_else(|| panic!("the input shape declares a {member_name} member"));
    member
        .traits()
        .and_then(|traits| traits.get(&PatternTrait::TRAIT_ID))
        .and_then(|declared| declared.as_any().downcast_ref::<PatternTrait>())
        .unwrap_or_else(|| panic!("{member_name} declares @pattern"))
        .regex
}

/// The lowerCamel form a validation message names a member by.
fn lower_camel(member_name: &str) -> String {
    let mut characters = member_name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}
