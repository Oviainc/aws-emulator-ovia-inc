// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeleteRoleAlias`: what removing a role alias does to the
//! store every other role-alias operation reads.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what an
//! AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: a single-resource delete of this control plane REMOVES the
//! record the requested name identifies, so every later reader of that store stops
//! reporting it and the name is free for a later create, while every other record is
//! left exactly as it was; a name the account holds no such record under is refused the
//! operation's modelled not-found error — with the same sentence the family's reader
//! refuses a miss with — rather than a hollow success. Here that means `DeleteRoleAlias`
//! drops the alias `CreateRoleAlias` recorded under `roleAlias`, after which
//! `DescribeRoleAlias` of that name answers 404 `ResourceNotFoundException`, and a
//! delete of a name the account holds no alias under is itself refused 404.
//!
//! The table spans what that rule generalises over — the form and length of a role alias
//! name, whether the create named a `credentialDurationSeconds` and where in the
//! modelled range it sat, whether it carried tags, and the region the call is signed for
//! — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// The bounds of `CredentialDurationSeconds`, which is modelled `range 900..43200`
/// (`smithy_query(query="operation",operation="CreateRoleAlias")`).
const MIN_CREDENTIAL_DURATION_SECONDS: i64 = 900;
const MAX_CREDENTIAL_DURATION_SECONDS: i64 = 43_200;

/// The longest legal `roleAlias`: the member is modelled `length 1..128`
/// (`smithy_query(query="operation",operation="DeleteRoleAlias")`).
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

    /// The `roleAliasDescription` a `DescribeRoleAlias` success carries.
    fn description(&self) -> Option<&Value> {
        self.body.get("roleAliasDescription")
    }

    /// Whether the body reports no members at all — `null` on an empty payload, or an
    /// object with no keys.
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

    /// `CreateRoleAlias`, bound to `POST /role-aliases/{roleAlias}` by this service's
    /// own model.
    async fn create_in(&self, name: &str, body: &Value, region: &str) -> Reply {
        self.call("POST", &format!("/role-aliases/{name}"), Some(body), region)
            .await
    }

    async fn create(&self, name: &str, body: &Value) -> Reply {
        self.create_in(name, body, REGION).await
    }

    /// `DescribeRoleAlias`, bound to `GET /role-aliases/{roleAlias}`.
    async fn describe(&self, name: &str) -> Reply {
        self.call("GET", &format!("/role-aliases/{name}"), None, REGION)
            .await
    }

    /// `DeleteRoleAlias`, bound to `DELETE /role-aliases/{roleAlias}`.
    async fn delete(&self, name: &str) -> Reply {
        self.delete_in(name, REGION).await
    }

    async fn delete_in(&self, name: &str, region: &str) -> Reply {
        self.call("DELETE", &format!("/role-aliases/{name}"), None, region)
            .await
    }
}

/// The sentence every reader of one role alias refuses a name the account holds no alias
/// under with, as the service words it with the name taken from the request.
fn missing_message(name: &str) -> String {
    format!("Role alias {name} not found")
}

/// One alias to create and then delete: a label, the name, the region the create is
/// signed for, and the create's body.
struct Case {
    label: &'static str,
    name: String,
    region: &'static str,
    body: Value,
}

