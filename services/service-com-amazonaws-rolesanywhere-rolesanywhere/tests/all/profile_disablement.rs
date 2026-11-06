// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DisableProfile`.
//!
//! The rule under test: the operation that turns one stored resource off writes that
//! resource's `enabled` flag to false in the store and answers with the whole stored
//! record as it now stands, so every later read reports the same thing; it touches no
//! other member, no sibling resource, and an id that resolves to nothing is refused
//! rather than answered.
//!
//! Each table's rows differ along what the rule generalises over: the shape of the
//! profile being disabled (which optional members its create carried, how many role and
//! managed-policy ARNs it holds, what `enabled` its create asked for), how many siblings
//! the store holds, how many times the call is repeated, and — on the refusal table —
//! why the id does not resolve and the Region the request is signed for.
//!
//! The harness below is local to this suite, as in each sibling suite.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the suite signs for unless a case varies it.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// Ids shaped like the modelled `Uuid`
/// (`@length(36, 36)`, `@pattern("[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}")`, from
/// `smithy_query(query="operation", operation="DisableProfile")`) that nothing in the store
/// is ever created under, so the boundary validator accepts them and the handler has to
/// resolve them.
const UNMINTED_IDS: [(&str, &str); 3] = [
    ("all-zero-id", "00000000-0000-0000-0000-000000000000"),
    ("hex-words-id", "deadbeef-dead-beef-dead-beefdeadbeef"),
    ("all-f-id", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
];

/// Ids OUTSIDE the modelled `Uuid` form: too short by one, too long by one, uppercase hex,
/// no separators, and not a uuid at all.
const MALFORMED_IDS: [&str; 5] = [
    "0123abcd-4567-89ef-0123-456789abcde",
    "0123abcd-4567-89ef-0123-456789abcdef0",
    "0123ABCD-4567-89EF-0123-456789ABCDEF",
    "0123abcd456789ef0123456789abcdef0123",
    "not-a-uuid",
];

struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body and the wire error code.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Answer {
    fn message(&self) -> Option<&str> {
        self.body.get("message").and_then(Value::as_str)
    }

    /// The `profile` member of a successful answer.
    fn profile(&self, case: &str) -> Value {
        self.body
            .pointer("/profile")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a profile member, got {self:?}"))
    }
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send_in_region(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        region: &str,
    ) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.send_in_region(method, uri, body, REGION).await
    }

    async fn create_profile(&self, body: Value) -> Answer {
        self.send("POST", "/profiles", Some(body)).await
    }

    async fn get_profile(&self, id: &str) -> Answer {
        self.send("GET", &format!("/profile/{id}"), None).await
    }

    async fn delete_profile(&self, id: &str) -> Answer {
        self.send("DELETE", &format!("/profile/{id}"), None).await
    }

    async fn list_profiles(&self) -> Answer {
        self.send("GET", "/profiles", None).await
    }

    async fn disable_profile(&self, id: &str) -> Answer {
        self.send("POST", &format!("/profile/{id}/disable"), None)
            .await
    }

    async fn disable_profile_in_region(&self, id: &str, region: &str) -> Answer {
        self.send_in_region("POST", &format!("/profile/{id}/disable"), None, region)
            .await
    }
}

/// An IAM role ARN a profile can vend credentials for.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The ARN a profile id carries in `region`, which is the ARN a refusal naming that id
/// words: `ProfileArn` is
/// `@pattern("^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:profile.*)$")`
/// (`smithy_query(query="shape", shape="ProfileDetail")`, member `profileArn`).
fn profile_arn(region: &str, profile_id: &str) -> String {
    format!("arn:aws:rolesanywhere:{region}:{ACCOUNT}:profile/{profile_id}")
}

/// The wording an ARN-keyed miss carries.
fn arn_not_found_message(resource_arn: &str) -> String {
    format!("The item doesn't exist. arn: {resource_arn} ; aborting.")
}

/// The body of a call that must have succeeded.
fn ok(case: &str, expected_status: u16, answer: &Answer) {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
    );
}

/// One profile detail with its `updatedAt` dropped, so two snapshots can be compared on
/// everything the instant of the last write is allowed to differ on.
fn without_updated_at(case: &str, detail: &Value) -> Value {
    let mut object = detail
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: a profile detail must be an object, got {detail:?}"))
        .clone();
    object.remove("updatedAt");
    Value::Object(object)
}

/// The instant one `date-time` member of a detail carries.
fn instant(case: &str, detail: &Value, member: &str) -> DateTime {
    let text = detail
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {member} must be an ISO-8601 string, got {detail:?}")
        });
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} {text:?} must parse as a date-time, got {error}")
    })
}

/// Whether a detail reports the profile as enabled.
fn enabled(case: &str, detail: &Value) -> bool {
    detail
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {case:?}: enabled must be a boolean, got {detail:?}"))
}

/// Create a profile and answer `(id, the detail the create returned)`.
async fn created(client: &Client, case: &str, body: Value) -> (String, Value) {
    let answer = client.create_profile(body).await;
    ok(case, 201, &answer);
    let detail = answer.profile(case);
    let id = detail
        .get("profileId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no profileId in {detail:?}"))
        .to_string();
    (id, detail)
}

