// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT Software Package Catalog's VERSIONS:
//! `CreatePackageVersion` and the `GetPackageVersion` reader that proves what it
//! persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what an
//! AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: a create of a SUB-RESOURCE records it under the pair of names
//! that identifies it, inside the parent resource that holds it, and answers with the
//! record it persisted; the single-resource reader reports that same record member for
//! member and stably across reads; a create whose parent does not exist, or whose own
//! name the parent already holds, is refused with the operation's modelled error and
//! changes nothing (unless it repeats the `@idempotencyToken` that minted the record,
//! when it is that one request arriving twice); and a pair the store does not hold is
//! refused with the reader's modelled not-found error instead of a success. Here that
//! means `CreatePackageVersion` records a version under `(packageName, versionName)`
//! and answers `packageVersionArn`
//! (`arn:aws:iot:<region>:<account>:package/<packageName>/version/<versionName>`) /
//! `packageName` / `versionName` / `description` / `attributes` / `status`, the status
//! being the `DRAFT` the package version lifecycle is entered at, and
//! `GetPackageVersion` reports those plus the `artifact`, `recipe` and the
//! `creationDate` / `lastModifiedDate` the create stamped, with no `sbom` (an SBOM is
//! attached by `AssociateSbomWithPackageVersion`, not by a create) and no
//! `errorReason`.
//!
//! The table spans what that rule generalises over — the form and length of a version
//! name, whether the create carries a description and how long it is, how many
//! attributes and tags it carries, whether it carries an artifact and a recipe and how
//! long the recipe is, whether it carries a `clientToken`, and the region the call is
//! signed for — rather than one recorded request.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    MAX_PACKAGE_VERSIONS, NEW_PACKAGE_VERSION_STATUS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// A timestamp member as the epoch seconds the restJson1 protocol renders a
    /// timestamp as.
    fn epoch(&self, name: &str) -> Option<f64> {
        self.body.get(name).and_then(Value::as_f64)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `CreatePackage`: `PUT /packages/{packageName}` — the parent a version hangs off.
    async fn create_package_in(&self, package: &str, region: &str) -> Reply {
        self.call("PUT", &format!("/packages/{package}"), Some(&json!({})), region)
            .await
    }

    async fn create_package(&self, package: &str) -> Reply {
        self.create_package_in(package, REGION).await
    }

    /// `CreatePackageVersion`: `PUT /packages/{packageName}/versions/{versionName}`,
    /// with `clientToken` bound to the query string
    /// (`smithy_query(query="operation",operation="CreatePackageVersion")`).
    async fn create_version_in(
        &self,
        package: &str,
        version: &str,
        body: &Value,
        client_token: Option<&str>,
        region: &str,
    ) -> Reply {
        let path = match client_token {
            Some(token) => format!("/packages/{package}/versions/{version}?clientToken={token}"),
            None => format!("/packages/{package}/versions/{version}"),
        };
        self.call("PUT", &path, Some(body), region).await
    }

    async fn create_version(&self, package: &str, version: &str, body: &Value) -> Reply {
        self.create_version_in(package, version, body, None, REGION)
            .await
    }

    /// `GetPackageVersion`: `GET /packages/{packageName}/versions/{versionName}`.
    async fn get_version_in(&self, package: &str, version: &str, region: &str) -> Reply {
        self.call(
            "GET",
            &format!("/packages/{package}/versions/{version}"),
            None,
            region,
        )
        .await
    }

    async fn get_version(&self, package: &str, version: &str) -> Reply {
        self.get_version_in(package, version, REGION).await
    }
}

/// The ARN a version of `package` created in `region` must carry.
fn expected_arn(region: &str, package: &str, version: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:package/{package}/version/{version}")
}

/// Epoch seconds now, for bounding a reported timestamp.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A `ResourceAttributes` body member carrying `count` distinct keys. Its key is
/// modelled `length 1.., pattern ^[a-zA-Z0-9:_-]+$` and its value
/// `length 1.., pattern ^[^\p{C}]+$`
/// (`smithy_query(query="shape",shape="ResourceAttributes")`), so the keys below use
/// every character class the key pattern allows.
fn attributes(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(
            format!("attr:key_{index}-A"),
            Value::from(format!("value {index}")),
        );
    }
    Value::Object(map)
}

/// A `TagMap` body member carrying `count` distinct keys. `TagMap` has modelled
/// `length 1..50` (`smithy_query(query="shape",shape="TagMap")`), so a request either
/// carries at least one tag or carries the member not at all.
fn tags(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("key-{index}"), Value::from(format!("value-{index}")));
    }
    Value::Object(map)
}

