// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT ROLE ALIAS store: `CreateRoleAlias` and the
//! `DescribeRoleAlias` reader that proves what it persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what an
//! AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: a single-resource create of this control plane PERSISTS the
//! resource under the name that identifies it, resolving each absent optional member to
//! the default its documentation names, and the operation's single-resource reader
//! reports that record member for member; a create under a name the store already holds
//! is refused the operation's modelled already-exists error and changes nothing, and a
//! name the store does not hold is refused the reader's modelled not-found error rather
//! than a success. Here that means `CreateRoleAlias` records a role alias under its
//! `roleAlias` and answers the name and the ARN
//! (`arn:aws:iot:<region>:<account>:rolealias/<roleAlias>`) minted for it, and
//! `DescribeRoleAlias` reports that record as a `RoleAliasDescription` — the role ARN it
//! points at, the owning account, the credential duration (3,600 seconds when the create
//! named none) and the `creationDate` / `lastModifiedDate` the create stamped.
//!
//! The table spans what that rule generalises over — the form and length of a role alias
//! name, whether the create names a `credentialDurationSeconds` and where in the modelled
//! range it sits, the form and length of the role ARN, whether the create carries tags,
//! and the region the call is signed for — rather than one recorded request.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    DEFAULT_CREDENTIAL_DURATION_SECONDS, MAX_ROLE_ALIASES,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`),
/// and so the `owner` of every role alias this emulator records.
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// The bounds of `CredentialDurationSeconds`, which is modelled `range 900..43200`
/// (`smithy_query(query="operation",operation="CreateRoleAlias")`).
const MIN_CREDENTIAL_DURATION_SECONDS: i64 = 900;
const MAX_CREDENTIAL_DURATION_SECONDS: i64 = 43_200;

/// The longest legal `roleAlias`: the member is modelled `length 1..128`.
const MAX_ROLE_ALIAS_LEN: usize = 128;

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

    /// The `roleAliasDescription` member of a `DescribeRoleAlias` response.
    fn description(&self) -> Option<&Value> {
        self.body.get("roleAliasDescription")
    }

    /// A member of the reported `RoleAliasDescription`, or `None` when the description
    /// or the member is absent.
    fn member(&self, name: &str) -> Option<&Value> {
        self.description().and_then(|value| value.get(name))
    }

    /// A string member of the reported `RoleAliasDescription`.
    fn member_str(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// An integer member of the reported `RoleAliasDescription`.
    fn member_i64(&self, name: &str) -> Option<i64> {
        self.member(name).and_then(Value::as_i64)
    }

    /// A timestamp member as the epoch seconds the restJson1 protocol renders a
    /// timestamp as.
    fn member_epoch(&self, name: &str) -> Option<f64> {
        self.member(name).and_then(Value::as_f64)
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

    /// `CreateRoleAlias`, bound to `POST /role-aliases/{roleAlias}` by this service's
    /// own model.
    async fn create(&self, name: &str, body: &Value) -> Reply {
        self.create_in(name, body, REGION).await
    }

    async fn create_in(&self, name: &str, body: &Value, region: &str) -> Reply {
        self.call("POST", &format!("/role-aliases/{name}"), Some(body), region)
            .await
    }

    /// `DescribeRoleAlias`, bound to `GET /role-aliases/{roleAlias}`.
    async fn describe(&self, name: &str) -> Reply {
        self.call("GET", &format!("/role-aliases/{name}"), None, REGION)
            .await
    }
}

/// The ARN a role alias created in `region` must carry.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:rolealias/{name}")
}

/// Epoch seconds now, for bounding a reported timestamp.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One create request: a label, the role alias name, the region it is signed for, the
/// request body, and what the body's optional members resolve to once stored.
struct Case {
    label: &'static str,
    name: String,
    region: &'static str,
    body: Value,
    /// What `roleAliasDescription.credentialDurationSeconds` must report: the body's
    /// value, or [`DEFAULT_CREDENTIAL_DURATION_SECONDS`] when it carried none.
    credential_duration_seconds: i64,
}

