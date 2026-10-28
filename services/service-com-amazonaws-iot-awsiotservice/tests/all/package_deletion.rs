// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeletePackageVersion` and `DeletePackage`: removing a
//! software package is removing its VERSIONS first and then the package itself.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over restJson1
//! `PUT`/`GET`/`DELETE` on `/packages/{packageName}` and
//! `/packages/{packageName}/versions/{versionName}`, signed for the `iot` signing name
//! — so what a delete removed is read back through the modelled surface, not out of the
//! state struct.
//!
//! The rules under test:
//!
//! * a deleted version is gone from its package and from every reader of it, while the
//!   package and its OTHER versions are untouched;
//! * "All package versions must be deleted before deleting the software package": a
//!   package that still holds versions is refused and stays whole;
//! * a version its package designates as the default is refused until the designation
//!   is removed;
//! * both deletes are `@idempotent` and model no not-found error, so a name the catalog
//!   does not hold is a 200 that removed nothing.
//!
//! The tables span what the rules generalise over — how many versions a package holds
//! and which of them is deleted, the order the deletes arrive in, a repeat of a delete,
//! and a name nothing created.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    missing_package_message, missing_package_version_message, ComAmazonawsIotAwsiotserviceState,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name:
/// the scope is how the router picks the claiming service.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
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
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
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

    /// `CreatePackage`: `PUT /packages/{packageName}`.
    async fn create_package(&self, package: &str) -> Reply {
        self.call("PUT", &format!("/packages/{package}"), Some(&json!({})))
            .await
    }

    /// `CreatePackageVersion`: `PUT /packages/{packageName}/versions/{versionName}`.
    async fn create_version(&self, package: &str, version: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/packages/{package}/versions/{version}"),
            Some(&json!({})),
        )
        .await
    }

    /// `GetPackage`: `GET /packages/{packageName}`.
    async fn get_package(&self, package: &str) -> Reply {
        self.call("GET", &format!("/packages/{package}"), None).await
    }

    /// `GetPackageVersion`: `GET /packages/{packageName}/versions/{versionName}`.
    async fn get_version(&self, package: &str, version: &str) -> Reply {
        self.call(
            "GET",
            &format!("/packages/{package}/versions/{version}"),
            None,
        )
        .await
    }

    /// `DeletePackage`: `DELETE /packages/{packageName}`, with `clientToken` bound to
    /// the query string (`smithy_query(query="operation",operation="DeletePackage")`).
    async fn delete_package(&self, package: &str, client_token: Option<&str>) -> Reply {
        let path = match client_token {
            Some(token) => format!("/packages/{package}?clientToken={token}"),
            None => format!("/packages/{package}"),
        };
        self.call("DELETE", &path, None).await
    }

    /// `DeletePackageVersion`: `DELETE /packages/{packageName}/versions/{versionName}`.
    async fn delete_version(&self, package: &str, version: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/packages/{package}/versions/{version}"),
            None,
        )
        .await
    }

    /// Designate `version` as `package`'s default version.
    ///
    /// `UpdatePackage$defaultVersionName` is the only request that sets it and that
    /// operation is still a scaffold, so the designation is written into the catalog
    /// directly to set up the state a delete has to refuse.
    fn designate_default_version(&self, package: &str, version: &str) {
        let state = self.engine.state::<ComAmazonawsIotAwsiotserviceState>();
        let mut state = state.lock();
        let package = state
            .packages
            .get_mut(package)
            .unwrap_or_else(|| panic!("{package} must have been created"));
        package.default_version_name = Some(version.to_string());
    }
}

/// One row of the version-deletion table.
struct VersionCase {
    case: &'static str,
    package: &'static str,
    /// The versions the package holds before the delete.
    versions: &'static [&'static str],
    /// The version the delete names; it need not be one the package holds.
    delete: &'static str,
}