/// One create request: a label, the names that identify the version, the region it is
/// signed for, the request body, the `clientToken` it carries, and what every later
/// read must report for the members the body supplied.
struct Case {
    label: &'static str,
    package: String,
    version: String,
    region: &'static str,
    body: Value,
    client_token: Option<String>,
    /// What `description` must report: the body's value, or `None` when the create
    /// carried none.
    description: Option<String>,
    /// What `attributes` must report: the body's value, or `None` when the create
    /// carried none.
    expected_attributes: Option<Value>,
    /// What `artifact` must report: the body's value, or `None` when the create carried
    /// none.
    expected_artifact: Option<Value>,
    /// What `recipe` must report: the body's value, or `None` when the create carried
    /// none.
    recipe: Option<String>,
}

/// The table every case list below is built from. It spans what the rule generalises
/// over:
///
/// * the form and length of `versionName` — modelled
///   `length 1..64, pattern ^[a-zA-Z0-9-_.]+$`
///   (`smithy_query(query="operation",operation="CreatePackageVersion")`), so the rows
///   include the shortest legal name, the longest, and one using every character class
///   the pattern allows;
/// * whether the create carries a `description` and how long it is — modelled
///   `ResourceDescription`, `length 0..1024, pattern ^[^\p{C}]+$`, so the rows cover the
///   member absent, a one-character description and the longest legal one;
/// * how many `attributes` it carries — none at all, one, and several;
/// * whether it carries an `artifact`, and whether that artifact's `s3Location` names a
///   bucket version;
/// * whether it carries a `recipe` and how long it is — modelled
///   `PackageVersionRecipe`, `length 0..3072`, so the rows cover absent, one character
///   and the longest legal one;
/// * how many `tags` it carries — none at all, one, and the modelled maximum of 50;
/// * whether it carries a `clientToken` — the `@idempotencyToken`, `length 36..64,
///   pattern ^\S{36,64}$`, so the rows cover absent, the shortest legal token and the
///   longest;
/// * the region the call is signed for, which is the region the version's ARN is scoped
///   to.
fn cases() -> Vec<Case> {
    let artifact = json!({ "s3Location": { "bucket": "ota-bucket", "key": "firmware/v1.bin" } });
    let versioned_artifact = json!({
        "s3Location": { "bucket": "ota-bucket", "key": "firmware/v2.bin", "version": "3HL4kqtJl" }
    });
    vec![
        Case {
            label: "only the required names",
            package: "minimal-package".to_string(),
            version: "1.0.0".to_string(),
            region: REGION,
            body: json!({}),
            client_token: None,
            description: None,
            expected_attributes: None,
            expected_artifact: None,
            recipe: None,
        },
        Case {
            label: "a one-character description, one attribute and one tag",
            package: "described-package".to_string(),
            version: "v".to_string(),
            region: REGION,
            body: json!({ "description": "x", "attributes": attributes(1), "tags": tags(1) }),
            client_token: None,
            description: Some("x".to_string()),
            expected_attributes: Some(attributes(1)),
            expected_artifact: None,
            recipe: None,
        },
        Case {
            label: "the longest legal description and recipe, with the maximum of 50 tags",
            package: "big-package".to_string(),
            version: "longest.payload-1_0".to_string(),
            region: REGION,
            body: json!({
                "description": "d".repeat(1024),
                "recipe": "r".repeat(3072),
                "tags": tags(50),
            }),
            client_token: None,
            description: Some("d".repeat(1024)),
            expected_attributes: None,
            expected_artifact: None,
            recipe: Some("r".repeat(3072)),
        },
        Case {
            label: "an artifact in S3 and several attributes",
            package: "artifact-package".to_string(),
            version: "Ver_01-aZ.9".to_string(),
            region: REGION,
            body: json!({ "artifact": artifact.clone(), "attributes": attributes(4) }),
            client_token: None,
            description: None,
            expected_attributes: Some(attributes(4)),
            expected_artifact: Some(artifact.clone()),
            recipe: None,
        },
        Case {
            label: "a versioned S3 artifact, a one-character recipe and the shortest token",
            package: "versioned-artifact-package".to_string(),
            version: "2.0.0-rc1".to_string(),
            region: REGION,
            body: json!({ "artifact": versioned_artifact.clone(), "recipe": "r" }),
            client_token: Some("t".repeat(36)),
            description: None,
            expected_attributes: None,
            expected_artifact: Some(versioned_artifact.clone()),
            recipe: Some("r".to_string()),
        },
        Case {
            label: "the longest legal client token, signed for another region",
            package: "other-region-package".to_string(),
            version: "3.1.4".to_string(),
            region: "eu-west-1",
            body: json!({ "description": "signed elsewhere", "tags": tags(3) }),
            client_token: Some("T".repeat(64)),
            description: Some("signed elsewhere".to_string()),
            expected_attributes: None,
            expected_artifact: None,
            recipe: None,
        },
        Case {
            label: "the shortest legal version name",
            package: "shortest-version-package".to_string(),
            version: "a".to_string(),
            region: "ap-southeast-2",
            body: json!({ "attributes": attributes(2), "recipe": "{\"steps\":[]}" }),
            client_token: None,
            description: None,
            expected_attributes: Some(attributes(2)),
            expected_artifact: None,
            recipe: Some("{\"steps\":[]}".to_string()),
        },
        Case {
            label: "the longest legal version name",
            package: "longest-version-package".to_string(),
            version: "v".repeat(64),
            region: REGION,
            body: json!({ "description": "longest name", "artifact": artifact.clone() }),
            client_token: Some(format!("{}-{}", "L".repeat(30), "l".repeat(9))),
            description: Some("longest name".to_string()),
            expected_attributes: None,
            expected_artifact: Some(artifact),
            recipe: None,
        },
    ]
}

