// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT Software Package Catalog:
//! `CreatePackage` and the `GetPackage` reader that proves what it persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: a create in this control plane RECORDS the resource under the
//! name that identifies it and answers with the record it persisted, the
//! single-resource reader reports that same record member for member and stably across
//! reads, a create under a name the store already holds is refused with the
//! operation's modelled conflict error and changes nothing (unless it repeats the
//! `@idempotencyToken` that minted the record, when it is that one request arriving
//! twice), and a name the store does not hold is refused with the reader's modelled
//! not-found error instead of a success. Here that means `CreatePackage` records a
//! package under its `packageName` and answers `packageName` / `packageArn`
//! (`arn:aws:iot:<region>:<account>:package/<packageName>`) / `description`, and
//! `GetPackage` reports those three plus the `creationDate` / `lastModifiedDate` the
//! create stamped and no `defaultVersionName` (a create promotes no version —
//! `UpdatePackage$defaultVersionName` is what sets one).
//!
//! The table spans what that rule generalises over — the form and length of a package
//! name, whether the create carries a description and how long it is, how many tags it
//! carries, whether it carries a `clientToken`, and the region the call is signed for
//! — rather than one recorded request.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::state::MAX_PACKAGES;
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

    /// `CreatePackage`: `PUT /packages/{packageName}`, with `clientToken` bound to the
    /// query string (`smithy_query(query="operation",operation="CreatePackage")`).
    async fn create_in(
        &self,
        name: &str,
        body: &Value,
        client_token: Option<&str>,
        region: &str,
    ) -> Reply {
        let path = match client_token {
            Some(token) => format!("/packages/{name}?clientToken={token}"),
            None => format!("/packages/{name}"),
        };
        self.call("PUT", &path, Some(body), region).await
    }

    async fn create(&self, name: &str, body: &Value) -> Reply {
        self.create_in(name, body, None, REGION).await
    }

    /// `GetPackage`: `GET /packages/{packageName}`.
    async fn get_in(&self, name: &str, region: &str) -> Reply {
        self.call("GET", &format!("/packages/{name}"), None, region)
            .await
    }

    async fn get(&self, name: &str) -> Reply {
        self.get_in(name, REGION).await
    }
}

/// The ARN a package created in `region` must carry.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:package/{name}")
}

/// Epoch seconds now, for bounding a reported timestamp.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A `TagMap` body member carrying `count` distinct keys. `TagMap` is a map of
/// `TagKey` to `TagValue` with modelled `length 1..50`
/// (`smithy_query(query="shape",shape="TagMap")`), so a request either carries at
/// least one tag or carries the member not at all.
fn tags(count: usize) -> Value {
    let mut map = Map::new();
    for index in 0..count {
        map.insert(format!("key-{index}"), Value::from(format!("value-{index}")));
    }
    Value::Object(map)
}

/// One create request: a label, the package name, the region it is signed for, the
/// request body, the `clientToken` it carries, and what the response and every later
/// read must report for `description`.
struct Case {
    label: &'static str,
    name: String,
    region: &'static str,
    body: Value,
    client_token: Option<String>,
    /// What `description` must report: the body's value, or `None` when the create
    /// carried none.
    description: Option<String>,
}

