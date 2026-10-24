// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tagging rules both creates hold a caller to.
//!
//! The rule under test: a create stores the `Tags` it was given only when EVERY entry is one
//! a caller may set — a key of 1 to [`MAX_TAG_KEY_LENGTH`] characters, a value of at most
//! [`MAX_TAG_VALUE_LENGTH`], both drawn from the tag alphabet (letters, numbers, spaces and
//! `_ . : / = + - @`), no key in the reserved `aws:` namespace, and at most
//! [`MAX_TAGS_PER_RESOURCE`] entries; any other map is refused with HTTP 400
//! `ValidationException` and nothing is created.
//!
//! `Tags` is modelled as a plain `map<String, String>` with no `@length` and no `@pattern`
//! on the map or on either half of an entry (smithy_query shape=Tags), so nothing is refused
//! at the request boundary and the rules above are the handlers' own; `ValidationException`'s
//! doc names exactly this case ("invalid characters in a name tag").
//!
//! Every case drives the service the way a client does — one HTTP request per call, through
//! the same `MultiServiceRouter` the emulator boots — and both operations that accept `Tags`
//! are put through the same table, so no assertion is pinned to one request. The rows differ
//! along what the rule generalises over: which half of the entry is at fault, the length, the
//! characters, the script those characters come from, the namespace of the key, and the
//! number of entries.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-1";

/// A private-CA ARN matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`).
const CA_ARN: &str = "arn:aws:acm-pca:eu-west-1:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555";

/// The per-resource tag ceiling, the tag-key length bound and the tag-value length bound,
/// named from the service's own constants so a row that sits on a bound cannot drift off it.
const MAX_TAGS_PER_RESOURCE: usize = scep::state::MAX_TAGS_PER_RESOURCE;
const MAX_TAG_KEY_LENGTH: usize = scep::state::MAX_TAG_KEY_LENGTH;
const MAX_TAG_VALUE_LENGTH: usize = scep::state::MAX_TAG_VALUE_LENGTH;

/// What one call answered: the status, the modelled error code (empty on success) and the
/// parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// One member of the response body, as a string.
    fn text(&self, name: &str) -> String {
        self.body[name]
            .as_str()
            .unwrap_or_else(|| panic!("expected a string `{name}` in {:?}", self.body))
            .to_string()
    }

    /// The returned `Tags` map, which an implemented listing always carries.
    fn tags(&self) -> &Map<String, Value> {
        self.body["Tags"]
            .as_object()
            .unwrap_or_else(|| panic!("expected a `Tags` map in {:?}", self.body))
    }

    /// How many entries a listing's item member carries (absent counts as none).
    fn count(&self, member: &str) -> usize {
        self.body[member].as_array().map_or(0, Vec::len)
    }
}

/// The router, with the shared engine it was built from, so a case that needs the connector's
/// clock-driven transition to be in a known position can put it there instead of racing it.
fn new_router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(scep::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Make the transition the clock owes `arn` due, so the next call settles `CREATING` into
/// `ACTIVE` and the connector can mint challenges.
fn release_pending(engine: &engine::InMemoryEngine, arn: &str) {
    let shared = scep::state::service_state(engine).expect("the service's own state");
    let mut store = shared.lock();
    if let Some(pending) = store
        .connectors
        .get_mut(arn)
        .and_then(|connector| connector.pending.as_mut())
    {
        pending.due_at = std::time::SystemTime::now() - Duration::from_secs(1);
    }
}

/// Percent-encode a value for use in a single `@httpLabel` path segment, the way an AWS SDK
/// does: every byte outside the unreserved set is escaped, so the `:` and `/` inside an ARN
/// cannot read as path structure.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://pca-connector-scep.{REGION}.amazonaws.com{path}"
        ))
        // The credential scope is how a handler learns the region its ARNs name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/pca-connector-scep/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/connectors", Some(request)).await
}

async fn create_challenge(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/challenges", Some(request)).await
}

async fn list_connectors(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/connectors", None).await
}

async fn list_challenge_metadata(router: &MultiServiceRouter, connector_arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/challengeMetadata?ConnectorArn={}", label(connector_arn)),
        None,
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", label(arn)), None).await
}

/// Create a connector carrying no tags and make the `ACTIVE` the clock owes it due, so it can
/// mint challenges.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
) -> String {
    let created = create_connector(router, json!({ "CertificateAuthorityArn": CA_ARN })).await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.text("ConnectorArn");
    release_pending(engine, &arn);
    arn
}

/// One row of the table: the `Tags` a create carries and whether they are a caller's to set.
#[derive(Debug)]
struct Case {
    name: &'static str,
    tags: Vec<(String, String)>,
    accepted: bool,
}

fn entry(key: &str, value: &str) -> Vec<(String, String)> {
    vec![(key.to_string(), value.to_string())]
}

