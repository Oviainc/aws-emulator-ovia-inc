// SPDX-License-Identifier: CC0-1.0

//! The Lightsail container image registry — what a container service reports
//! registered, and what registering an unpushed digest answers — driven over the
//! wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test:
//!
//! * `GetContainerImages` reports the registry of the ONE container service the
//!   request names in the region the request is made in — an empty list for a
//!   service nothing has been pushed to, and the service's `DoesNotExist`
//!   rejection for a name that names no container service in that region. It is a
//!   report: two reads in a row answer the same thing.
//! * `RegisterContainerImage` registers an image that is already in that
//!   service's private registry, so a digest nothing pushed there is refused with
//!   the same `DoesNotExist` envelope, and the registry it reports stays
//!   unchanged.
//! * `DeleteContainerImage` drops the named image from that registry if it is
//!   there and answers success either way — the delete is idempotent — while a
//!   request naming no container service in the request's region is refused with
//!   the modelled `InvalidInputException`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The longest label `RegisterContainerImage` accepts, from the modelled
/// `RegisterContainerImageRequest$label` constraint (`length 1..53`).
const MAX_LABEL_LEN: usize = 53;

/// A label of exactly [`MAX_LABEL_LEN`] characters, length-checked in
/// [`the_table_labels_match_the_modelled_bounds`].
const LABEL_AT_MAX_LEN: &str = "maximum-length-label-abcdefghijabcdefghijabcdefghijab";

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`, and the
        // `Authorization` header below carries only the credential SCOPE, which
        // is what names the region a request is made in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` in `region` with `body`, answering its HTTP status and
    /// parsed body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` in `region` and require `want_status`, answering the body.
    async fn ok(&self, region: &str, operation: &str, body: Value, want_status: u16) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, want_status,
            "case {operation} in {region}: expected HTTP {want_status}, got {status} {parsed:?}"
        );
        parsed
    }

    /// Create the container service `name` in `region`. `CreateContainerService`
    /// answers HTTP 201 (its modelled success status).
    async fn create_service(&self, region: &str, name: &str) {
        self.ok(
            region,
            "CreateContainerService",
            json!({"serviceName": name, "power": "nano", "scale": 1}),
            201,
        )
        .await;
    }

    /// The `containerImages` member `GetContainerImages` reports for `service` in
    /// `region`, as a raw value so a missing list is told from an empty one.
    async fn registry(&self, region: &str, service: &str) -> Value {
        self.ok(
            region,
            "GetContainerImages",
            json!({"serviceName": service}),
            200,
        )
        .await["containerImages"]
            .clone()
    }

    /// The images of that registry, as an array.
    async fn images(&self, region: &str, service: &str) -> Vec<Value> {
        let registry = self.registry(region, service).await;
        registry
            .as_array()
            .unwrap_or_else(|| {
                panic!("case reading {service} in {region}: expected an array, got {registry:?}")
            })
            .clone()
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Assert that `refused` is this service's `DoesNotExist` envelope: HTTP 400,
/// wire error `NotFoundException`, body `code` `DoesNotExist`, and a message.
fn assert_does_not_exist(case: &str, status: u16, refused: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(refused),
        "NotFoundException",
        "case {case}: expected the NotFoundException wire code, got {refused:?}"
    );
    assert_eq!(
        refused["code"].as_str(),
        Some("DoesNotExist"),
        "case {case}: expected the DoesNotExist body code, got {refused:?}"
    );
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case}: expected a message saying what does not exist, got {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One container service to create and then read the registry of. The rows
/// differ along everything the rule generalises over: the region the requests are
/// made in and the length and shape of the service name (the shortest the
/// modelled `ContainerServiceName` pattern admits, a hyphenated one, one carrying
/// digits).
#[derive(Debug, Clone, Copy)]
struct ServiceRow {
    case: &'static str,
    region: &'static str,
    name: &'static str,
}

/// The table of container services. Each is created, read, and then asked to
/// register a digest.
const SERVICE_ROWS: &[ServiceRow] = &[
    ServiceRow {
        case: "a hyphenated name in the default region",
        region: "us-east-1",
        name: "empty-registry",
    },
    ServiceRow {
        case: "the shortest name the modelled pattern admits",
        region: "us-west-2",
        name: "ab",
    },
    ServiceRow {
        case: "a name carrying digits, in a third region",
        region: "eu-west-1",
        name: "registry-2026",
    },
    ServiceRow {
        case: "a single-character name, in a fourth region",
        region: "ap-northeast-1",
        name: "r",
    },
];

/// One registration attempt. The rows differ along the digest's form and length
/// and along the label, which is what the rule generalises over: no digest at all
/// has been pushed, so every form is refused the same way.
#[derive(Debug, Clone, Copy)]
struct DigestRow {
    case: &'static str,
    label: &'static str,
    digest: &'static str,
}

/// The table of registration attempts.
const DIGEST_ROWS: &[DigestRow] = &[
    DigestRow {
        case: "a full sha256 digest",
        label: "roundtrip",
        digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    },
    DigestRow {
        case: "a truncated digest",
        label: "mystaticwebsite",
        digest: "sha256:0f1e2d",
    },
    DigestRow {
        case: "a digest naming another algorithm",
        label: "api",
        digest: "sha512:9c",
    },
    DigestRow {
        case: "a digest with no algorithm prefix at all",
        label: "x",
        digest: "deadbeef",
    },
    DigestRow {
        case: "a label at the modelled maximum length",
        label: LABEL_AT_MAX_LEN,
        digest: "sha256:ff",
    },
];

/// The image names a delete is driven with — none of which any registry here
/// holds, since nothing can be pushed to one. The rows differ along the form the
/// modelled `DeleteContainerImageRequest$image` documentation distinguishes: a
/// name "sourced from your Lightsail container service, that [is] registered and
/// stored on your service", which starts with a colon, and one "sourced from a
/// public registry like Docker Hub", which does not.
const IMAGE_NAMES: &[(&str, &str)] = &[
    (
        "a registered image name, in the form GetContainerImages reports",
        ":empty-registry.roundtrip.1",
    ),
    (
        "a registered image name at a later version",
        ":empty-registry.roundtrip.7",
    ),
    ("a public registry name with a tag", "nginx:latest"),
    ("a public registry name with no tag", "nginx"),
];

/// RULE: a container service nothing has been pushed to reports an EMPTY
/// registry — the list is present and carries no image — and a second read
/// reports the same, because the read is a report and changes nothing.
#[tokio::test]
async fn a_service_with_nothing_pushed_reports_an_empty_registry() {
    for row in SERVICE_ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.create_service(row.region, row.name).await;

        let registry = emulator.registry(row.region, row.name).await;
        assert!(
            registry.is_array(),
            "case {case}: expected a containerImages list, got {registry:?}"
        );
        assert_eq!(
            registry,
            json!([]),
            "case {case}: expected an empty registry, got {registry:?}"
        );
        assert_eq!(
            emulator.registry(row.region, row.name).await,
            registry,
            "case {case}: a second read must report the same registry"
        );
    }
}