/// The table every case list below is built from. It spans what the rule generalises
/// over:
///
/// * the form and length of `packageName` — modelled
///   `length 1..128, pattern ^[a-zA-Z0-9-_.]+$`
///   (`smithy_query(query="operation",operation="CreatePackage")`), so the rows
///   include the shortest legal name, the longest, and one using every character class
///   the pattern allows;
/// * whether the create carries a `description` and how long it is — modelled
///   `ResourceDescription`, `length 0..1024, pattern ^[^\p{C}]+$`, so the rows cover
///   the member absent, a one-character description and the longest legal one;
/// * how many `tags` it carries — none at all, one, and the modelled maximum of 50;
/// * whether it carries a `clientToken` — the `@idempotencyToken`, `length 36..64,
///   pattern ^\S{36,64}$`, so the rows cover absent, the shortest legal token and the
///   longest;
/// * the region the call is signed for, which is the region the package's ARN is
///   scoped to.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "only the required package name",
            name: "minimal-package".to_string(),
            region: REGION,
            body: json!({}),
            client_token: None,
            description: None,
        },
        Case {
            label: "a one-character description and one tag",
            name: "described_package".to_string(),
            region: REGION,
            body: json!({ "description": "x", "tags": tags(1) }),
            client_token: None,
            description: Some("x".to_string()),
        },
        Case {
            label: "the longest legal description and the modelled maximum of 50 tags",
            name: "big.package".to_string(),
            region: REGION,
            body: json!({ "description": "d".repeat(1024), "tags": tags(50) }),
            client_token: None,
            description: Some("d".repeat(1024)),
        },
        Case {
            label: "a name using every character class the pattern allows",
            name: "Pkg_01-aZ.9".to_string(),
            region: REGION,
            body: json!({ "description": "every character class" }),
            client_token: None,
            description: Some("every character class".to_string()),
        },
        Case {
            label: "the shortest legal client token",
            name: "token-shortest".to_string(),
            region: REGION,
            body: json!({ "description": "created with the shortest token" }),
            client_token: Some("t".repeat(36)),
            description: Some("created with the shortest token".to_string()),
        },
        Case {
            label: "the longest legal client token, signed for another region",
            name: "token-longest".to_string(),
            region: "eu-west-1",
            body: json!({ "tags": tags(3) }),
            client_token: Some("T".repeat(64)),
            description: None,
        },
        Case {
            label: "the shortest legal package name",
            name: "a".to_string(),
            region: "ap-southeast-2",
            body: json!({ "description": "shortest name" }),
            client_token: None,
            description: Some("shortest name".to_string()),
        },
        Case {
            label: "the longest legal package name",
            name: "p".repeat(128),
            region: REGION,
            body: json!({ "description": "longest name", "tags": tags(2) }),
            client_token: Some(format!("{}-{}", "L".repeat(30), "l".repeat(9))),
            description: Some("longest name".to_string()),
        },
    ]
}