/// The table every case list below is built from. It spans what the rule generalises
/// over:
///
/// * the form and length of `roleAlias` — modelled `length 1..128, pattern
///   `^[\w=,@-]+$`` (`smithy_query(query="operation",operation="CreateRoleAlias")`), so
///   the rows include the shortest legal name, the longest, and one using every
///   character class the pattern allows;
/// * whether the create names a `credentialDurationSeconds` and where in the modelled
///   `range 900..43200` it sits — the rows cover the absent member, both published
///   bounds and a value between them;
/// * the form and length of `roleArn` — modelled `length 20..2048` — so the rows cover
///   the shortest ARN that fits and a path-qualified role ARN;
/// * whether the create carries `tags`, which manage the alias rather than describe it
///   and so appear in no `RoleAliasDescription` member;
/// * the region the call is signed for, which is the region the alias's ARN is scoped
///   to.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "only the required role ARN",
            name: "minimal_role_alias".to_string(),
            region: REGION,
            body: json!({ "roleArn": "arn:aws:iam::000000000000:role/device" }),
            credential_duration_seconds: i64::from(DEFAULT_CREDENTIAL_DURATION_SECONDS),
        },
        Case {
            label: "the shortest credential duration the range allows",
            name: "shortest_credentials".to_string(),
            region: REGION,
            body: json!({
                "roleArn": "arn:aws:iam::000000000000:role/short",
                "credentialDurationSeconds": MIN_CREDENTIAL_DURATION_SECONDS,
            }),
            credential_duration_seconds: MIN_CREDENTIAL_DURATION_SECONDS,
        },
        Case {
            label: "the longest credential duration the range allows",
            name: "longest_credentials".to_string(),
            region: REGION,
            body: json!({
                "roleArn": "arn:aws:iam::000000000000:role/long",
                "credentialDurationSeconds": MAX_CREDENTIAL_DURATION_SECONDS,
            }),
            credential_duration_seconds: MAX_CREDENTIAL_DURATION_SECONDS,
        },
        Case {
            label: "a duration between the bounds and a path-qualified role ARN",
            name: "mid_credentials".to_string(),
            region: REGION,
            body: json!({
                "roleArn": "arn:aws:iam::000000000000:role/iot/devices/fleet-role",
                "credentialDurationSeconds": 7_200,
            }),
            credential_duration_seconds: 7_200,
        },
        Case {
            label: "a name using every character class the pattern allows, with tags",
            name: "Alias_01=aZ,x@y-z".to_string(),
            region: REGION,
            body: json!({
                "roleArn": "arn:aws:iam::000000000000:role/classes",
                "credentialDurationSeconds": 1_800,
                "tags": [
                    { "Key": "team", "Value": "platform" },
                    { "Key": "env", "Value": "test" },
                ],
            }),
            credential_duration_seconds: 1_800,
        },
        Case {
            label: "the shortest legal name, signed for another region",
            name: "a".to_string(),
            region: "eu-west-1",
            body: json!({ "roleArn": "arn:aws:iam::000000000000:role/eu" }),
            credential_duration_seconds: i64::from(DEFAULT_CREDENTIAL_DURATION_SECONDS),
        },
        Case {
            label: "the longest legal name, with the shortest role ARN that fits",
            name: "z".repeat(MAX_ROLE_ALIAS_LEN),
            region: "ap-southeast-2",
            body: json!({
                "roleArn": "arn:aws:iam::1:role/r",
                "credentialDurationSeconds": MAX_CREDENTIAL_DURATION_SECONDS,
            }),
            credential_duration_seconds: MAX_CREDENTIAL_DURATION_SECONDS,
        },
    ]
}

