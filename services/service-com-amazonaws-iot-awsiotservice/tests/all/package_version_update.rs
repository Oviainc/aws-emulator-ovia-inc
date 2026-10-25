// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdatePackageVersion`, the in-place update of one
//! version in the AWS IoT Software Package Catalog, read back through the
//! `GetPackageVersion` reader that proves what it persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what an
//! AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: an update of an existing resource is a PATCH of the record the
//! create persisted — each member the request supplies replaces the stored one, a
//! member it leaves out keeps the value already stored, the resource's identity (its
//! names and its ARN) and its creation stamp never move, its last-modified stamp moves
//! to the instant of the update, and the status member named by an action member is
//! assigned the status that action names; an update whose resource the store does not
//! hold is refused with the operation's modelled not-found error, an update of a member
//! the model documents as settable only in a given state is refused with the modelled
//! conflict error once the resource has left that state, and every refusal leaves the
//! record byte-for-byte as it was. Here that means `UpdatePackageVersion` patches
//! `description` / `attributes` / `artifact` / `recipe`, assigns `PUBLISHED` for
//! `PUBLISH` and `DEPRECATED` for `DEPRECATE`, answers 200 with an empty body, refuses a
//! `(packageName, versionName)` pair the catalog does not hold with
//! `ResourceNotFoundException` (HTTP 404), and refuses `attributes` on a version that is
//! no longer in the draft state with `ConflictException` (HTTP 409) — the one state
//! restriction the model documents (`UpdatePackageVersionRequest$attributes`: "Attributes
//! can be updated only when the package version is in a draft state").
//!
//! The table spans what that rule generalises over — which members an update carries
//! (none, one at a time, all at once), whether a supplied map replaces or empties the
//! stored one, the length of a supplied value at its published bound, which action the
//! update carries (every value of `PackageVersionAction`, driven off the model's own
//! value list), the status the version already holds, whether the update carries a
//! `clientToken`, and the region the calls are signed for.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::ops::types::{
    PACKAGE_VERSION_ACTION_VALUES, PACKAGE_VERSION_STATUS_VALUES,
};
use service_com_amazonaws_iot_awsiotservice::state::{
    package_version_status_for_action, NEW_PACKAGE_VERSION_STATUS,
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

    /// Whether the response carried no members at all — what an operation whose output
    /// shape binds none answers with.
    fn is_empty_payload(&self) -> bool {
        match &self.body {
            Value::Null => true,
            Value::Object(members) => members.is_empty(),
            _ => false,
        }
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

    /// `CreatePackageVersion`: `PUT /packages/{packageName}/versions/{versionName}`.
    async fn create_version_in(
        &self,
        package: &str,
        version: &str,
        body: &Value,
        region: &str,
    ) -> Reply {
        self.call(
            "PUT",
            &format!("/packages/{package}/versions/{version}"),
            Some(body),
            region,
        )
        .await
    }

    async fn create_version(&self, package: &str, version: &str, body: &Value) -> Reply {
        self.create_version_in(package, version, body, REGION).await
    }

    /// `UpdatePackageVersion`: `PATCH /packages/{packageName}/versions/{versionName}`,
    /// with `clientToken` bound to the query string
    /// (`smithy_query(query="operation",operation="UpdatePackageVersion")`).
    async fn update_version_in(
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
        self.call("PATCH", &path, Some(body), region).await
    }

    async fn update_version(&self, package: &str, version: &str, body: &Value) -> Reply {
        self.update_version_in(package, version, body, None, REGION)
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

/// A `ResourceAttributes` body member carrying `count` distinct keys, each key prefixed
/// with `prefix` so two calls can produce two disjoint maps. Its key is modelled
/// `length 1.., pattern ^[a-zA-Z0-9:_-]+$` and its value `length 1.., pattern ^[^\p{C}]+$`
/// (`smithy_query(query="shape",shape="ResourceAttributes")`), so the keys below use
/// every character class the key pattern allows.
fn attributes(prefix: &str, count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(
            format!("{prefix}:key_{index}-A"),
            Value::from(format!("value {index}")),
        );
    }
    Value::Object(map)
}

/// One update request: a label, the body the version is created with, the body the
/// update carries, the `clientToken` the update carries, the region both calls are
/// signed for, and what every later read must report.
struct Case {
    label: &'static str,
    create: Value,
    update: Value,
    client_token: Option<String>,
    region: &'static str,
    description: Option<String>,
    expected_attributes: Option<Value>,
    expected_artifact: Option<Value>,
    recipe: Option<String>,
    status: String,
}

/// The table every case list below is built from. It spans what the rule generalises
/// over:
///
/// * which members the update carries — none at all, each of `description`,
///   `attributes`, `artifact` and `recipe` alone, and all of them at once;
/// * whether a supplied map replaces the stored one entirely, and whether a map with no
///   entries leaves the version with no attributes;
/// * the length of a supplied value at its published bound — `ResourceDescription` is
///   `length 0..1024` and `PackageVersionRecipe` `length 0..3072`
///   (`smithy_query(query="operation",operation="UpdatePackageVersion")`), so the rows
///   cover one character and the longest legal value of each;
/// * which action the update carries — absent, and each value of `PackageVersionAction`,
///   alone and alongside other members;
/// * whether the update carries a `clientToken` — the `@idempotencyToken`,
///   `length 36..64, pattern ^\S{36,64}$`, so the rows cover absent, the shortest legal
///   token and the longest;
/// * the region the calls are signed for, which is the region the version's ARN is
///   scoped to and which an update must leave alone.
fn cases() -> Vec<Case> {
    let artifact = json!({ "s3Location": { "bucket": "ota-bucket", "key": "firmware/v1.bin" } });
    let other_artifact = json!({
        "s3Location": { "bucket": "ota-bucket", "key": "firmware/v2.bin", "version": "3HL4kqtJl" }
    });
    let published = package_version_status_for_action("PUBLISH")
        .expect("PUBLISH names a status")
        .to_string();
    let deprecated = package_version_status_for_action("DEPRECATE")
        .expect("DEPRECATE names a status")
        .to_string();
    vec![
        Case {
            label: "no member at all: every stored value survives",
            create: json!({
                "description": "before",
                "attributes": attributes("stored", 2),
                "artifact": artifact.clone(),
                "recipe": "stored recipe",
            }),
            update: json!({}),
            client_token: None,
            region: REGION,
            description: Some("before".to_string()),
            expected_attributes: Some(attributes("stored", 2)),
            expected_artifact: Some(artifact.clone()),
            recipe: Some("stored recipe".to_string()),
            status: NEW_PACKAGE_VERSION_STATUS.to_string(),
        },
        Case {
            label: "the description alone, leaving the other members stored",
            create: json!({
                "description": "before",
                "attributes": attributes("stored", 1),
                "recipe": "stored recipe",
            }),
            update: json!({ "description": "after" }),
            client_token: None,
            region: REGION,
            description: Some("after".to_string()),
            expected_attributes: Some(attributes("stored", 1)),
            expected_artifact: None,
            recipe: Some("stored recipe".to_string()),
            status: NEW_PACKAGE_VERSION_STATUS.to_string(),
        },
        Case {
            label: "the longest legal description, onto a version created without one",
            create: json!({}),
            update: json!({ "description": "d".repeat(1024) }),
            client_token: None,
            region: REGION,
            description: Some("d".repeat(1024)),
            expected_attributes: None,
            expected_artifact: None,
            recipe: None,
            status: NEW_PACKAGE_VERSION_STATUS.to_string(),
        },
        Case {
            label: "a one-character description with the shortest legal client token",
            create: json!({ "description": "before" }),
            update: json!({ "description": "x" }),
            client_token: Some("t".repeat(36)),
            region: REGION,
            description: Some("x".to_string()),
            expected_attributes: None,
            expected_artifact: None,
            recipe: None,
            status: NEW_PACKAGE_VERSION_STATUS.to_string(),
        },
        Case {
            label: "attributes replace the stored map rather than merging into it",
            create: json!({ "attributes": attributes("stored", 3) }),
            update: json!({ "attributes": attributes("patched", 1) }),
            client_token: None,
            region: REGION,
            description: None,
            expected_attributes: Some(attributes("patched", 1)),
            expected_artifact: None,
            recipe: None,
            status: NEW_PACKAGE_VERSION_STATUS.to_string(),
        },
        Case {
            label: "an attributes map with no entries leaves the version with none",
            create: json!({ "attributes": attributes("stored", 2) }),
            update: json!({ "attributes": {} }),
            client_token: None,
            region: REGION,
            description: None,
            expected_attributes: None,
            expected_artifact: None,
            recipe: None,
            status: NEW_PACKAGE_VERSION_STATUS.to_string(),
        },
        Case {
            label: "the artifact alone, replacing the stored one",
            create: json!({ "artifact": artifact.clone(), "description": "before" }),
            update: json!({ "artifact": other_artifact.clone() }),
            client_token: None,
            region: REGION,
            description: Some("before".to_string()),
            expected_attributes: None,
            expected_artifact: Some(other_artifact.clone()),
            recipe: None,
            status: NEW_PACKAGE_VERSION_STATUS.to_string(),
        },
        Case {
            label: "the longest legal recipe alone",
            create: json!({ "recipe": "stored recipe" }),
            update: json!({ "recipe": "r".repeat(3072) }),
            client_token: None,
            region: REGION,
            description: None,
            expected_attributes: None,
            expected_artifact: None,
            recipe: Some("r".repeat(3072)),
            status: NEW_PACKAGE_VERSION_STATUS.to_string(),
        },
        Case {
            label: "every member at once with PUBLISH, signed for another region",
            create: json!({
                "description": "before",
                "attributes": attributes("stored", 2),
                "artifact": artifact.clone(),
                "recipe": "stored recipe",
            }),
            update: json!({
                "description": "after",
                "attributes": attributes("patched", 4),
                "artifact": other_artifact.clone(),
                "recipe": "{\"steps\":[]}",
                "action": "PUBLISH",
            }),
            client_token: Some("T".repeat(64)),
            region: "eu-west-1",
            description: Some("after".to_string()),
            expected_attributes: Some(attributes("patched", 4)),
            expected_artifact: Some(other_artifact),
            recipe: Some("{\"steps\":[]}".to_string()),
            status: published.clone(),
        },
        Case {
            label: "PUBLISH alone leaves every other member stored",
            create: json!({
                "description": "before",
                "attributes": attributes("stored", 1),
                "artifact": artifact.clone(),
                "recipe": "stored recipe",
            }),
            update: json!({ "action": "PUBLISH" }),
            client_token: None,
            region: "ap-southeast-2",
            description: Some("before".to_string()),
            expected_attributes: Some(attributes("stored", 1)),
            expected_artifact: Some(artifact.clone()),
            recipe: Some("stored recipe".to_string()),
            status: published,
        },
        Case {
            label: "DEPRECATE alone leaves every other member stored",
            create: json!({ "description": "before", "artifact": artifact.clone() }),
            update: json!({ "action": "DEPRECATE" }),
            client_token: Some(format!("{}-{}", "L".repeat(30), "l".repeat(9))),
            region: REGION,
            description: Some("before".to_string()),
            expected_attributes: None,
            expected_artifact: Some(artifact),
            recipe: None,
            status: deprecated.clone(),
        },
        Case {
            label: "attributes alongside DEPRECATE, applied while still in draft",
            create: json!({}),
            update: json!({ "attributes": attributes("patched", 2), "action": "DEPRECATE" }),
            client_token: None,
            region: REGION,
            description: None,
            expected_attributes: Some(attributes("patched", 2)),
            expected_artifact: None,
            recipe: None,
            status: deprecated,
        },
    ]
}

#[tokio::test]
async fn update_package_version_patches_only_the_members_it_supplies() {
    let emulator = Emulator::new();

    for (index, case) in cases().into_iter().enumerate() {
        let Case {
            label,
            create,
            update,
            client_token,
            region,
            description,
            expected_attributes,
            expected_artifact,
            recipe,
            status,
        } = case;
        let package = format!("patched-package-{index}");
        let version = format!("1.0.{index}");

        assert_eq!(
            emulator.create_package_in(&package, region).await.status,
            200,
            "case {label:?}: the package a version hangs off must be creatable"
        );
        let created = emulator
            .create_version_in(&package, &version, &create, region)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the version must be creatable, got {created:?}"
        );
        let before_update = emulator.get_version_in(&package, &version, region).await;
        let creation_date = before_update.epoch("creationDate").unwrap_or_else(|| {
            panic!("case {label:?}: the created version must report a creationDate, got {before_update:?}")
        });

        let before = now_epoch_seconds();
        let updated = emulator
            .update_version_in(&package, &version, &update, client_token.as_deref(), region)
            .await;
        let after = now_epoch_seconds();

        assert_eq!(
            updated.status, 200,
            "case {label:?}: an update must answer 200, got {updated:?}"
        );
        // `UpdatePackageVersionResponse` binds no members.
        assert!(
            updated.is_empty_payload(),
            "case {label:?}: an update must answer with no members, got {updated:?}"
        );

        let read = emulator.get_version_in(&package, &version, region).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the updated version must be readable, got {read:?}"
        );
        assert_eq!(
            read.text("description"),
            description,
            "case {label:?}: expected description {description:?}, got {read:?}"
        );
        assert_eq!(
            read.body.get("attributes").cloned(),
            expected_attributes,
            "case {label:?}: expected attributes {expected_attributes:?}, got {read:?}"
        );
        assert_eq!(
            read.body.get("artifact").cloned(),
            expected_artifact,
            "case {label:?}: expected artifact {expected_artifact:?}, got {read:?}"
        );
        assert_eq!(
            read.text("recipe"),
            recipe,
            "case {label:?}: expected recipe {recipe:?}, got {read:?}"
        );
        assert_eq!(
            read.text("status"),
            Some(status.clone()),
            "case {label:?}: expected status {status:?}, got {read:?}"
        );

        // The version's identity and its creation stamp are not what an update
        // changes.
        assert_eq!(
            read.text("packageName"),
            Some(package.clone()),
            "case {label:?}: the package name must survive the update, got {read:?}"
        );
        assert_eq!(
            read.text("versionName"),
            Some(version.clone()),
            "case {label:?}: the version name must survive the update, got {read:?}"
        );
        assert_eq!(
            read.text("packageVersionArn"),
            Some(expected_arn(region, &package, &version)),
            "case {label:?}: expected {:?}, got {read:?}",
            expected_arn(region, &package, &version)
        );
        let read_creation = read.epoch("creationDate").unwrap_or_else(|| {
            panic!("case {label:?}: the reader must report a creationDate, got {read:?}")
        });
        assert!(
            (read_creation - creation_date).abs() < f64::EPSILON,
            "case {label:?}: the creation date must not move, expected {creation_date}, \
             got {read_creation}"
        );

        // "The date when the package version was last updated": this update is it.
        let last_modified = read.epoch("lastModifiedDate").unwrap_or_else(|| {
            panic!("case {label:?}: the reader must report a lastModifiedDate, got {read:?}")
        });
        assert!(
            last_modified >= before.floor() && last_modified <= after.ceil(),
            "case {label:?}: lastModifiedDate {last_modified} must lie in [{before}, {after}]"
        );
        assert!(
            last_modified >= read_creation,
            "case {label:?}: lastModifiedDate {last_modified} must not precede the \
             creationDate {read_creation}"
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
async fn each_action_assigns_the_status_it_names() {
    let emulator = Emulator::new();

    assert_eq!(
        emulator.create_package("action-package").await.status,
        200,
        "the package must be creatable"
    );

    // Every value the model binds for `action`, not only the one a recording used.
    for action in PACKAGE_VERSION_ACTION_VALUES {
        let version = format!("for-{}", action.to_lowercase());
        let expected = package_version_status_for_action(action).unwrap_or_else(|| {
            panic!("case {action:?}: every modelled action names a status it assigns")
        });
        assert!(
            PACKAGE_VERSION_STATUS_VALUES.contains(&expected),
            "case {action:?}: the assigned status {expected:?} must be a modelled status"
        );

        let created = emulator
            .create_version("action-package", &version, &json!({}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {action:?}: the version must be creatable, got {created:?}"
        );
        assert_eq!(
            created.text("status").as_deref(),
            Some(NEW_PACKAGE_VERSION_STATUS),
            "case {action:?}: a created version starts in the draft state, got {created:?}"
        );

        let updated = emulator
            .update_version("action-package", &version, &json!({ "action": action }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {action:?}: the action must be accepted, got {updated:?}"
        );
        let read = emulator.get_version("action-package", &version).await;
        assert_eq!(
            read.text("status").as_deref(),
            Some(expected),
            "case {action:?}: expected status {expected:?}, got {read:?}"
        );

        // The same action repeated assigns the same status: the request is idempotent.
        let repeated = emulator
            .update_version("action-package", &version, &json!({ "action": action }))
            .await;
        assert_eq!(
            repeated.status, 200,
            "case {action:?}: the action repeated must be accepted, got {repeated:?}"
        );
        let read_again = emulator.get_version("action-package", &version).await;
        assert_eq!(
            read_again.text("status").as_deref(),
            Some(expected),
            "case {action:?}: expected status {expected:?} again, got {read_again:?}"
        );
    }

    // One version walked through every action in turn ends in the status the LAST
    // action named, so an action is an assignment and not a one-way door.
    let walked = "walked-version";
    assert_eq!(
        emulator
            .create_version("action-package", walked, &json!({}))
            .await
            .status,
        200,
        "the walked version must be creatable"
    );
    for action in PACKAGE_VERSION_ACTION_VALUES
        .iter()
        .chain(PACKAGE_VERSION_ACTION_VALUES.iter().rev())
    {
        let updated = emulator
            .update_version("action-package", walked, &json!({ "action": action }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {action:?}: the action must be accepted, got {updated:?}"
        );
        let read = emulator.get_version("action-package", walked).await;
        assert_eq!(
            read.text("status").as_deref(),
            package_version_status_for_action(action),
            "case {action:?}: the status must follow the last action, got {read:?}"
        );
    }
}

#[tokio::test]
async fn attributes_are_refused_once_the_version_has_left_the_draft_state() {
    let emulator = Emulator::new();

    assert_eq!(
        emulator.create_package("conflict-package").await.status,
        200,
        "the package must be creatable"
    );

    // Every status an action can assign is a status that is no longer the draft one, so
    // each of them refuses an `attributes` update — whatever that map carries.
    for action in PACKAGE_VERSION_ACTION_VALUES {
        let version = format!("left-draft-{}", action.to_lowercase());
        assert_eq!(
            emulator
                .create_version(
                    "conflict-package",
                    &version,
                    &json!({ "attributes": attributes("stored", 2) })
                )
                .await
                .status,
            200,
            "case {action:?}: the version must be creatable"
        );
        assert_eq!(
            emulator
                .update_version("conflict-package", &version, &json!({ "action": action }))
                .await
                .status,
            200,
            "case {action:?}: the action must be accepted"
        );
        let before = emulator.get_version("conflict-package", &version).await;

        for attempt in [
            json!({ "attributes": attributes("patched", 1) }),
            json!({ "attributes": {} }),
            json!({ "attributes": attributes("patched", 3), "description": "and a description" }),
        ] {
            let refused = emulator
                .update_version("conflict-package", &version, &attempt)
                .await;
            assert_eq!(
                refused.status, 409,
                "case {action:?}/{attempt:?}: attributes outside the draft state must be \
                 refused 409, got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("ConflictException"),
                "case {action:?}/{attempt:?}: expected ConflictException, got {refused:?}"
            );
            let read = emulator.get_version("conflict-package", &version).await;
            assert_eq!(
                read.body, before.body,
                "case {action:?}/{attempt:?}: a refused update must change nothing, got {read:?}"
            );
        }

        // Every other member is still settable in that state.
        let allowed = emulator
            .update_version(
                "conflict-package",
                &version,
                &json!({ "description": "still settable", "recipe": "r" }),
            )
            .await;
        assert_eq!(
            allowed.status, 200,
            "case {action:?}: an update carrying no attributes must succeed, got {allowed:?}"
        );
        let read = emulator.get_version("conflict-package", &version).await;
        assert_eq!(
            read.text("description").as_deref(),
            Some("still settable"),
            "case {action:?}: the description must be patched, got {read:?}"
        );
        assert_eq!(
            read.body.get("attributes").cloned(),
            Some(attributes("stored", 2)),
            "case {action:?}: the stored attributes must survive, got {read:?}"
        );
    }

    // A version still in the draft state takes the very same attributes update.
    assert_eq!(
        emulator
            .create_version("conflict-package", "still-draft", &json!({}))
            .await
            .status,
        200,
        "the draft version must be creatable"
    );
    let accepted = emulator
        .update_version(
            "conflict-package",
            "still-draft",
            &json!({ "attributes": attributes("patched", 1) }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "attributes in the draft state must be accepted, got {accepted:?}"
    );
    let read = emulator.get_version("conflict-package", "still-draft").await;
    assert_eq!(
        read.body.get("attributes").cloned(),
        Some(attributes("patched", 1)),
        "the draft version must report the patched attributes, got {read:?}"
    );
}

#[tokio::test]
async fn a_version_the_catalog_does_not_hold_cannot_be_updated() {
    let emulator = Emulator::new();

    // Names of every legal form, with neither the package nor the version created:
    // each update is refused with the modelled 404 and records nothing.
    let names = [
        ("a".to_string(), "a".to_string()),
        ("never-created".to_string(), "1.0.0".to_string()),
        ("Never_Created.1".to_string(), "Ver_01-aZ.9".to_string()),
        ("n".repeat(128), "v".repeat(64)),
    ];
    for (package, version) in &names {
        let refused = emulator
            .update_version(package, version, &json!({ "description": "nothing to patch" }))
            .await;
        assert_eq!(
            refused.status, 404,
            "case {package:?}/{version:?}: an update of an uncreated version must be \
             refused 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {package:?}/{version:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        let read = emulator.get_version(package, version).await;
        assert_eq!(
            read.status, 404,
            "case {package:?}/{version:?}: the refused update must record nothing, got {read:?}"
        );
    }

    // A package with no versions holds no version either, so the update is still a 404.
    for (package, version) in &names {
        assert_eq!(
            emulator.create_package(package).await.status,
            200,
            "case {package:?}: the package must be creatable"
        );
        let refused = emulator
            .update_version(package, version, &json!({ "action": "PUBLISH" }))
            .await;
        assert_eq!(
            refused.status, 404,
            "case {package:?}/{version:?}: an update of a version the package does not \
             hold must be refused 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {package:?}/{version:?}: expected ResourceNotFoundException, got {refused:?}"
        );
    }

    // Creating the version makes exactly that pair updatable.
    let (package, version) = &names[1];
    assert_eq!(
        emulator
            .create_version(package, version, &json!({}))
            .await
            .status,
        200,
        "the version must be creatable"
    );
    for (other_package, other_version) in &names {
        let updated = emulator
            .update_version(
                other_package,
                other_version,
                &json!({ "description": "patched" }),
            )
            .await;
        let expected = if (other_package, other_version) == (package, version) {
            200
        } else {
            404
        };
        assert_eq!(
            updated.status, expected,
            "case {other_package:?}/{other_version:?}: expected {expected}, got {updated:?}"
        );
    }
}

#[tokio::test]
async fn an_update_touches_only_the_version_it_names() {
    let emulator = Emulator::new();

    // The same version name in two packages is two records, and a package holds several
    // versions: an update names ONE pair, so every other record is untouched.
    let packages = ["left-package", "right-package"];
    let versions = ["1.0.0", "2.0.0"];
    for package in packages {
        assert_eq!(
            emulator.create_package(package).await.status,
            200,
            "case {package:?}: the package must be creatable"
        );
        for version in versions {
            let created = emulator
                .create_version(
                    package,
                    version,
                    &json!({ "description": format!("{package}/{version}") }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {package:?}/{version:?}: the version must be creatable, got {created:?}"
            );
        }
    }

    let mut before = Vec::new();
    for package in packages {
        for version in versions {
            before.push((
                package,
                version,
                emulator.get_version(package, version).await.body,
            ));
        }
    }

    let updated = emulator
        .update_version(
            packages[0],
            versions[0],
            &json!({ "description": "patched", "action": "PUBLISH" }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "the named version must be updatable, got {updated:?}"
    );

    for (package, version, recorded) in before {
        let read = emulator.get_version(package, version).await;
        if package == packages[0] && version == versions[0] {
            assert_eq!(
                read.text("description").as_deref(),
                Some("patched"),
                "case {package:?}/{version:?}: the named version must be patched, got {read:?}"
            );
            assert_eq!(
                read.text("status"),
                package_version_status_for_action("PUBLISH").map(str::to_string),
                "case {package:?}/{version:?}: the named version must take the status the \
                 action names, got {read:?}"
            );
        } else {
            assert_eq!(
                read.body, recorded,
                "case {package:?}/{version:?}: an update of another version must change \
                 nothing here, got {read:?}"
            );
        }
    }
}