/// RULE: the registry belongs to ONE (region, service name) pair, so a read in
/// another region, or of another name, does not reach it — such a read is the
/// service's `DoesNotExist` rejection rather than an empty list.
#[tokio::test]
async fn a_registry_is_reachable_only_in_its_own_region() {
    let emulator = Emulator::new();
    for row in SERVICE_ROWS {
        emulator.create_service(row.region, row.name).await;
    }

    for row in SERVICE_ROWS {
        // In its own region the service is found.
        assert!(
            emulator.images(row.region, row.name).await.is_empty(),
            "case {}: expected the service's own region to report its registry",
            row.case
        );
        // In every other region of the table the same name names nothing.
        for other in SERVICE_ROWS {
            if other.region == row.region {
                continue;
            }
            let case = format!("{} read from {}", row.case, other.region);
            let (status, refused) = emulator
                .call(
                    other.region,
                    "GetContainerImages",
                    json!({"serviceName": row.name}),
                )
                .await;
            assert_does_not_exist(&case, status, &refused);
        }
    }
}

/// RULE: a well-formed service name that names no container service in the region
/// the request is made in is the service's `DoesNotExist` rejection, for the read
/// and for the registration alike.
#[tokio::test]
async fn a_name_that_names_no_service_is_refused() {
    let emulator = Emulator::new();
    emulator.create_service("us-east-1", "present").await;

    let cases: &[(&str, &str, Value)] = &[
        (
            "a read of a name nothing created",
            "GetContainerImages",
            json!({"serviceName": "absent"}),
        ),
        (
            "a read of a name that is a prefix of one that exists",
            "GetContainerImages",
            json!({"serviceName": "pres"}),
        ),
        (
            "a registration on a name nothing created",
            "RegisterContainerImage",
            json!({"serviceName": "absent", "label": "web", "digest": "sha256:ab"}),
        ),
    ];

    for (case, operation, body) in cases {
        let (status, refused) = emulator.call("us-east-1", operation, body.clone()).await;
        assert_does_not_exist(case, status, &refused);
        assert!(
            refused["message"]
                .as_str()
                .is_some_and(|message| message.contains("absent") || message.contains("pres")),
            "case {case}: expected the message to name the identifier the request carried, \
             got {refused:?}"
        );
    }
}