#[tokio::test]
async fn creating_a_role_alias_persists_what_describe_reports() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let before = now_epoch_seconds();

        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateRoleAlias must succeed, got {created:?}"
        );
        assert_eq!(
            created.text("roleAlias").as_deref(),
            Some(case.name.as_str()),
            "case {label:?}: the create must answer the name it recorded, got {created:?}"
        );
        assert_eq!(
            created.text("roleAliasArn"),
            Some(expected_arn(case.region, &case.name)),
            "case {label:?}: the create must answer the ARN it minted, got {created:?}"
        );

        let read = emulator.describe(&case.name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: DescribeRoleAlias must find the created alias, got {read:?}"
        );
        assert_eq!(
            read.member_str("roleAlias"),
            Some(case.name.as_str()),
            "case {label:?}: the description must report the created name, got {read:?}"
        );
        assert_eq!(
            read.member_str("roleAliasArn"),
            Some(expected_arn(case.region, &case.name).as_str()),
            "case {label:?}: the description must report the created ARN, got {read:?}"
        );
        assert_eq!(
            read.member("roleArn"),
            case.body.get("roleArn"),
            "case {label:?}: the description must report the persisted role ARN, got {read:?}"
        );
        assert_eq!(
            read.member_str("owner"),
            Some(ACCOUNT_ID),
            "case {label:?}: the description must report the owning account, got {read:?}"
        );
        assert_eq!(
            read.member_i64("credentialDurationSeconds"),
            Some(case.credential_duration_seconds),
            "case {label:?}: expected credentialDurationSeconds {}, got {read:?}",
            case.credential_duration_seconds
        );
        // Tags manage the alias rather than describe it, so no `RoleAliasDescription`
        // member reports them.
        assert!(
            read.member("tags").is_none(),
            "case {label:?}: RoleAliasDescription models no tags member, got {read:?}"
        );

        // `creationDate` is the instant of the create: it lies between the instant
        // before the create and the instant after this read, so it is a real clock
        // reading rather than a fixed or zero timestamp. Nothing has updated the alias,
        // so `lastModifiedDate` is that same instant.
        let created_at = read.member_epoch("creationDate").unwrap_or_else(|| {
            panic!("case {label:?}: the description reported no creationDate: {read:?}")
        });
        let after = now_epoch_seconds();
        assert!(
            (before - 1.0..=after + 1.0).contains(&created_at),
            "case {label:?}: creationDate {created_at} is outside [{before}, {after}], \
             got {read:?}"
        );
        assert_eq!(
            read.member_epoch("lastModifiedDate"),
            Some(created_at),
            "case {label:?}: an alias nothing has updated was last modified when it was \
             created ({created_at}), got {read:?}"
        );
    }
}

#[tokio::test]
async fn reading_a_role_alias_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    // `DescribeRoleAlias` reports the alias; it must not re-time, re-mint or otherwise
    // rewrite it, so two reads of one alias are byte-identical.
    for case in cases() {
        let label = case.label;
        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateRoleAlias must succeed, got {created:?}"
        );

        let first = emulator.describe(&case.name).await;
        let second = emulator.describe(&case.name).await;
        assert_eq!(
            first.status, second.status,
            "case {label:?}: both reads must answer the same status, got {second:?}"
        );
        assert_eq!(
            first.body, second.body,
            "case {label:?}: a read must not change the alias, got {first:?} then {second:?}"
        );
    }
}