/// The `Tags` member of a create request.
fn tags_member(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// What a listing should report for an accepted row.
fn expected_tags(tags: &[(String, String)]) -> Map<String, Value> {
    tags.iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect()
}

/// The rows.
///
/// The accepted ones walk the alphabet, the scripts and both length bounds; the refused ones
/// walk each way an entry can be outside the rule. The punctuation row carries the whole
/// published set of non-alphanumeric tag characters (`_ . : / = + - @` and the space), and
/// none of these keys is one the issue that prompted the rule cited.
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "a single-character key and value",
            tags: entry("k", "v"),
            accepted: true,
        },
        Case {
            name: "a key holding every punctuation character the tag alphabet allows",
            tags: entry("a_b.c:d/e=f+g-h@i", "1 2_3.4:5/6=7+8-9@0"),
            accepted: true,
        },
        Case {
            name: "letters and numbers from outside ASCII",
            tags: entry("équipe-sécurité", "東京 ٣"),
            accepted: true,
        },
        Case {
            name: "an empty value, which has no lower bound",
            tags: entry("owner", ""),
            accepted: true,
        },
        Case {
            name: "the reserved namespace away from the start of the key",
            tags: entry("team:aws:owner", "platform"),
            accepted: true,
        },
        Case {
            name: "a key opening with the reserved namespace's letters but not its colon",
            tags: entry("awsome", "yes"),
            accepted: true,
        },
        Case {
            name: "several entries, every one of them a caller's to set",
            tags: vec![
                ("environment".to_string(), "production".to_string()),
                ("cost:center".to_string(), "4711".to_string()),
                ("Name".to_string(), "scep connector".to_string()),
            ],
            accepted: true,
        },
        Case {
            name: "an empty key",
            tags: entry("", "empty-key"),
            accepted: false,
        },
        Case {
            name: "a character outside the tag alphabet in the key",
            tags: entry("bad*key", "x"),
            accepted: false,
        },
        Case {
            name: "a character outside the tag alphabet in the value",
            tags: entry("payload", "<not-a-tag-value>"),
            accepted: false,
        },
        Case {
            name: "a newline in the key, which is a control character and not a space",
            tags: entry("two\nlines", "x"),
            accepted: false,
        },
        Case {
            name: "a tab in the value, which is a control character and not a space",
            tags: entry("indented", "a\tb"),
            accepted: false,
        },
        Case {
            name: "a key in the reserved namespace",
            tags: entry("aws:cloudformation:stack-name", "mine"),
            accepted: false,
        },
        Case {
            name: "the reserved namespace spelled in capitals",
            tags: entry("AWS:Owner", "mine"),
            accepted: false,
        },
        Case {
            name: "one entry a caller may not set among entries it may",
            tags: vec![
                ("environment".to_string(), "production".to_string()),
                ("aws:created-by".to_string(), "me".to_string()),
                ("owner".to_string(), "platform".to_string()),
            ],
            accepted: false,
        },
    ];
    // The entry-count ceiling: the number a resource may carry, and one past it.
    for (name, count, accepted) in [
        ("the per-resource entry ceiling exactly", MAX_TAGS_PER_RESOURCE, true),
        (
            "one entry past the per-resource ceiling",
            MAX_TAGS_PER_RESOURCE + 1,
            false,
        ),
    ] {
        cases.push(Case {
            name,
            tags: (0..count)
                .map(|index| (format!("key-{index:03}"), format!("value-{index}")))
                .collect(),
            accepted,
        });
    }
    cases
}

/// Assert one row against one create's answer, and that an accepted row's tags are exactly
/// what the resource reports back while a refused row left nothing behind.
fn assert_answer(case: &Case, operation: &str, answer: &Answer) {
    if case.accepted {
        assert_eq!(
            answer.status, 202,
            "case {:?} on {operation}: expected 202, got {} / {:?}",
            case.name, answer.status, answer.body
        );
    } else {
        assert_eq!(
            answer.status, 400,
            "case {:?} on {operation}: expected 400, got {} / {:?}",
            case.name, answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ValidationException",
            "case {:?} on {operation}: expected ValidationException, got {:?} / {:?}",
            case.name, answer.error_type, answer.body
        );
        let message = answer.body["Message"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {:?} on {operation}: expected a Message saying why, got {:?}",
            case.name,
            answer.body
        );
    }
}

