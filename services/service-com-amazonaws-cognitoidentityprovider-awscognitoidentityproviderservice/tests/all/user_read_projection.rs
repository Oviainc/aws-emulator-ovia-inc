// SPDX-License-Identifier: CC0-1.0

//! What a user READ reports: the username a pool resolved a lookup to, and the
//! attributes it projects.
//!
//! Four rules are under test.
//!
//! * A registration does not PROVE the contact value it carries: a user "confirms
//!   ownership of the email address or phone number that they provided" by answering
//!   the code (`ConfirmSignUp` in this service's model), so `SignUp` records the
//!   verified flag of every verifiable attribute it stores (`email` ->
//!   `email_verified`, `phone_number` -> `phone_number_verified`, the two members of
//!   `VerifiedAttributeType`) as `false`, and both user reads project that flag
//!   beside the value until something confirms it.
//! * A `Username` an administrative create submits is a DUPLICATE whenever the pool
//!   already resolves it to one of its users — by username, by that user's `sub`, or
//!   by a value of one of the pool's sign-in or alias attributes that currently names
//!   it — because "the username can't be a duplicate of another username in the same
//!   user pool" (`AdminCreateUser.Username`) and a `Username` "can be any of their
//!   alias attributes ... [or] the sub of a local user" (`AdminGetUser.Username`). A
//!   sign-in value the create SUBMITS is the same duplicate, since an alias names one
//!   user of the pool and this operation models no `AliasExistsException`.
//! * One pool compares usernames the way its `UsernameConfiguration` says, and a read
//!   reports the STORED spelling whatever spelling resolved it; an alias value names
//!   the user that holds it NOW, and the user it was migrated away from stays
//!   readable under its own name.
//! * An administrator's verified flag of `true` asserts the value beside it is
//!   verified whether or not the request RESTATES that value, so the flag alone makes
//!   the user's stored value an active alias — and an alias value names one user, so
//!   every other holder of it loses it.
//!
//! Each table row names the case it asserts.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

use cognito_idp::state;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The attribute that records whether a stored `email` was confirmed.
const EMAIL_FLAG: &str = "email_verified";

/// The attribute that records whether a stored `phone_number` was confirmed.
const PHONE_FLAG: &str = "phone_number_verified";

/// Whether a request carries a SigV4 credential scope.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// A signed call, as the SDK sends every credentialed operation of this service.
    Signed,
    /// An unsigned call to the emulator's own endpoint: how the self-service
    /// operations are called.
    Anonymous,
}

/// The shared engine every router in one test mounts.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// One emulator with just this service mounted over `engine`.
fn router(engine: &Arc<engine::InMemoryEngine>, authenticating: bool) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(authenticating);
    router
}

/// Dispatch `operation` with `body`, addressed as `addressing` says, and return the
/// status and JSON response body.
async fn dispatch(
    router: &MultiServiceRouter,
    addressing: Addressing,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = match addressing {
        Addressing::Signed => ENDPOINT_HOST,
        Addressing::Anonymous => EMULATOR_HOST,
    };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
    if matches!(addressing, Addressing::Signed) {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, payload)
}