#[tokio::test]
async fn a_repeat_create_of_one_name_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    // The name is the alias's identity, so a second create under a name the store holds
    // is refused 409 `ResourceAlreadyExistsException` whatever the second body says —
    // and the first alias is left exactly as it was.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "the same body again",
            "repeat_same",
            json!({ "roleArn": "arn:aws:iam::000000000000:role/a" }),
        ),
        (
            "a different role ARN",
            "repeat_role",
            json!({ "roleArn": "arn:aws:iam::000000000000:role/b" }),
        ),
        (
            "a body that asks for another credential duration",
            "repeat_duration",
            json!({
                "roleArn": "arn:aws:iam::000000000000:role/a",
                "credentialDurationSeconds": MAX_CREDENTIAL_DURATION_SECONDS,
            }),
        ),
        (
            "a body that adds tags",
            "repeat_tags",
            json!({
                "roleArn": "arn:aws:iam::000000000000:role/a",
                "tags": [{ "Key": "team", "Value": "platform" }],
            }),
        ),
    ];

    for (label, name, second_body) in rows {
        let first_body = json!({ "roleArn": "arn:aws:iam::000000000000:role/a" });
        let first = emulator.create(name, &first_body).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first CreateRoleAlias must succeed, got {first:?}"
        );
        let before = emulator.describe(name).await;

        let second = emulator.create(name, &second_body).await;
        assert_eq!(
            second.status, 409,
            "case {label:?}: a repeat create must be refused 409, got {second:?}"
        );
        assert_eq!(
            second.error_code.as_deref(),
            Some("ResourceAlreadyExistsException"),
            "case {label:?}: expected ResourceAlreadyExistsException, got {second:?}"
        );
        // The refusal's message is the service's create-conflict sentence with the name
        // taken from the request, not prose about this one request.
        assert_eq!(
            second.text("message"),
            Some(format!(
                "RoleAlias cannot be created - name already exists (name={name})"
            )),
            "case {label:?}: unexpected conflict message, got {second:?}"
        );
        // The refusal is built from the request: it names the alias that exists and the
        // ARN it was minted with.
        assert_eq!(
            second.text("resourceId").as_deref(),
            Some(name),
            "case {label:?}: the refusal must name the alias that exists, got {second:?}"
        );
        assert_eq!(
            second.text("resourceArn"),
            Some(expected_arn(REGION, name)),
            "case {label:?}: the refusal must name the existing alias's ARN, got {second:?}"
        );

        let after = emulator.describe(name).await;
        assert_eq!(
            before.body, after.body,
            "case {label:?}: the refused create must leave the alias untouched, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_role_alias_the_store_does_not_hold_is_refused_not_found() {
    let emulator = Emulator::new();

    // One alias exists, so the refusals below are about the NAME not being in the store
    // rather than the store being empty.
    let created = emulator
        .create(
            "existing_alias",
            &json!({ "roleArn": "arn:aws:iam::000000000000:role/a" }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the setup CreateRoleAlias must succeed, got {created:?}"
    );

    for name in [
        "a",
        "unknown_alias",
        "existing_alias_2",
        "EXISTING_ALIAS",
        &"a".repeat(MAX_ROLE_ALIAS_LEN),
    ] {
        let read = emulator.describe(name).await;
        assert_eq!(
            read.status, 404,
            "case {name:?}: DescribeRoleAlias of an unknown name must be refused 404, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: expected ResourceNotFoundException, got {read:?}"
        );
        assert!(
            read.description().is_none(),
            "case {name:?}: a refusal carries no roleAliasDescription, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_credential_duration_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();

    // `credentialDurationSeconds` is modelled `range 900..43200`, so a value past either
    // published bound is refused 400 `ValidationException` and records nothing — while
    // each bound itself is accepted (covered by the table above).
    for duration in [
        0,
        -1,
        MIN_CREDENTIAL_DURATION_SECONDS - 1,
        MAX_CREDENTIAL_DURATION_SECONDS + 1,
        86_400,
    ] {
        let name = format!("out_of_range_{}", duration.unsigned_abs());
        let refused = emulator
            .create(
                &name,
                &json!({
                    "roleArn": "arn:aws:iam::000000000000:role/a",
                    "credentialDurationSeconds": duration,
                }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {duration}: a duration outside the modelled range must be refused 400, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {duration}: expected ValidationException, got {refused:?}"
        );

        let read = emulator.describe(&name).await;
        assert_eq!(
            read.status, 404,
            "case {duration}: a refused create must record nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_create_past_the_stores_ceiling_is_refused_and_records_nothing() {
    let emulator = Emulator::new();

    // The store is one a caller grows, so it is bounded: the create that would be the
    // (MAX + 1)th alias is refused 410 `LimitExceededException` and records nothing,
    // while the aliases already in the store are untouched.
    for index in 0..MAX_ROLE_ALIASES {
        let created = emulator
            .create(
                &format!("ceiling_{index}"),
                &json!({ "roleArn": "arn:aws:iam::000000000000:role/a" }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "alias {index} of the ceiling must be created, got {created:?}"
        );
    }

    let refused = emulator
        .create(
            "one_too_many",
            &json!({ "roleArn": "arn:aws:iam::000000000000:role/a" }),
        )
        .await;
    assert_eq!(
        refused.status, 410,
        "a create past the ceiling must be refused 410, got {refused:?}"
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("LimitExceededException"),
        "expected LimitExceededException, got {refused:?}"
    );

    let read = emulator.describe("one_too_many").await;
    assert_eq!(
        read.status, 404,
        "the refused create must record nothing, got {read:?}"
    );
    let survivor = emulator.describe("ceiling_0").await;
    assert_eq!(
        survivor.status, 200,
        "the refused create must leave the existing aliases in place, got {survivor:?}"
    );
}