/// The create bodies this suite disables: the rows span which optional members the create
/// carried, the sizes of the two ARN lists, and what `enabled` the create asked for.
fn profile_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal-enabled-omitted",
            json!({"name": "minimal", "roleArns": [role_arn("Minimal")]}),
        ),
        (
            "explicitly-enabled",
            json!({
                "name": "explicitly enabled",
                "roleArns": [role_arn("One"), role_arn("Two")],
                "enabled": true,
            }),
        ),
        (
            "created-already-disabled",
            json!({
                "name": "created disabled",
                "roleArns": [role_arn("Three")],
                "enabled": false,
            }),
        ),
        (
            "every-optional-member",
            json!({
                "name": "fully specified",
                "roleArns": [role_arn("Four"), role_arn("team/Five")],
                "managedPolicyArns": [
                    "arn:aws:iam::aws:policy/ReadOnlyAccess",
                    format!("arn:aws:iam::{ACCOUNT}:policy/Custom"),
                ],
                "sessionPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "durationSeconds": 900,
                "requireInstanceProperties": true,
                "acceptRoleSessionName": true,
                "tags": [{"key": "team", "value": "platform"}],
            }),
        ),
        (
            "no-roles-at-the-duration-ceiling",
            json!({
                "name": "no roles",
                "roleArns": [],
                "durationSeconds": 43200,
            }),
        ),
        (
            "many-roles",
            json!({
                "name": "many roles",
                "roleArns": (0..25).map(|index| role_arn(&format!("Bulk{index}")))
                    .collect::<Vec<String>>(),
            }),
        ),
    ]
}

/// `DisableProfile` clears `enabled` on the stored profile, leaves every other member as it
/// was, and is what every later read reports — whatever shape the profile has and whatever
/// `enabled` its create asked for.
///
/// The flag is written rather than toggled, so repeating the call reports the same state;
/// each call writes the record, so the instant it was last updated never goes backwards.
#[tokio::test]
async fn disable_profile_clears_enabled_and_persists_it() {
    for (case, body) in profile_rows() {
        let client = Client::new();
        // A sibling the case never addresses, so a write that reached the wrong record
        // shows up below.
        let (sibling_id, sibling_detail) =
            created(&client, &format!("{case}/sibling"), json!({
                "name": "untouched sibling",
                "roleArns": [role_arn("Sibling")],
            }))
            .await;
        let (id, create_detail) = created(&client, case, body).await;

        let disabled = client.disable_profile(&id).await;
        ok(case, 200, &disabled);
        let detail = disabled.profile(case);
        assert!(
            !enabled(case, &detail),
            "case {case:?}: DisableProfile must answer with enabled false, got {detail:?}"
        );
        // Everything else the create reported is still what it was: only the flag and the
        // instant of the write may differ.
        let mut expected = without_updated_at(case, &create_detail);
        expected["enabled"] = Value::Bool(false);
        assert_eq!(
            without_updated_at(case, &detail),
            expected,
            "case {case:?}: DisableProfile must touch no member but enabled, got {detail:?}"
        );
        let created_at = instant(case, &detail, "createdAt");
        let updated_at = instant(case, &detail, "updatedAt");
        assert!(
            updated_at.as_nanos() >= created_at.as_nanos(),
            "case {case:?}: updatedAt must not precede createdAt, got {created_at:?} and \
             {updated_at:?}"
        );

        // Persisted: the read path answers exactly what the write answered.
        let read = client.get_profile(&id).await;
        ok(case, 200, &read);
        assert_eq!(
            read.profile(case),
            detail,
            "case {case:?}: GetProfile must report the disabled profile, got {read:?}"
        );

        // Repeating the call reports the same state: the flag is written, not toggled.
        // Each call writes the record, so the instant it was last updated may move
        // forward but never back, and the read path keeps agreeing with the write.
        let mut previous = detail.clone();
        for repeat in 2..=3 {
            let labelled = format!("{case}/repeat-{repeat}");
            let again = client.disable_profile(&id).await;
            ok(&labelled, 200, &again);
            let repeated = again.profile(&labelled);
            assert_eq!(
                without_updated_at(&labelled, &repeated),
                without_updated_at(&labelled, &previous),
                "case {labelled:?}: a repeated DisableProfile must report the same profile, got \
                 {repeated:?}"
            );
            assert!(
                instant(&labelled, &repeated, "updatedAt").as_nanos()
                    >= instant(&labelled, &previous, "updatedAt").as_nanos(),
                "case {labelled:?}: updatedAt must not go backwards, got {repeated:?}"
            );
            let read_again = client.get_profile(&id).await;
            ok(&labelled, 200, &read_again);
            assert_eq!(
                read_again.profile(&labelled),
                repeated,
                "case {labelled:?}: GetProfile must report what the repeat wrote, got \
                 {read_again:?}"
            );
            previous = repeated;
        }

        // The sibling is untouched, flag and timestamps alike.
        let sibling = client.get_profile(&sibling_id).await;
        ok(&format!("{case}/sibling"), 200, &sibling);
        assert_eq!(
            sibling.profile(case),
            sibling_detail,
            "case {case:?}: no other profile may change, got {sibling:?}"
        );
    }
}