/// Call `operation` signed, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Addressing::Signed, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Call `operation` unsigned, requiring success, and return its response body.
async fn ok_anonymous(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Addressing::Anonymous, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool with `extra` merged into its configuration, plus an app client every
/// flow these tests use, and return both minted ids.
async fn pool_with_client(
    router: &MultiServiceRouter,
    pool_name: &str,
    extra: Value,
) -> (String, String) {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(pool_name));
    for (name, value) in extra.as_object().cloned().unwrap_or_default() {
        body.insert(name, value);
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "app",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    (user_pool_id, client_id)
}

/// A `SignUp` body carrying `attributes`.
fn sign_up_body(client_id: &str, username: &str, attributes: &[(&str, &str)]) -> Value {
    json!({
        "ClientId": client_id,
        "Username": username,
        "Password": GOOD_PASSWORD,
        "UserAttributes": attributes
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect::<Vec<Value>>(),
    })
}

/// The attribute list of a user read, as a name-to-value map.
fn attribute_map(read: &Value) -> BTreeMap<String, String> {
    read["UserAttributes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|attribute| {
            (
                attribute["Name"].as_str().unwrap_or_default().to_string(),
                attribute["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The attribute names a user read projects, in the order it projects them.
fn attribute_names(read: &Value) -> Vec<String> {
    read["UserAttributes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|attribute| attribute["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `AdminGetUser` for `username`, requiring success.
async fn admin_get_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> Value {
    ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await
}

/// The code outstanding for `purpose`, read out of the emulator's own state: the real
/// service delivers a code out of band, so no response carries it.
fn outstanding_code(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    purpose: &state::CodePurpose,
) -> Option<String> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .and_then(|pool| pool.user(username))
        .and_then(|user| user.outstanding_code(purpose))
        .map(|code| code.code.clone())
}

/// RULE: `SignUp` records the verified flag of every verifiable attribute it stores as
/// unconfirmed, so a read projects `<attribute>_verified` = `false` beside the value —
/// over a table whose rows differ along WHICH contact attributes the registration
/// carries, whether it submitted a flag itself, and whether the address arrived as the
/// `Username` of a pool that signs users in with it.
#[tokio::test]
async fn sign_up_seeds_an_unconfirmed_verification_flag_for_the_values_it_stores() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    let (open_pool, open_client) = pool_with_client(&setup, "flag-seeding", json!({})).await;

    // (case, username, submitted attributes, expected flag values)
    type Row = (
        &'static str,
        &'static str,
        Vec<(&'static str, &'static str)>,
        Vec<(&'static str, Option<&'static str>)>,
    );
    let rows: Vec<Row> = vec![
        (
            "an email address and nothing else",
            "flag-email",
            vec![("email", "flag-email@example.com")],
            vec![(EMAIL_FLAG, Some("false")), (PHONE_FLAG, None)],
        ),
        (
            "a phone number and nothing else",
            "flag-phone",
            vec![("phone_number", "+12065550100")],
            vec![(EMAIL_FLAG, None), (PHONE_FLAG, Some("false"))],
        ),
        (
            "both verifiable attributes at once",
            "flag-both",
            vec![
                ("email", "flag-both@example.com"),
                ("phone_number", "+12065550101"),
            ],
            vec![(EMAIL_FLAG, Some("false")), (PHONE_FLAG, Some("false"))],
        ),
        (
            "an attribute that carries no verified flag",
            "flag-none",
            vec![("name", "Flagless")],
            vec![(EMAIL_FLAG, None), (PHONE_FLAG, None)],
        ),
        (
            "no attribute at all",
            "flag-empty",
            vec![],
            vec![(EMAIL_FLAG, None), (PHONE_FLAG, None)],
        ),
        (
            "an email_verified the registration submitted itself",
            "flag-asserted",
            vec![
                ("email", "flag-asserted@example.com"),
                (EMAIL_FLAG, "true"),
            ],
            vec![(EMAIL_FLAG, Some("true")), (PHONE_FLAG, None)],
        ),
        (
            "a phone_number_verified the registration submitted itself",
            "flag-asserted-phone",
            vec![("phone_number", "+12065550102"), (PHONE_FLAG, "true")],
            vec![(EMAIL_FLAG, None), (PHONE_FLAG, Some("true"))],
        ),
    ];

    for (case, username, attributes, expected) in &rows {
        ok_anonymous(
            &emulator,
            "SignUp",
            &sign_up_body(&open_client, username, attributes),
        )
        .await;
        let read = admin_get_user(&setup, &open_pool, username).await;
        let stored = attribute_map(&read);
        for (flag, expected_value) in expected {
            assert_eq!(
                stored.get(*flag).map(String::as_str),
                *expected_value,
                "case {case:?}: expected {flag} = {expected_value:?}, got {stored:?}"
            );
        }
        // The projection is in attribute-name order, so a seeded flag is reported
        // beside its value and ahead of the service-generated `sub`.
        let names = attribute_names(&read);
        let mut ordered = names.clone();
        ordered.sort();
        assert_eq!(
            names, ordered,
            "case {case:?}: expected the attributes in name order, got {names:?}"
        );
    }

    // A pool whose users sign in WITH their email address: the submitted value is the
    // address, the pool mints the username, and the flag is seeded for the value it
    // stored for the sign-in attribute.
    let (email_pool, email_client) = pool_with_client(
        &setup,
        "flag-seeding-username-attributes",
        json!({ "UsernameAttributes": ["email"] }),
    )
    .await;
    let address = "sign-in-name@example.com";
    let registered = ok_anonymous(
        &emulator,
        "SignUp",
        &json!({
            "ClientId": email_client,
            "Username": address,
            "Password": GOOD_PASSWORD,
        }),
    )
    .await;
    let user_sub = registered["UserSub"]
        .as_str()
        .expect("a minted UserSub")
        .to_string();
    let read = admin_get_user(&setup, &email_pool, address).await;
    assert_eq!(
        read["Username"],
        json!(user_sub),
        "case \"an email sign-in name\": the username is the minted sub, got {read}"
    );
    let stored = attribute_map(&read);
    assert_eq!(
        stored.get("email").map(String::as_str),
        Some(address),
        "case \"an email sign-in name\": the address is stored as the email, got {stored:?}"
    );
    assert_eq!(
        stored.get(EMAIL_FLAG).map(String::as_str),
        Some("false"),
        "case \"an email sign-in name\": expected {EMAIL_FLAG} = false, got {stored:?}"
    );
    assert_eq!(
        attribute_names(&read),
        vec![
            "email".to_string(),
            EMAIL_FLAG.to_string(),
            "sub".to_string()
        ],
        "case \"an email sign-in name\": expected the value, its flag and the sub, got {read}"
    );

    // ROUND TRIP through the SELF-SERVICE read: the same unconfirmed flag is what the
    // signed-in user sees of its own profile. An administrative confirmation proves no
    // ownership, so the flag the registration seeded still says the address is not
    // confirmed.
    ok(
        &setup,
        "AdminConfirmSignUp",
        &json!({ "UserPoolId": open_pool, "Username": "flag-email" }),
    )
    .await;
    ok(
        &setup,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": open_pool,
            "Username": "flag-email",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let signed_in = ok(
        &setup,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": open_pool,
            "ClientId": open_client,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "flag-email", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let access_token = signed_in["AuthenticationResult"]["AccessToken"]
        .as_str()
        .expect("a sign-in issues an access token")
        .to_string();
    let own = ok_anonymous(
        &emulator,
        "GetUser",
        &json!({ "AccessToken": access_token }),
    )
    .await;
    let own_attributes = attribute_map(&own);
    assert_eq!(
        own_attributes.get(EMAIL_FLAG).map(String::as_str),
        Some("false"),
        "case \"the self-service read of an unverified registration\": expected \
         {EMAIL_FLAG} = false, got {own_attributes:?}"
    );
}

/// RULE: an administrative create is refused as a duplicate whenever the pool already
/// resolves the name it submits to one of its users, or already has a user answering
/// to a sign-in value it submits — over a table whose rows differ along WHICH way the
/// value resolves (the username, the user's `sub`, an alias value, an alias value sent
/// as an attribute), whether the holder's alias is confirmed, and whether the pool
/// configures that attribute as a sign-in name at all.
#[tokio::test]
async fn admin_create_user_refuses_every_name_the_pool_already_resolves() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    let shared = "claimed@example.com";
    let (alias_pool, alias_client) = pool_with_client(
        &setup,
        "alias-duplicates",
        json!({
            "AliasAttributes": ["email"],
            "AutoVerifiedAttributes": ["email"],
        }),
    )
    .await;
    // A pool that does NOT make email an alias, so the same address names no user
    // there: the refusal below comes from the POOL's configuration, not the value.
    let (plain_pool, _) = pool_with_client(&setup, "plain-duplicates", json!({})).await;

    // The alias holder: a registration whose confirmation proves the address, which is
    // what makes it an alias of that user.
    ok_anonymous(
        &emulator,
        "SignUp",
        &sign_up_body(&alias_client, "holder", &[("email", shared)]),
    )
    .await;
    let code = outstanding_code(
        &engine,
        &alias_pool,
        "holder",
        &state::CodePurpose::SignUpConfirmation,
    )
    .expect("the sign-up leaves a confirmation code outstanding");
    ok_anonymous(
        &emulator,
        "ConfirmSignUp",
        &json!({
            "ClientId": alias_client,
            "Username": "holder",
            "ConfirmationCode": code,
        }),
    )
    .await;
    let holder = admin_get_user(&setup, &alias_pool, "holder").await;
    let holder_sub = attribute_map(&holder)
        .get("sub")
        .cloned()
        .expect("the holder carries the sub the pool minted");

    // A registration whose address is NOT confirmed: a pending value is nobody's
    // alias yet, so it names no user and collides with nothing.
    let pending = "pending@example.com";
    ok_anonymous(
        &emulator,
        "SignUp",
        &sign_up_body(&alias_client, "pending-holder", &[("email", pending)]),
    )
    .await;

    // (case, pool, submitted username, submitted attributes, expected status,
    //  expected error code)
    type Row = (
        &'static str,
        String,
        String,
        Vec<(&'static str, &'static str)>,
        u16,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "the stored username itself",
            alias_pool.clone(),
            "holder".to_string(),
            vec![],
            400,
            Some("UsernameExistsException"),
        ),
        (
            "the sub of the stored user",
            alias_pool.clone(),
            holder_sub.clone(),
            vec![],
            400,
            Some("UsernameExistsException"),
        ),
        (
            "an alias value that names the stored user",
            alias_pool.clone(),
            shared.to_string(),
            vec![],
            400,
            Some("UsernameExistsException"),
        ),
        (
            "a fresh name carrying an alias value another user holds",
            alias_pool.clone(),
            "carries-a-held-alias".to_string(),
            vec![("email", shared)],
            400,
            Some("UsernameExistsException"),
        ),
        (
            "a name the pool resolves to nobody",
            alias_pool.clone(),
            "unclaimed".to_string(),
            vec![],
            200,
            None,
        ),
        (
            "a fresh name carrying an address nobody holds",
            alias_pool.clone(),
            "carries-a-free-alias".to_string(),
            vec![("email", "free@example.com")],
            200,
            None,
        ),
        (
            "a fresh name carrying an address whose holder never confirmed it",
            alias_pool.clone(),
            "carries-a-pending-alias".to_string(),
            vec![("email", pending)],
            200,
            None,
        ),
        (
            "a fresh name carrying a value of an attribute the pool aliases nobody by",
            alias_pool.clone(),
            "carries-a-shared-name".to_string(),
            vec![("name", "Shared Name")],
            200,
            None,
        ),
        (
            "the same address in a pool where email is no alias",
            plain_pool.clone(),
            shared.to_string(),
            vec![],
            200,
            None,
        ),
        (
            "the same address as an attribute in a pool where email is no alias",
            plain_pool.clone(),
            "carries-an-unaliased-address".to_string(),
            vec![("email", shared)],
            200,
            None,
        ),
    ];

    for (case, user_pool_id, username, attributes, expected_status, expected_code) in &rows {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("Username".to_string(), json!(username));
        if !attributes.is_empty() {
            body.insert(
                "UserAttributes".to_string(),
                Value::Array(
                    attributes
                        .iter()
                        .map(|(name, value)| json!({ "Name": name, "Value": value }))
                        .collect(),
                ),
            );
        }
        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminCreateUser",
            &Value::Object(body),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        match expected_code {
            Some(code) => {
                assert_eq!(
                    error_code(&payload),
                    *code,
                    "case {case:?}: expected error code {code}, got {payload}"
                );
                assert_eq!(
                    error_message(&payload),
                    "User account already exists",
                    "case {case:?}: expected the duplicate-account sentence, got {payload}"
                );
            }
            None => assert!(
                payload["User"]["Username"].is_string(),
                "case {case:?}: a served create reports the user it stored, got {payload}"
            ),
        }
    }

    // A refused create wrote nothing: the address still names the holder, with the
    // holder's own username and `sub`.
    let still = admin_get_user(&setup, &alias_pool, shared).await;
    assert_eq!(
        still["Username"],
        json!("holder"),
        "a refused create leaves the alias naming its holder, got {still}"
    );
    assert_eq!(
        attribute_map(&still).get("sub").map(String::as_str),
        Some(holder_sub.as_str()),
        "a refused create leaves the holder's sub in place, got {still}"
    );
}

/// RULE: an alias value names the user that holds it NOW — a confirmation that
/// migrates it re-points the lookup — and the user it was migrated away from stays a
/// readable account under its own name and its own `sub`.
#[tokio::test]
async fn a_migrated_alias_re_points_the_lookup_and_leaves_both_users_readable() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    let shared = "migrated@example.com";
    let (user_pool_id, client_id) = pool_with_client(
        &setup,
        "alias-migration",
        json!({
            "AliasAttributes": ["email"],
            "AutoVerifiedAttributes": ["email"],
        }),
    )
    .await;

    // Two registrations for the SAME address: the alias belongs to whichever
    // confirmation claims it, and the second one has to ask for the migration.
    let mut subs = BTreeMap::new();
    for username in ["alias-user-one", "alias-user-two"] {
        ok_anonymous(
            &emulator,
            "SignUp",
            &sign_up_body(&client_id, username, &[("email", shared)]),
        )
        .await;
        let code = outstanding_code(
            &engine,
            &user_pool_id,
            username,
            &state::CodePurpose::SignUpConfirmation,
        )
        .unwrap_or_else(|| panic!("case {username:?}: the sign-up leaves a code outstanding"));
        let force = username == "alias-user-two";
        let (status, payload) = dispatch(
            &emulator,
            Addressing::Anonymous,
            "ConfirmSignUp",
            &json!({
                "ClientId": client_id,
                "Username": username,
                "ConfirmationCode": code,
                "ForceAliasCreation": force,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {username:?}: the confirmation must be served, got {status} with {payload}"
        );
        let read = admin_get_user(&setup, &user_pool_id, username).await;
        subs.insert(
            username.to_string(),
            attribute_map(&read)
                .get("sub")
                .cloned()
                .unwrap_or_else(|| panic!("case {username:?}: the user carries a sub")),
        );
    }

    // The alias now names the claiming user, with that user's own username and sub.
    let by_alias = admin_get_user(&setup, &user_pool_id, shared).await;
    assert_eq!(
        by_alias["Username"],
        json!("alias-user-two"),
        "case \"a lookup by the migrated alias\": expected the claiming user, got {by_alias}"
    );
    assert_eq!(
        attribute_map(&by_alias).get("sub").map(String::as_str),
        subs.get("alias-user-two").map(String::as_str),
        "case \"a lookup by the migrated alias\": expected the claiming user's own sub, \
         got {by_alias}"
    );

    // (case, the name the read is addressed by, the user it must report)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "the username the alias was migrated AWAY from",
            "alias-user-one".to_string(),
            "alias-user-one",
        ),
        (
            "the sub of the user the alias was migrated away from",
            subs["alias-user-one"].clone(),
            "alias-user-one",
        ),
        (
            "the username that claimed the alias",
            "alias-user-two".to_string(),
            "alias-user-two",
        ),
        (
            "the sub of the user that claimed the alias",
            subs["alias-user-two"].clone(),
            "alias-user-two",
        ),
    ];

    for (case, addressed_by, expected_username) in &rows {
        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": addressed_by }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an existing account stays readable, got {status} with {payload}"
        );
        assert_eq!(
            payload["Username"],
            json!(expected_username),
            "case {case:?}: expected {expected_username}, got {payload}"
        );
        assert_eq!(
            payload["Enabled"],
            json!(true),
            "case {case:?}: the account is still activated for sign-in, got {payload}"
        );
        assert_eq!(
            payload["UserStatus"],
            json!("CONFIRMED"),
            "case {case:?}: the account is still confirmed, got {payload}"
        );
        assert!(
            payload["UserCreateDate"].is_number() && payload["UserLastModifiedDate"].is_number(),
            "case {case:?}: the read reports both timestamps, got {payload}"
        );
        let stored = attribute_map(&payload);
        assert_eq!(
            stored.get("email").map(String::as_str),
            Some(shared),
            "case {case:?}: the account keeps the address it registered, got {stored:?}"
        );
        assert!(
            stored.contains_key(EMAIL_FLAG),
            "case {case:?}: the read projects the address's verified flag, got {stored:?}"
        );
        assert_eq!(
            stored.get("sub").map(String::as_str),
            subs.get(*expected_username).map(String::as_str),
            "case {case:?}: the read projects that user's own sub, got {stored:?}"
        );
    }

    // Only ONE user holds the alias: the migration demoted the previous holder's flag,
    // so the two users' flags differ even though both keep the address.
    let previous = attribute_map(&admin_get_user(&setup, &user_pool_id, "alias-user-one").await);
    let claimant = attribute_map(&admin_get_user(&setup, &user_pool_id, "alias-user-two").await);
    assert_eq!(
        (
            previous.get(EMAIL_FLAG).map(String::as_str),
            claimant.get(EMAIL_FLAG).map(String::as_str),
        ),
        (Some("false"), Some("true")),
        "the migration leaves the alias confirmed for its claimant alone, got \
         {previous:?} and {claimant:?}"
    );
}

/// RULE: a pool compares usernames the way its `UsernameConfiguration` says, and a
/// read reports the STORED spelling whatever spelling resolved it — over a table whose
/// rows differ along the pool's case sensitivity, the creating operation and the
/// spelling the read is addressed by.
#[tokio::test]
async fn a_user_read_reports_the_stored_username_whatever_spelling_resolved_it() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    let (insensitive_pool, insensitive_client) = pool_with_client(
        &setup,
        "case-insensitive-reads",
        json!({ "UsernameConfiguration": { "CaseSensitive": false } }),
    )
    .await;
    let (sensitive_pool, sensitive_client) = pool_with_client(
        &setup,
        "case-sensitive-reads",
        json!({ "UsernameConfiguration": { "CaseSensitive": true } }),
    )
    .await;

    // An administrative create and a self-service registration, in each pool, with the
    // same mixed-case name: the stored spelling is the pool's, not the request's.
    ok(
        &setup,
        "AdminCreateUser",
        &json!({ "UserPoolId": insensitive_pool, "Username": "CaseUser-Mixed" }),
    )
    .await;
    ok_anonymous(
        &emulator,
        "SignUp",
        &sign_up_body(&insensitive_client, "SignedUp-Mixed", &[]),
    )
    .await;
    ok(
        &setup,
        "AdminCreateUser",
        &json!({ "UserPoolId": sensitive_pool, "Username": "CaseUser-Mixed" }),
    )
    .await;
    ok_anonymous(
        &emulator,
        "SignUp",
        &sign_up_body(&sensitive_client, "SignedUp-Mixed", &[]),
    )
    .await;

    // (case, pool, the spelling the read is addressed by, the username it must report)
    let rows: Vec<(&str, String, &str, Option<&str>)> = vec![
        (
            "a case-insensitive pool read by the submitted spelling",
            insensitive_pool.clone(),
            "CaseUser-Mixed",
            Some("caseuser-mixed"),
        ),
        (
            "a case-insensitive pool read by the folded spelling",
            insensitive_pool.clone(),
            "caseuser-mixed",
            Some("caseuser-mixed"),
        ),
        (
            "a case-insensitive pool read by an upper-cased spelling",
            insensitive_pool.clone(),
            "CASEUSER-MIXED",
            Some("caseuser-mixed"),
        ),
        (
            "a case-insensitive pool's self-service registration",
            insensitive_pool.clone(),
            "SIGNEDUP-MIXED",
            Some("signedup-mixed"),
        ),
        (
            "a case-sensitive pool read by the submitted spelling",
            sensitive_pool.clone(),
            "CaseUser-Mixed",
            Some("CaseUser-Mixed"),
        ),
        (
            "a case-sensitive pool read by another spelling, which names nobody",
            sensitive_pool.clone(),
            "caseuser-mixed",
            None,
        ),
        (
            "a case-sensitive pool's self-service registration",
            sensitive_pool.clone(),
            "SignedUp-Mixed",
            Some("SignedUp-Mixed"),
        ),
    ];

    for (case, user_pool_id, addressed_by, expected_username) in &rows {
        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": addressed_by }),
        )
        .await;
        match expected_username {
            Some(username) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the stored user, got {status} with {payload}"
                );
                assert_eq!(
                    payload["Username"],
                    json!(username),
                    "case {case:?}: expected Username {username}, got {payload}"
                );
            }
            None => {
                // The refusal class is what this rule fixes: a spelling this pool
                // compares case-sensitively names no user at all.
                assert!(
                    (400..500).contains(&status),
                    "case {case:?}: expected a client-error refusal, got {status} with {payload}"
                );
                assert_eq!(
                    error_code(&payload),
                    "UserNotFoundException",
                    "case {case:?}: expected UserNotFoundException, got {payload}"
                );
            }
        }
    }
}