#[tokio::test]
async fn create_package_version_records_what_get_package_version_reports() {
    let emulator = Emulator::new();

    for case in cases() {
        let Case {
            label,
            package,
            version,
            region,
            body,
            client_token,
            description,
            expected_attributes,
            expected_artifact,
            recipe,
        } = case;

        let parent = emulator.create_package_in(&package, region).await;
        assert_eq!(
            parent.status, 200,
            "case {label:?}: the package a version hangs off must be creatable, got {parent:?}"
        );

        let before = now_epoch_seconds();
        let created = emulator
            .create_version_in(&package, &version, &body, client_token.as_deref(), region)
            .await;
        let after = now_epoch_seconds();

        assert_eq!(
            created.status, 200,
            "case {label:?}: a create must answer 200, got {created:?}"
        );
        assert_eq!(
            created.text("packageName"),
            Some(package.clone()),
            "case {label:?}: the create must echo the package name, got {created:?}"
        );
        assert_eq!(
            created.text("versionName"),
            Some(version.clone()),
            "case {label:?}: the create must echo the version name, got {created:?}"
        );
        assert_eq!(
            created.text("packageVersionArn"),
            Some(expected_arn(region, &package, &version)),
            "case {label:?}: expected {:?}, got {created:?}",
            expected_arn(region, &package, &version)
        );
        assert_eq!(
            created.text("description"),
            description,
            "case {label:?}: the create must report the description it stored, got {created:?}"
        );
        assert_eq!(
            created.body.get("attributes").cloned(),
            expected_attributes.clone(),
            "case {label:?}: the create must report the attributes it stored, got {created:?}"
        );
        // A new version enters the package version lifecycle at `DRAFT`; `PUBLISHED` /
        // `DEPRECATED` are reached through `UpdatePackageVersion$action`.
        assert_eq!(
            created.text("status").as_deref(),
            Some(NEW_PACKAGE_VERSION_STATUS),
            "case {label:?}: expected status {NEW_PACKAGE_VERSION_STATUS:?}, got {created:?}"
        );
        assert!(
            created.body.get("errorReason").is_none(),
            "case {label:?}: a create that succeeded has no error reason, got {created:?}"
        );

        let read = emulator.get_version_in(&package, &version, region).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the created version must be readable, got {read:?}"
        );
        assert_eq!(
            read.text("packageName"),
            Some(package.clone()),
            "case {label:?}: the reader must report the stored package name, got {read:?}"
        );
        assert_eq!(
            read.text("versionName"),
            Some(version.clone()),
            "case {label:?}: the reader must report the stored version name, got {read:?}"
        );
        assert_eq!(
            read.text("packageVersionArn"),
            created.text("packageVersionArn"),
            "case {label:?}: the reader must report the ARN the create minted, got {read:?}"
        );
        assert_eq!(
            read.text("description"),
            description,
            "case {label:?}: the reader must report the stored description, got {read:?}"
        );
        assert_eq!(
            read.body.get("attributes").cloned(),
            expected_attributes,
            "case {label:?}: the reader must report the stored attributes, got {read:?}"
        );
        assert_eq!(
            read.body.get("artifact").cloned(),
            expected_artifact,
            "case {label:?}: the reader must report the stored artifact, got {read:?}"
        );
        assert_eq!(
            read.text("recipe"),
            recipe,
            "case {label:?}: the reader must report the stored recipe, got {read:?}"
        );
        assert_eq!(
            read.text("status").as_deref(),
            Some(NEW_PACKAGE_VERSION_STATUS),
            "case {label:?}: the reader must report the stored status, got {read:?}"
        );
        // An SBOM is attached by `AssociateSbomWithPackageVersion`, so a created
        // version carries neither it nor its validation status.
        assert!(
            read.body.get("sbom").is_none() && read.body.get("sbomValidationStatus").is_none(),
            "case {label:?}: a created version has no software bill of materials, got {read:?}"
        );

        let creation_date = read.epoch("creationDate").unwrap_or_else(|| {
            panic!("case {label:?}: the reader must report a creationDate, got {read:?}")
        });
        let last_modified = read.epoch("lastModifiedDate").unwrap_or_else(|| {
            panic!("case {label:?}: the reader must report a lastModifiedDate, got {read:?}")
        });
        assert!(
            creation_date >= before.floor() && creation_date <= after.ceil(),
            "case {label:?}: creationDate {creation_date} must lie in [{before}, {after}]"
        );
        assert!(
            (last_modified - creation_date).abs() < f64::EPSILON,
            "case {label:?}: a version nobody updated was last modified when it was \
             created, got {last_modified} against {creation_date}"
        );

        // Reading does not change what a later caller sees.
        let again = emulator.get_version_in(&package, &version, region).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: a second read must report the same record, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_version_name_the_package_already_holds_is_a_conflict() {
    let emulator = Emulator::new();

    for case in cases() {
        let Case {
            label,
            package,
            version,
            region,
            body,
            client_token,
            description,
            ..
        } = case;

        assert_eq!(
            emulator.create_package_in(&package, region).await.status,
            200,
            "case {label:?}: the package must be creatable"
        );
        let created = emulator
            .create_version_in(&package, &version, &body, client_token.as_deref(), region)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the first create must succeed, got {created:?}"
        );

        // A second create of the same version, with a DIFFERENT idempotency token (and
        // a different description), is refused 409 and overwrites nothing.
        let other_token = "Z".repeat(40);
        let refused = emulator
            .create_version_in(
                &package,
                &version,
                &json!({ "description": "a different version" }),
                Some(&other_token),
                region,
            )
            .await;
        assert_eq!(
            refused.status, 409,
            "case {label:?}: a repeat create must be refused 409, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {label:?}: expected ConflictException, got {refused:?}"
        );

        let read = emulator.get_version_in(&package, &version, region).await;
        assert_eq!(
            read.text("description"),
            description,
            "case {label:?}: the refused create must change nothing, got {read:?}"
        );
        assert_eq!(
            read.text("packageVersionArn"),
            Some(expected_arn(region, &package, &version)),
            "case {label:?}: the refused create must leave the ARN alone, got {read:?}"
        );

        // Repeating the create that minted the record — same names, same client token —
        // is that one request arriving twice: it answers from the record.
        if let Some(token) = client_token.as_deref() {
            let repeated = emulator
                .create_version_in(&package, &version, &body, Some(token), region)
                .await;
            assert_eq!(
                repeated.status, 200,
                "case {label:?}: the same request repeated must answer 200, got {repeated:?}"
            );
            assert_eq!(
                repeated.body, created.body,
                "case {label:?}: the same request repeated must answer the same record, \
                 got {repeated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_version_of_a_package_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();

    // A version hangs off an EXISTING package, so names of every legal form with no
    // package created are refused with the modelled 400 and record nothing. This
    // operation models no not-found error, so `ValidationException` is the refusal.
    let absent = [
        "a".to_string(),
        "never-created".to_string(),
        "Never_Created.1".to_string(),
        "n".repeat(128),
    ];
    for package in &absent {
        let refused = emulator.create_version(package, "1.0.0", &json!({})).await;
        assert_eq!(
            refused.status, 400,
            "case {package:?}: a version of an uncreated package must be refused 400, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {package:?}: expected ValidationException, got {refused:?}"
        );
        let read = emulator.get_version(package, "1.0.0").await;
        assert_eq!(
            read.status, 404,
            "case {package:?}: the refused create must record no version, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {package:?}: expected ResourceNotFoundException, got {read:?}"
        );
    }

    // Creating the package makes exactly that one accept a version, and leaves the rest
    // refused.
    assert_eq!(
        emulator.create_package(&absent[1]).await.status,
        200,
        "the package must be creatable"
    );
    for package in &absent {
        let created = emulator.create_version(package, "1.0.0", &json!({})).await;
        let expected = if package == &absent[1] { 200 } else { 400 };
        assert_eq!(
            created.status, expected,
            "case {package:?}: expected {expected}, got {created:?}"
        );
    }
}

#[tokio::test]
async fn a_version_the_package_does_not_hold_is_not_found() {
    let emulator = Emulator::new();

    assert_eq!(
        emulator.create_package("reader-package").await.status,
        200,
        "the package must be creatable"
    );

    // Version names of every legal form, none of them created: the reader refuses each
    // with the modelled 404 rather than a hollow 200.
    let absent = [
        "a".to_string(),
        "never-created".to_string(),
        "Never_Created.1".to_string(),
        "v".repeat(64),
    ];
    for version in &absent {
        let read = emulator.get_version("reader-package", version).await;
        assert_eq!(
            read.status, 404,
            "case {version:?}: an uncreated version must be refused 404, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {version:?}: expected ResourceNotFoundException, got {read:?}"
        );
    }

    let created = emulator
        .create_version("reader-package", &absent[1], &json!({}))
        .await;
    assert_eq!(
        created.status, 200,
        "a create of an absent version must succeed, got {created:?}"
    );
    for version in &absent {
        let read = emulator.get_version("reader-package", version).await;
        let expected = if version == &absent[1] { 200 } else { 404 };
        assert_eq!(
            read.status, expected,
            "case {version:?}: expected {expected}, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_version_belongs_to_the_package_it_was_created_under() {
    let emulator = Emulator::new();

    // A version's identity is the PAIR of names, so the same version name in two
    // packages is two records, each readable only under its own package.
    let packages = ["left-package", "right-package", "third.package"];
    for package in packages {
        assert_eq!(
            emulator.create_package(package).await.status,
            200,
            "case {package:?}: the package must be creatable"
        );
    }

    for (index, package) in packages.iter().enumerate() {
        let created = emulator
            .create_version(
                package,
                "1.0.0",
                &json!({ "description": format!("version of {package}") }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {package:?}: a create must succeed, got {created:?}"
        );
        assert_eq!(
            created.text("packageVersionArn"),
            Some(expected_arn(REGION, package, "1.0.0")),
            "case {package:?}: each version's ARN names its own package, got {created:?}"
        );
        // Only the packages created so far hold the version.
        for (other_index, other) in packages.iter().enumerate() {
            let read = emulator.get_version(other, "1.0.0").await;
            let expected = if other_index <= index { 200 } else { 404 };
            assert_eq!(
                read.status, expected,
                "case {package:?}/{other:?}: expected {expected}, got {read:?}"
            );
            if expected == 200 {
                assert_eq!(
                    read.text("description"),
                    Some(format!("version of {other}")),
                    "case {other:?}: each record keeps its own description, got {read:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_package_holds_a_bounded_number_of_versions() {
    let emulator = Emulator::new();

    // A package's version set is one a caller grows, so it has a ceiling: creates up to
    // `MAX_PACKAGE_VERSIONS` succeed, and every create past it is refused with the
    // modelled `ServiceQuotaExceededException` (HTTP 402) while the versions already
    // recorded stay readable — and another package's set is bounded separately.
    for package in ["bounded-package", "unbounded-neighbour"] {
        assert_eq!(
            emulator.create_package(package).await.status,
            200,
            "case {package:?}: the package must be creatable"
        );
    }

    for index in 0..MAX_PACKAGE_VERSIONS {
        let created = emulator
            .create_version("bounded-package", &format!("1.0.{index}"), &json!({}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {index}: a create below the ceiling must succeed, got {created:?}"
        );
    }

    for version in ["one-too-many", "another-too-many"] {
        let refused = emulator
            .create_version("bounded-package", version, &json!({}))
            .await;
        assert_eq!(
            refused.status, 402,
            "case {version:?}: a create past the ceiling must be refused 402, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {version:?}: expected ServiceQuotaExceededException, got {refused:?}"
        );
        let read = emulator.get_version("bounded-package", version).await;
        assert_eq!(
            read.status, 404,
            "case {version:?}: the refused create must record no version, got {read:?}"
        );
    }

    for index in [0, MAX_PACKAGE_VERSIONS / 2, MAX_PACKAGE_VERSIONS - 1] {
        let read = emulator
            .get_version("bounded-package", &format!("1.0.{index}"))
            .await;
        assert_eq!(
            read.status, 200,
            "case {index}: a recorded version must still be readable, got {read:?}"
        );
    }

    let neighbour = emulator
        .create_version("unbounded-neighbour", "1.0.0", &json!({}))
        .await;
    assert_eq!(
        neighbour.status, 200,
        "another package's version set is bounded separately, got {neighbour:?}"
    );
}