/// The table every case list below is built from. It spans what the rule generalises
/// over:
///
/// * the form and length of `roleAlias` — modelled `length 1..128, pattern
///   `^[\w=,@-]+$`` — so the rows include the shortest legal name, the longest, and one
///   using every character class the pattern allows;
/// * whether the create named a `credentialDurationSeconds` and where in the modelled
///   `range 900..43200` it sat, since what a delete removes must not depend on it;
/// * whether the create carried tags;
/// * the region the create and the delete are signed for — a role alias is addressed by
///   name, so a delete must reach the alias whatever region scoped its ARN.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "only the required role ARN",
            name: "minimal_role_alias".to_string(),
            region: REGION,
            body: json!({ "roleArn": "arn:aws:iam::000000000000:role/device" }),
        },
        Case {
            label: "the shortest credential duration the range allows",
            name: "shortest_credentials".to_string(),
            region: REGION,
            body: json!({
                "roleArn": "arn:aws:iam::000000000000:role/short",
                "credentialDurationSeconds": MIN_CREDENTIAL_DURATION_SECONDS,
            }),
        },
        Case {
            label: "the longest credential duration the range allows",
            name: "longest_credentials".to_string(),
            region: REGION,
            body: json!({
                "roleArn": "arn:aws:iam::000000000000:role/long",
                "credentialDurationSeconds": MAX_CREDENTIAL_DURATION_SECONDS,
            }),
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
        },
        Case {
            label: "the shortest legal name, signed for another region",
            name: "a".to_string(),
            region: "eu-west-1",
            body: json!({ "roleArn": "arn:aws:iam::000000000000:role/eu" }),
        },
        Case {
            label: "the longest legal name, with the shortest role ARN that fits",
            name: "z".repeat(MAX_ROLE_ALIAS_LEN),
            region: "ap-southeast-2",
            body: json!({
                "roleArn": "arn:aws:iam::1:role/r",
                "credentialDurationSeconds": MAX_CREDENTIAL_DURATION_SECONDS,
            }),
        },
    ]
}

#[tokio::test]
async fn deleting_a_role_alias_removes_what_describe_reported() {
    for case in cases() {
        let label = case.label;
        // A fresh emulator per row, so what the delete removes is what this row created.
        let emulator = Emulator::new();

        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateRoleAlias must succeed, got {created:?}"
        );
        let before = emulator.describe(&case.name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: the created alias must be readable before the delete, got \
             {before:?}"
        );

        let deleted = emulator.delete_in(&case.name, case.region).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteRoleAlias must succeed, got {deleted:?}"
        );
        // `DeleteRoleAliasResponse` models no members: the success reports nothing, and
        // that the alias is gone is read back through `DescribeRoleAlias`.
        assert!(
            deleted.is_empty_payload(),
            "case {label:?}: a successful delete reports no members, got {deleted:?}"
        );

        let after = emulator.describe(&case.name).await;
        assert_eq!(
            after.status, 404,
            "case {label:?}: the deleted alias must be gone for every later reader, got \
             {after:?}"
        );
        assert_eq!(
            after.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {after:?}"
        );
        assert!(
            after.description().is_none(),
            "case {label:?}: a refusal carries no roleAliasDescription, got {after:?}"
        );
    }
}