#[tokio::test]
async fn create_connector_stores_only_tags_a_caller_may_set() {
    for case in cases() {
        let (router, _engine) = new_router();
        let created = create_connector(
            &router,
            json!({ "CertificateAuthorityArn": CA_ARN, "Tags": tags_member(&case.tags) }),
        )
        .await;
        assert_answer(&case, "CreateConnector", &created);

        if case.accepted {
            let arn = created.text("ConnectorArn");
            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed.status, 200,
                "case {:?}: expected 200 from ListTagsForResource, got {} / {:?}",
                case.name, listed.status, listed.body
            );
            assert_eq!(
                listed.tags(),
                &expected_tags(&case.tags),
                "case {:?}: expected the created tags {:?}, got {:?}",
                case.name,
                expected_tags(&case.tags),
                listed.tags()
            );
        } else {
            // A refused create is not a half-done one: no connector was minted.
            let listed = list_connectors(&router).await;
            assert_eq!(
                listed.count("Connectors"),
                0,
                "case {:?}: a refused create must leave no connector, got {:?}",
                case.name,
                listed.body
            );
        }
    }
}

#[tokio::test]
async fn create_challenge_stores_only_tags_a_caller_may_set() {
    for case in cases() {
        let (router, engine) = new_router();
        let connector = active_connector(&router, &engine).await;
        let created = create_challenge(
            &router,
            json!({ "ConnectorArn": connector, "Tags": tags_member(&case.tags) }),
        )
        .await;
        assert_answer(&case, "CreateChallenge", &created);

        if case.accepted {
            let arn = created.body["Challenge"]["Arn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected a challenge Arn in {:?}",
                        case.name, created.body
                    )
                })
                .to_string();
            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed.status, 200,
                "case {:?}: expected 200 from ListTagsForResource, got {} / {:?}",
                case.name, listed.status, listed.body
            );
            assert_eq!(
                listed.tags(),
                &expected_tags(&case.tags),
                "case {:?}: expected the created tags {:?}, got {:?}",
                case.name,
                expected_tags(&case.tags),
                listed.tags()
            );
        } else {
            // Nothing was minted, so no challenge password was handed out either.
            let listed = list_challenge_metadata(&router, &connector).await;
            assert_eq!(
                listed.count("Challenges"),
                0,
                "case {:?}: a refused create must leave no challenge, got {:?}",
                case.name,
                listed.body
            );
        }
    }
}

/// The two length bounds, each at its published limit and one character past it, and in a
/// multi-byte script as well as in ASCII — the bound counts CHARACTERS, so a key of 128
/// two-byte letters is as acceptable as one of 128 ASCII ones.
#[tokio::test]
async fn tag_length_bounds_are_enforced_in_characters() {
    // (what the row varies, the character repeated, how many of it, which half of the entry)
    let mut rows: Vec<(String, String, String, bool)> = Vec::new();
    for (script, character) in [("ASCII", 'k'), ("two-byte letters", 'é'), ("CJK", '東')] {
        for (name, length, accepted) in [
            ("one character", 1, true),
            ("the published maximum", MAX_TAG_KEY_LENGTH, true),
            ("one past the published maximum", MAX_TAG_KEY_LENGTH + 1, false),
        ] {
            rows.push((
                format!("a key of {name} in {script}"),
                std::iter::repeat_n(character, length).collect(),
                "v".to_string(),
                accepted,
            ));
        }
        for (name, length, accepted) in [
            ("no characters", 0, true),
            ("the published maximum", MAX_TAG_VALUE_LENGTH, true),
            (
                "one past the published maximum",
                MAX_TAG_VALUE_LENGTH + 1,
                false,
            ),
        ] {
            rows.push((
                format!("a value of {name} in {script}"),
                "key".to_string(),
                std::iter::repeat_n(character, length).collect(),
                accepted,
            ));
        }
    }

    for (name, key, value, accepted) in rows {
        let (router, engine) = new_router();
        let expected = if accepted { 202 } else { 400 };
        let tags = json!({ key.clone(): value.clone() });

        let created = create_connector(
            &router,
            json!({ "CertificateAuthorityArn": CA_ARN, "Tags": tags.clone() }),
        )
        .await;
        assert_eq!(
            created.status, expected,
            "case {name:?} ({} key characters, {} value characters) on CreateConnector: \
             expected {expected}, got {} / {:?}",
            key.chars().count(),
            value.chars().count(),
            created.status,
            created.body
        );

        let connector = active_connector(&router, &engine).await;
        let challenge = create_challenge(
            &router,
            json!({ "ConnectorArn": connector, "Tags": tags }),
        )
        .await;
        assert_eq!(
            challenge.status, expected,
            "case {name:?} ({} key characters, {} value characters) on CreateChallenge: \
             expected {expected}, got {} / {:?}",
            key.chars().count(),
            value.chars().count(),
            challenge.status,
            challenge.body
        );
        if accepted {
            let arn = challenge.body["Challenge"]["Arn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {name:?}: expected a challenge Arn in {:?}", challenge.body)
                })
                .to_string();
            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed.tags().get(&key).and_then(Value::as_str),
                Some(value.as_str()),
                "case {name:?}: expected the stored tag to round-trip, got {:?}",
                listed.body
            );
        }
    }
}