/// RULE: deleting a package version removes exactly that version — its package and
/// every other version it holds are untouched — and a version the catalog does not hold
/// is the same 200, because the delete is idempotent and models no not-found error.
#[tokio::test]
async fn deleting_a_package_version_removes_only_that_version() {
    let emulator = Emulator::new();

    let cases: Vec<VersionCase> = vec![
        VersionCase {
            case: "the only version of a package",
            package: "pkg-one-version",
            versions: &["1.0.0"],
            delete: "1.0.0",
        },
        VersionCase {
            case: "the first of three versions",
            package: "pkg-first-of-three",
            versions: &["1.0.0", "1.1.0", "2.0.0"],
            delete: "1.0.0",
        },
        VersionCase {
            case: "the last of three versions",
            package: "pkg-last-of-three",
            versions: &["1.0.0", "1.1.0", "2.0.0"],
            delete: "2.0.0",
        },
        VersionCase {
            case: "a version name the package does not hold",
            package: "pkg-unknown-version",
            versions: &["1.0.0"],
            delete: "9.9.9",
        },
        VersionCase {
            case: "a version name differing from a held one only in case",
            package: "pkg-case-sensitive",
            versions: &["v1.0.0"],
            delete: "V1.0.0",
        },
    ];

    for case in &cases {
        assert_eq!(
            emulator.create_package(case.package).await.status,
            200,
            "case {:?}: the package must be creatable",
            case.case
        );
        for version in case.versions {
            assert_eq!(
                emulator.create_version(case.package, version).await.status,
                200,
                "case {:?}: version {version} must be creatable",
                case.case
            );
        }

        let reply = emulator.delete_version(case.package, case.delete).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, reply.status, reply.body
        );

        // The version the delete named is gone, whichever way: the reader refuses it
        // with the catalog's own wording.
        let read = emulator.get_version(case.package, case.delete).await;
        assert_eq!(
            read.status, 404,
            "case {:?}: expected the deleted version to be gone, got {} {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {:?}: expected ResourceNotFoundException, got {:?}",
            case.case,
            read.error_code
        );
        assert_eq!(
            read.message(),
            missing_package_version_message(case.package, case.delete),
            "case {:?}: expected the refusal to name the version and its package, got {:?}",
            case.case,
            read.body
        );

        // Every version the delete did NOT name survives, and so does the package.
        for version in case.versions.iter().filter(|held| **held != case.delete) {
            assert_eq!(
                emulator.get_version(case.package, version).await.status,
                200,
                "case {:?}: version {version} must survive the delete of {}",
                case.case,
                case.delete
            );
        }
        assert_eq!(
            emulator.get_package(case.package).await.status,
            200,
            "case {:?}: the package must survive a version delete",
            case.case
        );

        // Idempotent: the same delete again is the same answer and removes nothing
        // more.
        let again = emulator.delete_version(case.package, case.delete).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: a repeated delete must answer 200, got {} {:?}",
            case.case, again.status, again.body
        );
    }

    // A package name nothing created holds no version either, so the delete is the same
    // 200 rather than a refusal this operation cannot express.
    let unknown = emulator.delete_version("pkg-never-created", "1.0.0").await;
    assert_eq!(
        unknown.status, 200,
        "case \"a package nothing created\": expected 200, got {} {:?}",
        unknown.status, unknown.body
    );
}