#[tokio::test]
async fn deleting_one_role_alias_leaves_every_other_in_place() {
    let emulator = Emulator::new();

    // Every row of the table lives in one store, so each delete below is a delete of one
    // record out of many rather than of the only record there is.
    for case in cases() {
        let label = case.label;
        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateRoleAlias must succeed, got {created:?}"
        );
    }

    let mut remaining: Vec<Case> = cases();
    while !remaining.is_empty() {
        let case = remaining.remove(0);
        let label = case.label;
        // What every surviving alias reports right before the delete, so the comparison
        // after it is against a value this test read rather than one it rebuilt.
        let mut survivors = Vec::new();
        for other in &remaining {
            survivors.push((other.label, emulator.describe(&other.name).await));
        }

        let deleted = emulator.delete_in(&case.name, case.region).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteRoleAlias must succeed, got {deleted:?}"
        );

        for ((survivor_label, before), other) in survivors.into_iter().zip(&remaining) {
            let after = emulator.describe(&other.name).await;
            assert_eq!(
                after.status, 200,
                "case {label:?}: deleting it must leave {survivor_label:?} readable, got \
                 {after:?}"
            );
            assert_eq!(
                before.body, after.body,
                "case {label:?}: deleting it must leave {survivor_label:?} untouched, got \
                 {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_role_alias_the_store_does_not_hold_is_refused_not_found() {
    let emulator = Emulator::new();

    // One alias exists and stays, so the refusals below are about the NAME not being in
    // the store rather than the store being empty.
    let kept = json!({ "roleArn": "arn:aws:iam::000000000000:role/kept" });
    let created = emulator.create("existing_alias", &kept).await;
    assert_eq!(
        created.status, 200,
        "the setup CreateRoleAlias must succeed, got {created:?}"
    );

    // An alias created and then deleted: the SECOND delete of it is refused for exactly
    // the same reason a name nobody created is — the account holds no alias under it.
    let once = json!({ "roleArn": "arn:aws:iam::000000000000:role/once" });
    let created = emulator.create("deleted_once", &once).await;
    assert_eq!(
        created.status, 200,
        "the setup CreateRoleAlias must succeed, got {created:?}"
    );
    let deleted = emulator.delete("deleted_once").await;
    assert_eq!(
        deleted.status, 200,
        "the first DeleteRoleAlias must succeed, got {deleted:?}"
    );

    // The names differ in form, length and case — a role alias name is case-sensitive,
    // so a name that differs from an existing one only in case is a different name.
    for name in [
        "a",
        "unknown_alias",
        "deleted_once",
        "existing_alias_2",
        "EXISTING_ALIAS",
        "Missing_Alias=1,2@host-9",
        &"n".repeat(MAX_ROLE_ALIAS_LEN),
    ] {
        let refused = emulator.delete(name).await;
        assert_eq!(
            refused.status, 404,
            "case {name:?}: DeleteRoleAlias of a name the account holds no alias under \
             must be refused 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        // The refusal is the family's sentence with the name taken from the request, and
        // it is the SAME sentence the family's reader refuses a miss with, so the two
        // cannot drift.
        assert_eq!(
            refused.text("message"),
            Some(missing_message(name)),
            "case {name:?}: unexpected not-found message, got {refused:?}"
        );
        assert_eq!(
            refused.text("message"),
            emulator.describe(name).await.text("message"),
            "case {name:?}: the delete and the read must word the miss the same way, got \
             {refused:?}"
        );
    }

    // The refused deletes changed nothing: the alias that was there is still there.
    let survivor = emulator.describe("existing_alias").await;
    assert_eq!(
        survivor.status, 200,
        "a refused delete must leave the existing alias in place, got {survivor:?}"
    );
}

#[tokio::test]
async fn a_delete_frees_the_name_for_a_later_create() {
    let emulator = Emulator::new();

    // The name is the alias's identity, so while an alias holds it a create is refused;
    // once the delete removes the record the name is free again, and what the re-create
    // records is the NEW body rather than anything the deleted alias carried.
    for case in cases() {
        let label = case.label;
        let created = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the first CreateRoleAlias must succeed, got {created:?}"
        );

        let refused = emulator.create_in(&case.name, &case.body, case.region).await;
        assert_eq!(
            refused.status, 409,
            "case {label:?}: a create under a name the store holds must be refused 409, \
             got {refused:?}"
        );

        let deleted = emulator.delete_in(&case.name, case.region).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteRoleAlias must succeed, got {deleted:?}"
        );

        let recreated_body = json!({
            "roleArn": "arn:aws:iam::000000000000:role/recreated",
            "credentialDurationSeconds": 2_700,
        });
        let recreated = emulator
            .create_in(&case.name, &recreated_body, case.region)
            .await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: the name must be free after the delete, got {recreated:?}"
        );

        let read = emulator.describe(&case.name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the re-created alias must be readable, got {read:?}"
        );
        assert_eq!(
            read.description().and_then(|value| value.get("roleArn")),
            recreated_body.get("roleArn"),
            "case {label:?}: the re-created alias must report the new role ARN, got {read:?}"
        );
        assert_eq!(
            read.description()
                .and_then(|value| value.get("credentialDurationSeconds")),
            recreated_body.get("credentialDurationSeconds"),
            "case {label:?}: the re-created alias must report the new credential \
             duration, got {read:?}"
        );
        assert_eq!(
            read.description()
                .and_then(|value| value.get("roleAliasArn"))
                .and_then(Value::as_str),
            Some(
                format!(
                    "arn:aws:iot:{}:{ACCOUNT_ID}:rolealias/{}",
                    case.region, case.name
                )
                .as_str()
            ),
            "case {label:?}: the re-created alias must carry the ARN its region mints, \
             got {read:?}"
        );
    }
}