/// RULE: `RegisterContainerImage` registers an image that is ALREADY in the
/// service's private registry, so a digest nothing has pushed there is refused
/// with the `DoesNotExist` envelope — whatever form the digest takes — and the
/// registry the service reports is unchanged by the attempt.
#[tokio::test]
async fn registering_a_digest_nothing_pushed_is_refused() {
    for service in SERVICE_ROWS {
        let emulator = Emulator::new();
        emulator.create_service(service.region, service.name).await;

        for digest in DIGEST_ROWS {
            let case = format!("{} on {}", digest.case, service.case);
            let (status, refused) = emulator
                .call(
                    service.region,
                    "RegisterContainerImage",
                    json!({
                        "serviceName": service.name,
                        "label": digest.label,
                        "digest": digest.digest,
                    }),
                )
                .await;
            assert_does_not_exist(&case, status, &refused);
            assert!(
                refused["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(digest.digest)),
                "case {case}: expected the message to name the digest the request carried, \
                 got {refused:?}"
            );
            assert!(
                emulator.images(service.region, service.name).await.is_empty(),
                "case {case}: a refused registration must leave the registry empty, got {:?}",
                emulator.images(service.region, service.name).await
            );
        }
    }
}

/// RULE: `DeleteContainerImage` is idempotent for a container service that
/// exists: the named image is dropped from that service's registry if it is
/// there, and a name the registry does not hold leaves it as it was — either way
/// the call answers success (HTTP 204, the operation's modelled success status)
/// with no body, and the registry stays readable.
#[tokio::test]
async fn deleting_an_image_the_registry_does_not_hold_still_succeeds() {
    for service in SERVICE_ROWS {
        let emulator = Emulator::new();
        emulator.create_service(service.region, service.name).await;

        for (case, image) in IMAGE_NAMES {
            let case = format!("{case} on {}", service.case);
            // Twice: an idempotent delete answers the same the second time.
            for attempt in 1..=2 {
                let (status, body) = emulator
                    .call(
                        service.region,
                        "DeleteContainerImage",
                        json!({"serviceName": service.name, "image": image}),
                    )
                    .await;
                assert_eq!(
                    status, 204,
                    "case {case} (attempt {attempt}): expected HTTP 204, got {status} {body:?}"
                );
            }
            assert!(
                emulator.images(service.region, service.name).await.is_empty(),
                "case {case}: the registry must still read as empty, got {:?}",
                emulator.images(service.region, service.name).await
            );
        }
    }
}

/// RULE: `DeleteContainerImage` naming no container service in the region the
/// request is made in is refused with the modelled `InvalidInputException` —
/// this operation's refusal, and not the `DoesNotExist` envelope the reporting
/// operations answer a missing service with — whose message names the service the
/// request carried.
#[tokio::test]
async fn deleting_from_a_service_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    emulator.create_service("us-east-1", "present").await;

    let cases: &[(&str, &str, &str)] = &[
        ("a name nothing created", "us-east-1", "absent"),
        (
            "a name created in another region only",
            "us-west-2",
            "present",
        ),
        ("a name that is a prefix of one that exists", "us-east-1", "pres"),
    ];

    for (case, region, service) in cases {
        let (status, refused) = emulator
            .call(
                region,
                "DeleteContainerImage",
                json!({"serviceName": service, "image": ":present.roundtrip.1"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidInputException",
            "case {case}: expected the InvalidInputException wire code, got {refused:?}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(format!("Service \"{service}\" does not exist.").as_str()),
            "case {case}: expected the message to name the service the request carried, \
             got {refused:?}"
        );
    }
}