/// RULE: a package is deleted only once it holds no versions — one that still does is
/// refused with the modelled `ValidationException` and stays whole — and a package name
/// the catalog does not hold is the idempotent 200.
#[tokio::test]
async fn a_package_is_deleted_only_once_its_versions_are_gone() {
    let emulator = Emulator::new();

    // Rows differ in how many versions stand in the way of the delete.
    let cases: [(&str, &str, &[&str]); 3] = [
        ("a package with no version at all", "pkg-empty", &[]),
        ("a package holding one version", "pkg-holding-one", &["1.0.0"]),
        (
            "a package holding three versions",
            "pkg-holding-three",
            &["1.0.0", "1.1.0", "2.0.0"],
        ),
    ];

    for (case, package, versions) in cases {
        assert_eq!(
            emulator.create_package(package).await.status,
            200,
            "case {case:?}: the package must be creatable"
        );
        for version in versions {
            assert_eq!(
                emulator.create_version(package, version).await.status,
                200,
                "case {case:?}: version {version} must be creatable"
            );
        }

        if !versions.is_empty() {
            let refused = emulator.delete_package(package, None).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected 400 while versions remain, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {:?}",
                refused.error_code
            );
            assert!(
                refused
                    .message()
                    .contains(&format!("{} version(s)", versions.len())),
                "case {case:?}: expected the refusal to count the versions in the way, \
                 got {:?}",
                refused.message()
            );

            // Nothing was removed: the package and every version are still readable.
            assert_eq!(
                emulator.get_package(package).await.status,
                200,
                "case {case:?}: the refused delete must keep the package"
            );
            for version in versions {
                assert_eq!(
                    emulator.get_version(package, version).await.status,
                    200,
                    "case {case:?}: the refused delete must keep version {version}"
                );
            }

            for version in versions {
                assert_eq!(
                    emulator.delete_version(package, version).await.status,
                    200,
                    "case {case:?}: version {version} must be deletable"
                );
            }
        }

        // With no version left, the package goes.
        let deleted = emulator
            .delete_package(package, Some("0123456789abcdef0123456789abcdef0123"))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 once no version remains, got {} {:?}",
            deleted.status, deleted.body
        );
        let read = emulator.get_package(package).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the deleted package to be gone, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.message(),
            missing_package_message(package),
            "case {case:?}: expected the refusal to name the package, got {:?}",
            read.body
        );

        // Idempotent: a repeat of the delete, and a name nothing created, answer the
        // same 200.
        for name in [package, "pkg-never-created"] {
            let repeated = emulator.delete_package(name, None).await;
            assert_eq!(
                repeated.status, 200,
                "case {case:?}: a delete of {name} must answer 200, got {} {:?}",
                repeated.status, repeated.body
            );
        }

        // The name is free again once the package is gone.
        assert_eq!(
            emulator.create_package(package).await.status,
            200,
            "case {case:?}: the deleted name must be free to create again"
        );
        assert_eq!(
            emulator.delete_package(package, None).await.status,
            200,
            "case {case:?}: the re-created package must be deletable"
        );
    }
}

/// RULE: "If a package version is designated as default, you must remove the
/// designation from the software package" — so a delete of the default version is
/// refused and keeps it, while the package's other versions delete as usual.
#[tokio::test]
async fn the_default_version_of_a_package_is_not_deleted_while_it_is_designated() {
    let emulator = Emulator::new();
    let package = "pkg-with-default";

    assert_eq!(
        emulator.create_package(package).await.status,
        200,
        "the package must be creatable"
    );
    for version in ["1.0.0", "2.0.0"] {
        assert_eq!(
            emulator.create_version(package, version).await.status,
            200,
            "version {version} must be creatable"
        );
    }
    emulator.designate_default_version(package, "2.0.0");

    let refused = emulator.delete_version(package, "2.0.0").await;
    assert_eq!(
        refused.status, 400,
        "case \"the designated default version\": expected 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("ValidationException"),
        "case \"the designated default version\": expected ValidationException, got {:?}",
        refused.error_code
    );
    assert!(
        refused.message().contains("UpdatePackage"),
        "case \"the designated default version\": expected the refusal to name how the \
         designation is removed, got {:?}",
        refused.message()
    );
    assert_eq!(
        emulator.get_version(package, "2.0.0").await.status,
        200,
        "case \"the designated default version\": the refused delete must keep it"
    );

    // A version that is NOT the default is unaffected by the designation.
    assert_eq!(
        emulator.delete_version(package, "1.0.0").await.status,
        200,
        "case \"a version that is not the default\": expected 200"
    );
    assert_eq!(
        emulator.get_version(package, "1.0.0").await.status,
        404,
        "case \"a version that is not the default\": it must be gone"
    );

    // And the package still holds its default version, so it cannot be deleted either.
    let package_refused = emulator.delete_package(package, None).await;
    assert_eq!(
        package_refused.status, 400,
        "case \"a package still holding its default version\": expected 400, got {} {:?}",
        package_refused.status, package_refused.body
    );
}