/// RULE: an administrator vouching for a user's contact value — the verified flag of
/// `true`, with or without the value restated — makes that value the user's active
/// alias, and an alias value names ONE user, so every other holder of it is demoted —
/// over a table whose rows differ along the attribute vouched for, whether the request
/// restates the value, what the flag says, whether another user holds the value, and
/// whether the pool aliases that attribute at all.
#[tokio::test]
async fn an_administrator_vouching_for_a_value_claims_the_alias_for_the_user_it_names() {
    let engine = engine();
    let setup = router(&engine, false);
    let emulator = router(&engine, true);

    let address: &str = "claimable@example.com";
    let other_address: &str = "unclaimed-address@example.com";
    let number: &str = "+12065550150";

    // (case, the pool's alias attributes, the attribute, the holder's value, the
    //  claimant's value, the attributes the update submits, the username the
    //  claimant's value names afterwards, the holder's flag afterwards)
    type Row = (
        &'static str,
        Vec<&'static str>,
        &'static str,
        &'static str,
        &'static str,
        Vec<(&'static str, &'static str)>,
        Option<&'static str>,
        &'static str,
    );
    let rows: Vec<Row> = vec![
        (
            "an email_verified of true alone, where another user holds the address",
            vec!["email"],
            "email",
            address,
            address,
            vec![(EMAIL_FLAG, "true")],
            Some("claimant"),
            "false",
        ),
        (
            "an email_verified of true beside the address it vouches for",
            vec!["email"],
            "email",
            address,
            address,
            vec![("email", address), (EMAIL_FLAG, "true")],
            Some("claimant"),
            "false",
        ),
        (
            "an email_verified of false, which vouches for nothing",
            vec!["email"],
            "email",
            address,
            address,
            vec![(EMAIL_FLAG, "false")],
            Some("holder"),
            "true",
        ),
        (
            "an email_verified of true where no other user holds the address",
            vec!["email"],
            "email",
            other_address,
            address,
            vec![(EMAIL_FLAG, "true")],
            Some("claimant"),
            "true",
        ),
        (
            "a phone_number_verified of true alone, where another user holds the number",
            vec!["phone_number"],
            "phone_number",
            number,
            number,
            vec![(PHONE_FLAG, "true")],
            Some("claimant"),
            "false",
        ),
        (
            "a phone_number_verified of true where the pool aliases email instead",
            vec!["email"],
            "phone_number",
            number,
            number,
            vec![(PHONE_FLAG, "true")],
            None,
            "true",
        ),
        (
            "an email_verified of true in a pool that aliases nothing",
            vec![],
            "email",
            address,
            address,
            vec![(EMAIL_FLAG, "true")],
            None,
            "true",
        ),
    ];

    for (index, row) in rows.iter().enumerate() {
        let (
            case,
            alias_attributes,
            attribute,
            holder_value,
            claimant_value,
            submitted,
            names_afterwards,
            holder_flag_afterwards,
        ) = row;
        let flag = match *attribute {
            "email" => EMAIL_FLAG,
            _ => PHONE_FLAG,
        };
        let mut configuration = Map::new();
        if !alias_attributes.is_empty() {
            configuration.insert("AliasAttributes".to_string(), json!(alias_attributes));
        }
        let (user_pool_id, client_id) = pool_with_client(
            &setup,
            &format!("alias-assertion-{index}"),
            Value::Object(configuration),
        )
        .await;
        // The holder's value is vouched for at creation, so it is that user's active
        // alias wherever the pool aliases the attribute.
        ok(
            &setup,
            "AdminCreateUser",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": "holder",
                "UserAttributes": [
                    { "Name": attribute, "Value": holder_value },
                    { "Name": flag, "Value": "true" },
                ],
            }),
        )
        .await;
        // The claimant registers the value itself: a registration proves nothing, so
        // the value is not its alias yet.
        ok_anonymous(
            &emulator,
            "SignUp",
            &sign_up_body(&client_id, "claimant", &[(attribute, claimant_value)]),
        )
        .await;

        let updated = ok(
            &setup,
            "AdminUpdateUserAttributes",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": "claimant",
                "UserAttributes": submitted
                    .iter()
                    .map(|(name, value)| json!({ "Name": name, "Value": value }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
        assert_eq!(
            updated,
            json!({}),
            "case {case:?}: the update reports nothing of its own, got {updated}"
        );

        let holder = attribute_map(&admin_get_user(&setup, &user_pool_id, "holder").await);
        assert_eq!(
            holder.get(flag).map(String::as_str),
            Some(*holder_flag_afterwards),
            "case {case:?}: expected the holder's {flag} to be \
             {holder_flag_afterwards}, got {holder:?}"
        );
        assert_eq!(
            holder.get(*attribute).map(String::as_str),
            Some(*holder_value),
            "case {case:?}: the holder keeps the value it registered, got {holder:?}"
        );

        let (status, payload) = dispatch(
            &setup,
            Addressing::Signed,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": claimant_value }),
        )
        .await;
        match names_afterwards {
            Some(username) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the value must name a user, got {status} with {payload}"
                );
                assert_eq!(
                    payload["Username"],
                    json!(username),
                    "case {case:?}: expected the value to name {username}, got {payload}"
                );
            }
            None => {
                assert!(
                    (400..500).contains(&status),
                    "case {case:?}: a value the pool aliases nobody by names no user, \
                     got {status} with {payload}"
                );
                assert_eq!(
                    error_code(&payload),
                    "UserNotFoundException",
                    "case {case:?}: expected UserNotFoundException, got {payload}"
                );
            }
        }
    }
}