/// RULE (the table's own source): the longest label in the table is exactly the
/// modelled `label` maximum, and every label in it is inside the modelled
/// `length 1..53`.
#[tokio::test]
async fn the_table_labels_match_the_modelled_bounds() {
    assert_eq!(
        LABEL_AT_MAX_LEN.len(),
        MAX_LABEL_LEN,
        "case the longest label: expected the modelled maximum of {MAX_LABEL_LEN} characters, \
         got {}",
        LABEL_AT_MAX_LEN.len()
    );
    for row in DIGEST_ROWS {
        assert!(
            (1..=MAX_LABEL_LEN).contains(&row.label.len()),
            "case {}: label {:?} is outside the modelled 1..{MAX_LABEL_LEN} length, got {}",
            row.case,
            row.label,
            row.label.len()
        );
    }
}

/// RULE: a member outside its modelled constraint — a label one character over
/// the modelled maximum, an empty label, a label the modelled pattern admits
/// nowhere — is refused before the handler runs, which is a different refusal
/// from the `DoesNotExist` one above.
#[tokio::test]
async fn a_label_outside_its_modelled_constraint_is_refused() {
    let emulator = Emulator::new();
    emulator.create_service("us-east-1", "constrained").await;

    let one_over_max = format!("{LABEL_AT_MAX_LEN}c");
    let cases: Vec<(String, Value)> = vec![
        (
            "a label one character over the modelled maximum".to_string(),
            json!({
                "serviceName": "constrained",
                "label": one_over_max,
                "digest": "sha256:ab",
            }),
        ),
        (
            "a label under the modelled minimum length".to_string(),
            json!({"serviceName": "constrained", "label": "", "digest": "sha256:ab"}),
        ),
        (
            // The modelled pattern `^[a-z0-9]{1,2}|[a-z0-9][a-z0-9-]+[a-z0-9]$`
            // is a Smithy pattern, i.e. a FIND and not a full match, so its
            // first alternative admits anything beginning with a lowercase
            // alphanumeric and its second anything ending in one. An
            // all-uppercase label satisfies neither.
            "a label the modelled pattern admits nowhere".to_string(),
            json!({"serviceName": "constrained", "label": "WEB", "digest": "sha256:ab"}),
        ),
    ];

    for (case, body) in cases {
        let (status, refused) = emulator
            .call("us-east-1", "RegisterContainerImage", body)
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} {refused:?}"
        );
        assert_ne!(
            refused["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: a modelled-constraint refusal is not a DoesNotExist, got {refused:?}"
        );
        assert!(
            emulator.images("us-east-1", "constrained").await.is_empty(),
            "case {case}: a refused registration must store nothing, got {:?}",
            emulator.images("us-east-1", "constrained").await
        );
    }
}