/// Disabling one profile is confined to that profile: with a store of many, exactly the
/// addressed ones report `enabled` false, on the read path and in the list projection.
///
/// The rows span how many of the stored profiles are disabled, and in what order.
#[tokio::test]
async fn disable_profile_is_confined_to_the_profile_it_addresses() {
    let cases: [(&str, usize, &[usize]); 5] = [
        ("one-of-one", 1, &[0]),
        ("none-of-four", 4, &[]),
        ("first-of-four", 4, &[0]),
        ("last-then-first-of-four", 4, &[3, 0]),
        ("all-of-three-in-reverse", 3, &[2, 1, 0]),
    ];

    for (case, total, to_disable) in cases {
        let client = Client::new();
        let mut ids: Vec<String> = Vec::new();
        for index in 0..total {
            let (id, _) = created(
                &client,
                &format!("{case}/create-{index}"),
                json!({
                    "name": format!("profile {index}"),
                    "roleArns": [role_arn(&format!("Role{index}"))],
                }),
            )
            .await;
            ids.push(id);
        }

        for index in to_disable {
            let answer = client.disable_profile(&ids[*index]).await;
            ok(&format!("{case}/disable-{index}"), 200, &answer);
        }

        let listed = client.list_profiles().await;
        ok(case, 200, &listed);
        let elements = listed
            .body
            .get("profiles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a profiles list, got {listed:?}"));
        assert_eq!(
            elements.len(),
            total,
            "case {case:?}: every stored profile must still be listed, got {listed:?}"
        );

        for (index, id) in ids.iter().enumerate() {
            let expected = !to_disable.contains(&index);
            let labelled = format!("{case}/profile-{index}");
            let read = client.get_profile(id).await;
            ok(&labelled, 200, &read);
            let detail = read.profile(&labelled);
            assert_eq!(
                enabled(&labelled, &detail),
                expected,
                "case {labelled:?}: expected enabled {expected}, got {detail:?}"
            );
            let element = elements
                .iter()
                .find(|entry| entry.get("profileId").and_then(Value::as_str) == Some(id.as_str()))
                .unwrap_or_else(|| panic!("case {labelled:?}: {id} must appear in {listed:?}"));
            assert_eq!(
                enabled(&labelled, element),
                expected,
                "case {labelled:?}: the list projection must report enabled {expected}, got \
                 {element:?}"
            );
        }
    }
}

/// An id that resolves to no stored profile is the modelled 404 carrying the ARN-keyed
/// wording — never a canned success — and an id outside the modelled `Uuid` form is a 400.
///
/// The rows span why the id does not resolve (never minted, deleted a moment ago) and the
/// Region the request is signed for, which is the only variable part of the message.
#[tokio::test]
async fn disable_profile_refuses_an_id_that_resolves_to_nothing() {
    let client = Client::new();

    let (sibling_id, sibling_detail) = created(
        &client,
        "sibling",
        json!({"name": "sibling", "roleArns": [role_arn("Sibling")]}),
    )
    .await;
    let (deleted_id, _) = created(
        &client,
        "deleted",
        json!({"name": "deleted", "roleArns": [role_arn("Deleted")]}),
    )
    .await;
    let removed = client.delete_profile(&deleted_id).await;
    ok("delete-the-deleted-fixture", 200, &removed);

    let mut cases: Vec<(String, String, &str)> = Vec::new();
    for (label, id) in UNMINTED_IDS {
        cases.push((format!("unminted-{label}"), id.to_string(), REGION));
    }
    cases.push(("deleted-profile-id".to_string(), deleted_id.clone(), REGION));
    for region in ["eu-central-1", "ap-southeast-2", "us-east-1"] {
        cases.push((
            format!("unminted-in-{region}"),
            UNMINTED_IDS[1].1.to_string(),
            region,
        ));
    }

    for (case, id, region) in cases {
        let expected = arn_not_found_message(&profile_arn(region, &id));
        let answer = client.disable_profile_in_region(&id, region).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an unresolvable profileId must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            Some(expected.as_str()),
            "case {case:?}: expected message {expected:?}, got {:?}",
            answer.message()
        );
    }

    for id in MALFORMED_IDS {
        let answer = client.disable_profile(id).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {id:?}: an id outside the modelled Uuid form must be refused, got {answer:?}"
        );
    }

    // Not one refusal wrote anything: the stored profile is still exactly as created.
    let still_there = client.get_profile(&sibling_id).await;
    ok("sibling-survives-the-refusals", 200, &still_there);
    assert_eq!(
        still_there.profile("sibling"),
        sibling_detail,
        "case \"sibling-survives-the-refusals\": a refused call must change nothing, got \
         {still_there:?}"
    );
}