#[tokio::test]
async fn create_package_records_the_package_get_package_reports() {
    let emulator = Emulator::new();

    for case in cases() {
        let Case {
            label,
            name,
            region,
            body,
            client_token,
            description,
        } = case;

        let before = now_epoch_seconds();
        let created = emulator
            .create_in(&name, &body, client_token.as_deref(), region)
            .await;
        let after = now_epoch_seconds();

        assert_eq!(
            created.status, 200,
            "case {label:?}: a create must answer 200, got {created:?}"
        );
        assert_eq!(
            created.text("packageName"),
            Some(name.clone()),
            "case {label:?}: the create must echo the package name, got {created:?}"
        );
        assert_eq!(
            created.text("packageArn"),
            Some(expected_arn(region, &name)),
            "case {label:?}: expected {:?}, got {created:?}",
            expected_arn(region, &name)
        );
        assert_eq!(
            created.text("description"),
            description,
            "case {label:?}: the create must report the description it stored, got {created:?}"
        );

        let read = emulator.get_in(&name, region).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the created package must be readable, got {read:?}"
        );
        assert_eq!(
            read.text("packageName"),
            Some(name.clone()),
            "case {label:?}: the reader must report the stored name, got {read:?}"
        );
        assert_eq!(
            read.text("packageArn"),
            created.text("packageArn"),
            "case {label:?}: the reader must report the ARN the create minted, got {read:?}"
        );
        assert_eq!(
            read.text("description"),
            description,
            "case {label:?}: the reader must report the stored description, got {read:?}"
        );
        // A create promotes no version, so the package has no default version yet.
        assert!(
            read.body.get("defaultVersionName").is_none(),
            "case {label:?}: a created package has no default version, got {read:?}"
        );

        let creation_date = read
            .epoch("creationDate")
            .unwrap_or_else(|| panic!("case {label:?}: the reader must report a creationDate, got {read:?}"));
        let last_modified = read
            .epoch("lastModifiedDate")
            .unwrap_or_else(|| panic!("case {label:?}: the reader must report a lastModifiedDate, got {read:?}"));
        assert!(
            creation_date >= before.floor() && creation_date <= after.ceil(),
            "case {label:?}: creationDate {creation_date} must lie in [{before}, {after}]"
        );
        assert!(
            (last_modified - creation_date).abs() < f64::EPSILON,
            "case {label:?}: a package nobody updated was last modified when it was \
             created, got {last_modified} against {creation_date}"
        );

        // Reading does not change what a later caller sees.
        let again = emulator.get_in(&name, region).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: a second read must report the same record, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_package_name_already_held_is_a_conflict() {
    let emulator = Emulator::new();

    for case in cases() {
        let Case {
            label,
            name,
            region,
            body,
            client_token,
            description,
        } = case;

        let created = emulator
            .create_in(&name, &body, client_token.as_deref(), region)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the first create must succeed, got {created:?}"
        );

        // A second create under the same name, with a DIFFERENT idempotency token (and
        // a different description), is refused 409 and overwrites nothing.
        let other_token = "Z".repeat(40);
        let refused = emulator
            .create_in(
                &name,
                &json!({ "description": "a different package" }),
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

        let read = emulator.get_in(&name, region).await;
        assert_eq!(
            read.text("description"),
            description,
            "case {label:?}: the refused create must change nothing, got {read:?}"
        );
        assert_eq!(
            read.text("packageArn"),
            Some(expected_arn(region, &name)),
            "case {label:?}: the refused create must leave the ARN alone, got {read:?}"
        );

        // Repeating the create that minted the record — same name, same client token —
        // is that one request arriving twice: it answers from the record.
        if let Some(token) = client_token.as_deref() {
            let repeated = emulator.create_in(&name, &body, Some(token), region).await;
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
async fn a_package_the_catalog_does_not_hold_is_not_found() {
    let emulator = Emulator::new();

    // Names of every legal form, none of them created: the reader refuses each with
    // the modelled 404 rather than a hollow 200.
    let absent = [
        "a".to_string(),
        "never-created".to_string(),
        "Never_Created.1".to_string(),
        "n".repeat(128),
    ];
    for name in &absent {
        let read = emulator.get(name).await;
        assert_eq!(
            read.status, 404,
            "case {name:?}: an uncreated package must be refused 404, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: expected ResourceNotFoundException, got {read:?}"
        );
    }

    // Creating one of them makes exactly that one readable, and leaves the rest absent.
    let created = emulator.create(&absent[1], &json!({})).await;
    assert_eq!(
        created.status, 200,
        "a create of an absent name must succeed, got {created:?}"
    );
    for name in &absent {
        let read = emulator.get(name).await;
        let expected = if name == &absent[1] { 200 } else { 404 };
        assert_eq!(
            read.status, expected,
            "case {name:?}: expected {expected}, got {read:?}"
        );
    }
}

#[tokio::test]
async fn the_package_catalog_is_bounded() {
    let emulator = Emulator::new();

    // The catalog is one a caller grows, so it has a ceiling: creates up to
    // `MAX_PACKAGES` succeed, and every create past it is refused with the modelled
    // `ServiceQuotaExceededException` (HTTP 402) while the packages already recorded
    // stay readable.
    for index in 0..MAX_PACKAGES {
        let created = emulator
            .create(&format!("bounded-package-{index}"), &json!({}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {index}: a create below the ceiling must succeed, got {created:?}"
        );
    }

    for name in ["one-too-many", "another-too-many"] {
        let refused = emulator.create(name, &json!({})).await;
        assert_eq!(
            refused.status, 402,
            "case {name:?}: a create past the ceiling must be refused 402, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {name:?}: expected ServiceQuotaExceededException, got {refused:?}"
        );
        // The refused create recorded nothing under that name.
        let read = emulator.get(name).await;
        assert_eq!(
            read.status, 404,
            "case {name:?}: the refused create must record no package, got {read:?}"
        );
    }

    for index in [0, MAX_PACKAGES / 2, MAX_PACKAGES - 1] {
        let read = emulator.get(&format!("bounded-package-{index}")).await;
        assert_eq!(
            read.status, 200,
            "case {index}: a recorded package must still be readable, got {read:?}"
        );
    }
}
