// SPDX-License-Identifier: CC0-1.0

//! The account's default encryption configuration, end to end over the wire:
//! `PutDefaultEncryptionConfiguration` -> `GetDefaultEncryptionConfiguration`.
//!
//! RULE: the default encryption configuration is a per-account, per-REGION setting that
//! every account always has. `GetDefaultEncryptionConfiguration` (GET
//! `/configuration/account/encryption`, `@readonly`) reports the configuration stored for
//! the region the request was signed for, and the service default
//! (`MANAGED_INTEGRATIONS_DEFAULT_ENCRYPTION`, no `kmsKeyArn`, state `ENABLED`) for a
//! region nothing has configured — never a 404, and never anything a read itself wrote.
//! `PutDefaultEncryptionConfiguration` (POST on the same URI, success 201) stores the
//! submitted `encryptionType` and `kmsKeyArn` for that one region and answers with the
//! configuration as stored, so a later get reports exactly what the put wrote. Because
//! `kmsKeyArn` is the key `CUSTOMER_KEY_ENCRYPTION` encrypts under, the two members hold
//! together one way only — that type requires the key, the service-owned type takes none —
//! and either mismatch, like a value outside `EncryptionType`, is refused with the
//! modelled `ValidationException` (HTTP 400) while the stored configuration is left
//! untouched. A put that CHANGES the configuration starts an asynchronous switch, so it
//! reports `UPDATE_IN_PROGRESS` until the clock makes it due and `ENABLED` from then on; a
//! put asking for the configuration already in force starts no switch.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.default-encryption-configuration-lifecycle`) is ONE row of
//! the tables below. The rows differ along what the rule generalises over: the region the
//! request is signed for, the `EncryptionType` value (the model's whole value set), the
//! form of the `kmsKeyArn` the `@pattern` admits and its length at and past the modelled
//! maximum, and whether a put changes the configuration or restates it.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    CONFIGURATION_STATE_ENABLED, CONFIGURATION_STATE_UPDATE_IN_PROGRESS,
    CONFIGURATION_STATE_VALUES, CUSTOMER_KEY_ENCRYPTION_TYPE, DEFAULT_ENCRYPTION_TYPE,
    ENCRYPTION_TYPE_VALUES, ENCRYPTION_UPDATE_DELAY, MAX_CONFIGURED_REGIONS,
    MAX_REGION_NAME_LENGTH,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`), which
/// is also what the KMS key ARNs below are built for.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The route both operations are bound to
/// (`smithy_query(query="operation",operation="GetDefaultEncryptionConfiguration")`).
const ROUTE: &str = "/configuration/account/encryption";

/// The wire error code the model's `@httpError` 400 shape is rendered under.
const VALIDATION: &str = "ValidationException";

/// The `@length` maximum of `KmsKeyArn`, in characters
/// (`smithy_query(query="shape",shape="KmsKeyArn")` -> length 1..200).
const KMS_KEY_ARN_MAX_LENGTH: usize = 200;

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// The `ConfigurationState` on `configurationStatus`.
    fn state(&self) -> Option<&str> {
        self.member("configurationStatus")?
            .get("state")?
            .as_str()
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{ROUTE}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
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
        error_type,
        body,
    }
}

/// `GetDefaultEncryptionConfiguration`, signed for `region`.
async fn get_in(router: &MultiServiceRouter, region: &str) -> Reply {
    call_in(router, region, "GET", None).await
}

/// [`get_in`] in [`REGION`].
async fn get(router: &MultiServiceRouter) -> Reply {
    get_in(router, REGION).await
}

/// `PutDefaultEncryptionConfiguration`, signed for `region`, carrying `body`.
async fn put_in(router: &MultiServiceRouter, region: &str, body: Value) -> Reply {
    call_in(router, region, "POST", Some(body)).await
}

/// [`put_in`] in [`REGION`].
async fn put(router: &MultiServiceRouter, body: Value) -> Reply {
    put_in(router, REGION, body).await
}

/// The request body that asks for customer-key encryption under `kms_key_arn`.
fn customer_key(kms_key_arn: &str) -> Value {
    json!({"encryptionType": CUSTOMER_KEY_ENCRYPTION_TYPE, "kmsKeyArn": kms_key_arn})
}

/// The request body that asks for the service-owned default encryption.
fn service_default() -> Value {
    json!({"encryptionType": DEFAULT_ENCRYPTION_TYPE})
}

/// A KMS key ARN for `region` and `ACCOUNT` holding `key_id`, as the `KmsKeyArn`
/// `@pattern` (`^arn:aws:kms:[0-9a-zA-Z-]+:[0-9]+:key/[0-9a-zA-Z-]+$`) requires.
fn key_arn(region: &str, key_id: &str) -> String {
    format!("arn:aws:kms:{region}:{ACCOUNT}:key/{key_id}")
}

/// A key ARN for `region` whose total length is exactly `length` characters, by padding the
/// key id — so a case can sit on the modelled `@length` bound, or one character past it.
fn key_arn_of_length(region: &str, length: usize) -> String {
    let prefix = key_arn(region, "");
    assert!(
        length > prefix.len(),
        "a {length}-character key ARN cannot hold the {}-character prefix {prefix:?}",
        prefix.len()
    );
    format!("{prefix}{}", "a".repeat(length - prefix.len()))
}

/// Assert one answer reports exactly the configuration `encryption_type`/`kms_key_arn`
/// names, whatever `ConfigurationState` it is in.
fn assert_reports(case: &str, reply: &Reply, encryption_type: &str, kms_key_arn: Option<&str>) {
    assert_eq!(
        reply.string("encryptionType"),
        Some(encryption_type),
        "case {case:?}: expected encryptionType {encryption_type:?}, got {:?} in body {:?}",
        reply.string("encryptionType"),
        reply.body
    );
    assert_eq!(
        reply.string("kmsKeyArn"),
        kms_key_arn,
        "case {case:?}: expected kmsKeyArn {kms_key_arn:?}, got {:?} in body {:?}",
        reply.string("kmsKeyArn"),
        reply.body
    );
    let state = reply.state();
    assert!(
        state.is_some_and(|state| CONFIGURATION_STATE_VALUES.contains(&state)),
        "case {case:?}: expected a modelled ConfigurationState out of \
         {CONFIGURATION_STATE_VALUES:?}, got {state:?} in body {:?}",
        reply.body
    );
    assert!(
        reply
            .member("configurationStatus")
            .and_then(|status| status.get("error"))
            .is_none_or(Value::is_null),
        "case {case:?}: a configuration no update failed for carries no Error, got {:?}",
        reply.member("configurationStatus")
    );
}

/// Assert one answer is the modelled 400 refusal, and that its reason names `names`.
fn assert_refused(case: &str, reply: &Reply, names: &[&str]) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(VALIDATION),
        "case {case:?}: expected the modelled {VALIDATION} code, got {:?}",
        reply.error_type
    );
    let message = reply
        .string("Message")
        .or_else(|| reply.string("message"))
        .unwrap_or_default();
    for named in names {
        assert!(
            message.contains(named),
            "case {case:?}: expected the reason to name {named:?}, got {message:?}"
        );
    }
}

/// RULE: every account HAS a default encryption configuration in every region — before any
/// put, the service-owned default, enabled and with no customer key — and a get is a pure
/// read, so repeating it, and reading other regions, reports the same thing.
#[tokio::test]
async fn the_unconfigured_default_is_reported_in_every_region() {
    let router = emulator();

    // The rows differ along the one thing that selects a configuration: the region the
    // request is signed for.
    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        for read in 1..=3 {
            let case = format!("{region}/read {read}");
            let reply = get_in(&router, region).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {:?}",
                reply.status, reply.body
            );
            assert_reports(&case, &reply, DEFAULT_ENCRYPTION_TYPE, None);
            assert_eq!(
                reply.state(),
                Some(CONFIGURATION_STATE_ENABLED),
                "case {case:?}: a configuration nothing is changing is enabled, got {:?}",
                reply.state()
            );
        }
    }
}

/// RULE: a put stores the submitted configuration for its own region and a get reads back
/// exactly what it wrote — for every form of `kmsKeyArn` the modelled `@pattern` admits,
/// including one whose length sits on the modelled maximum — and switching back to the
/// service-owned default drops the customer key.
#[tokio::test]
async fn a_put_stores_the_submitted_configuration_and_a_get_reads_it_back() {
    // `KmsKeyArn`: `@length` 1..200, `@pattern`
    // `^arn:aws:kms:[0-9a-zA-Z-]+:[0-9]+:key/[0-9a-zA-Z-]+$`
    // (`smithy_query(query="shape",shape="KmsKeyArn")`). The rows differ along the form of
    // the key ARN: the dashed key-id form AWS mints, a key id of hex digits only, one of
    // letters and digits in mixed case, a key in a region other than the caller's, and one
    // whose total length is exactly the modelled maximum.
    let cases = vec![
        (
            "the dashed key id form",
            key_arn(REGION, "1234abcd-12ab-34cd-56ef-1234567890ab"),
        ),
        ("a key id of digits only", key_arn(REGION, "0123456789")),
        (
            "a key id in mixed case",
            key_arn(REGION, "AbCdEf0123456789xyz"),
        ),
        (
            "a key in another region than the caller's",
            key_arn("eu-west-1", "abcdef01-2345-6789-abcd-ef0123456789"),
        ),
        (
            "the modelled length maximum",
            key_arn_of_length(REGION, KMS_KEY_ARN_MAX_LENGTH),
        ),
    ];

    for (form, arn) in &cases {
        // A fresh store per row, so each row starts from the unconfigured default.
        let router = emulator();
        let case = format!("put {form}");

        let stored = put(&router, customer_key(arn)).await;
        assert_eq!(
            stored.status, 201,
            "case {case:?}: expected HTTP 201 from PutDefaultEncryptionConfiguration, got {} \
             with body {:?}",
            stored.status, stored.body
        );
        assert_reports(&case, &stored, CUSTOMER_KEY_ENCRYPTION_TYPE, Some(arn));

        let read_back = get(&router).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: expected HTTP 200 from GetDefaultEncryptionConfiguration, got {} \
             with body {:?}",
            read_back.status, read_back.body
        );
        assert_reports(
            &format!("{case} read back"),
            &read_back,
            CUSTOMER_KEY_ENCRYPTION_TYPE,
            Some(arn),
        );

        // Back to the service-owned encryption: the customer key is no longer part of the
        // configuration, so neither answer reports one.
        let reverted = put(&router, service_default()).await;
        assert_eq!(
            reverted.status, 201,
            "case {case:?}: expected HTTP 201 switching back to the default, got {} with body \
             {:?}",
            reverted.status, reverted.body
        );
        assert_reports(
            &format!("{case} reverted"),
            &reverted,
            DEFAULT_ENCRYPTION_TYPE,
            None,
        );
        let reverted_read = get(&router).await;
        assert_reports(
            &format!("{case} reverted read back"),
            &reverted_read,
            DEFAULT_ENCRYPTION_TYPE,
            None,
        );
    }
}

/// RULE: a put that CHANGES the configuration starts a switch that is not in force yet, so
/// it reports `UPDATE_IN_PROGRESS` and only the clock makes it `ENABLED`; a put that
/// restates the configuration already in force starts nothing and is enabled at once. The
/// submitted values are reported throughout, whichever state the configuration is in.
#[tokio::test]
async fn a_change_comes_into_force_by_the_clock_and_a_restatement_starts_nothing() {
    let router = emulator();
    let arn = key_arn(REGION, "11112222-3333-4444-5555-666677778888");

    // Restating the configuration already in force — the unconfigured account's own
    // default — changes nothing, so no switch is pending.
    let restated_default = put(&router, service_default()).await;
    assert_reports(
        "restate the default",
        &restated_default,
        DEFAULT_ENCRYPTION_TYPE,
        None,
    );
    assert_eq!(
        restated_default.state(),
        Some(CONFIGURATION_STATE_ENABLED),
        "case \"restate the default\": a put that changes nothing starts no update, got {:?}",
        restated_default.state()
    );

    // Changing the encryption starts the switch: the span is never zero, so the caller that
    // acts straight after the put still sees the transient state.
    let started = Instant::now();
    let changed = put(&router, customer_key(&arn)).await;
    assert_reports("change the key", &changed, CUSTOMER_KEY_ENCRYPTION_TYPE, Some(&arn));
    assert_eq!(
        changed.state(),
        Some(CONFIGURATION_STATE_UPDATE_IN_PROGRESS),
        "case \"change the key\": a put that changes the configuration starts a switch that is \
         not in force yet, got {:?}",
        changed.state()
    );

    // A read taken before the switch is due still reports it as in progress. Guarded on the
    // clock rather than assumed, so a slow machine cannot make this flaky.
    let pending = get(&router).await;
    if started.elapsed() < ENCRYPTION_UPDATE_DELAY {
        assert_eq!(
            pending.state(),
            Some(CONFIGURATION_STATE_UPDATE_IN_PROGRESS),
            "case \"read before the switch is due\": expected the switch to still be in \
             progress, got {:?}",
            pending.state()
        );
    }
    assert_reports(
        "read before the switch is due",
        &pending,
        CUSTOMER_KEY_ENCRYPTION_TYPE,
        Some(&arn),
    );

    // Once the clock has made it due, the configuration is in force — without anyone having
    // called in between.
    tokio::time::sleep(ENCRYPTION_UPDATE_DELAY + std::time::Duration::from_millis(250)).await;
    let settled = get(&router).await;
    assert_reports(
        "read after the switch is due",
        &settled,
        CUSTOMER_KEY_ENCRYPTION_TYPE,
        Some(&arn),
    );
    assert_eq!(
        settled.state(),
        Some(CONFIGURATION_STATE_ENABLED),
        "case \"read after the switch is due\": the clock brings the switch into force, got {:?}",
        settled.state()
    );

    // Restating a configuration that IS in force starts no second switch.
    let restated = put(&router, customer_key(&arn)).await;
    assert_reports(
        "restate the key",
        &restated,
        CUSTOMER_KEY_ENCRYPTION_TYPE,
        Some(&arn),
    );
    assert_eq!(
        restated.state(),
        Some(CONFIGURATION_STATE_ENABLED),
        "case \"restate the key\": a put that changes nothing starts no update, got {:?}",
        restated.state()
    );
}

/// RULE: `kmsKeyArn` belongs to customer-key encryption, so each `EncryptionType` admits
/// exactly one shape of request; a request the two members cannot hold together in, or one
/// naming a type outside the model's value set, is refused with the modelled
/// `ValidationException` (HTTP 400) and the stored configuration is left exactly as it was.
#[tokio::test]
async fn a_request_whose_members_cannot_hold_together_is_refused_and_stores_nothing() {
    let arn = key_arn(REGION, "99998888-7777-6666-5555-444433332222");
    let other_arn = key_arn(REGION, "aaaabbbb-cccc-dddd-eeee-ffff00001111");

    // The rows differ along WHY the request cannot be honoured: a required pairing missing,
    // a member the type does not take, a type outside the enum the boundary validator
    // enforces (including one AWS IoT SiteWise publishes but this service does not model,
    // an empty one, and the right value in the wrong case), and a `kmsKeyArn` outside the
    // modelled `@length` maximum or `@pattern`. Each refusal names the member at fault.
    let refusals: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "customer-key encryption without a key",
            json!({"encryptionType": CUSTOMER_KEY_ENCRYPTION_TYPE}),
            vec!["kmsKeyArn", CUSTOMER_KEY_ENCRYPTION_TYPE],
        ),
        (
            "the service-owned type with a key",
            json!({"encryptionType": DEFAULT_ENCRYPTION_TYPE, "kmsKeyArn": other_arn}),
            vec!["kmsKeyArn", DEFAULT_ENCRYPTION_TYPE],
        ),
        (
            "a type this service does not model",
            json!({"encryptionType": "KMS_BASED_ENCRYPTION", "kmsKeyArn": other_arn}),
            vec!["encryptionType"],
        ),
        (
            "an empty type",
            json!({"encryptionType": ""}),
            vec!["encryptionType"],
        ),
        (
            "the right type in the wrong case",
            json!({"encryptionType": "customer_key_encryption", "kmsKeyArn": other_arn}),
            vec!["encryptionType"],
        ),
        (
            "a key ARN one character past the length maximum",
            customer_key(&key_arn_of_length(REGION, KMS_KEY_ARN_MAX_LENGTH + 1)),
            vec!["kmsKeyArn"],
        ),
        (
            "a kmsKeyArn the pattern does not admit",
            customer_key("not-an-arn"),
            vec!["kmsKeyArn"],
        ),
    ];

    for (case, body, names) in &refusals {
        // Each row is driven twice: against the unconfigured default, and against a store a
        // put has already configured — a refusal must change neither.
        for configured in [false, true] {
            let router = emulator();
            let (expected_type, expected_key) = if configured {
                let seeded = put(&router, customer_key(&arn)).await;
                assert_eq!(
                    seeded.status, 201,
                    "case {case:?}: expected HTTP 201 seeding the configuration, got {} with \
                     body {:?}",
                    seeded.status, seeded.body
                );
                (CUSTOMER_KEY_ENCRYPTION_TYPE, Some(arn.as_str()))
            } else {
                (DEFAULT_ENCRYPTION_TYPE, None)
            };

            let case = format!("{case} ({})", if configured { "configured" } else { "default" });
            let refused = put(&router, body.clone()).await;
            assert_refused(&case, &refused, names);

            let after = get(&router).await;
            assert_eq!(
                after.status, 200,
                "case {case:?}: expected HTTP 200 reading after the refusal, got {} with body \
                 {:?}",
                after.status, after.body
            );
            assert_reports(
                &format!("{case} after the refusal"),
                &after,
                expected_type,
                expected_key,
            );
        }
    }
}

/// RULE: the configuration is per REGION — a put configures the region its request was
/// signed for and leaves every other region's configuration exactly as it was.
#[tokio::test]
async fn each_region_carries_its_own_configuration() {
    let router = emulator();
    let regions = ["us-east-1", "us-west-2", "eu-central-1"];

    // Configure each region with a key ARN of its own, one region at a time, checking after
    // every put that only that region moved.
    let mut configured: Vec<(&str, String)> = Vec::new();
    for region in regions {
        let arn = key_arn(region, &format!("key-for-{region}"));
        let stored = put_in(&router, region, customer_key(&arn)).await;
        assert_eq!(
            stored.status, 201,
            "case {region:?}: expected HTTP 201, got {} with body {:?}",
            stored.status, stored.body
        );
        configured.push((region, arn));

        for other in regions {
            let case = format!("after configuring {region}, read {other}");
            let reply = get_in(&router, other).await;
            match configured.iter().find(|(name, _)| *name == other) {
                Some((_, expected)) => assert_reports(
                    &case,
                    &reply,
                    CUSTOMER_KEY_ENCRYPTION_TYPE,
                    Some(expected.as_str()),
                ),
                // A region no put has reached still reports the account default.
                None => assert_reports(&case, &reply, DEFAULT_ENCRYPTION_TYPE, None),
            }
        }
    }
}

/// RULE: the store holds configurations for a BOUNDED number of regions. A put for a
/// region it already holds always succeeds, and one for a further region past
/// `MAX_CONFIGURED_REGIONS` is refused with the modelled `ThrottlingException` (HTTP 429)
/// rather than growing the store, so a caller that varies its signing region cannot make
/// the emulator hold configurations without end.
#[tokio::test]
async fn the_number_of_configured_regions_is_bounded() {
    let router = emulator();

    // Fill the store to its bound, one distinct signing region per put.
    let regions: Vec<String> = (0..MAX_CONFIGURED_REGIONS)
        .map(|index| format!("us-test-{index}"))
        .collect();
    for region in &regions {
        let stored = put_in(&router, region, customer_key(&key_arn(region, "boundkey"))).await;
        assert_eq!(
            stored.status, 201,
            "case {region:?}: expected HTTP 201 while under the bound, got {} with body {:?}",
            stored.status, stored.body
        );
    }

    // One region past the bound is refused, and nothing is stored for it.
    let past = "us-test-past-the-bound";
    let refused = put_in(&router, past, customer_key(&key_arn(past, "boundkey"))).await;
    assert_eq!(
        refused.status, 429,
        "case {past:?}: expected HTTP 429 past the bound of {MAX_CONFIGURED_REGIONS} regions, \
         got {} with body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ThrottlingException"),
        "case {past:?}: expected the modelled ThrottlingException code, got {:?}",
        refused.error_type
    );
    assert_reports(
        &format!("{past} after the refusal"),
        &get_in(&router, past).await,
        DEFAULT_ENCRYPTION_TYPE,
        None,
    );

    // A region the store already holds is still writable at the bound.
    let held = regions.last().expect("a configured region");
    let again = put_in(&router, held, service_default()).await;
    assert_eq!(
        again.status, 201,
        "case {held:?}: a region the store already holds stays writable at the bound, got {} \
         with body {:?}",
        again.status, again.body
    );
}

/// RULE: the region a configuration is stored under comes from the request's credential
/// scope, so a scope that does not name a region is refused with the modelled
/// `ValidationException` and nothing is stored for it.
#[tokio::test]
async fn a_credential_scope_that_names_no_region_is_refused() {
    let router = emulator();
    let arn = key_arn(REGION, "1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d");

    // The rows differ along how the scope's region fails to be one: upper case, characters
    // a region name does not use, and a string longer than any region AWS publishes.
    let scopes = [
        ("upper case", "US-EAST-1".to_string()),
        ("characters a region name never uses", "us_east_1".to_string()),
        (
            "longer than the longest region name",
            "a".repeat(MAX_REGION_NAME_LENGTH + 1),
        ),
    ];

    for (form, region) in &scopes {
        let refused = put_in(&router, region, customer_key(&arn)).await;
        assert_refused(form, &refused, &["region"]);

        // Nothing was stored for it: the read still reports the account default, and the
        // regions that ARE named properly are untouched.
        assert_reports(
            &format!("{form} after the refusal"),
            &get_in(&router, region).await,
            DEFAULT_ENCRYPTION_TYPE,
            None,
        );
        assert_reports(
            &format!("{form}, {REGION} untouched"),
            &get(&router).await,
            DEFAULT_ENCRYPTION_TYPE,
            None,
        );
    }
}

/// RULE: the values these handlers report come from the model, not from a transcription of
/// one recorded response — so each constant the implementation reports is a member of the
/// model-generated value set for its shape, and those sets are the model's whole sets.
#[tokio::test]
async fn the_reported_values_are_the_models_own() {
    // `smithy_query(query="shape",shape="EncryptionType")` -> exactly these two values.
    assert_eq!(
        ENCRYPTION_TYPE_VALUES.len(),
        2,
        "EncryptionType's value set is {ENCRYPTION_TYPE_VALUES:?}"
    );
    for value in [DEFAULT_ENCRYPTION_TYPE, CUSTOMER_KEY_ENCRYPTION_TYPE] {
        assert!(
            ENCRYPTION_TYPE_VALUES.contains(&value),
            "case {value:?}: expected a member of {ENCRYPTION_TYPE_VALUES:?}"
        );
    }

    // `smithy_query(query="shape",shape="ConfigurationState")` -> ENABLED,
    // UPDATE_IN_PROGRESS, UPDATE_FAILED.
    assert_eq!(
        CONFIGURATION_STATE_VALUES.len(),
        3,
        "ConfigurationState's value set is {CONFIGURATION_STATE_VALUES:?}"
    );
    for value in [
        CONFIGURATION_STATE_ENABLED,
        CONFIGURATION_STATE_UPDATE_IN_PROGRESS,
    ] {
        assert!(
            CONFIGURATION_STATE_VALUES.contains(&value),
            "case {value:?}: expected a member of {CONFIGURATION_STATE_VALUES:?}"
        );
    }

    // Both states this service reports are reachable over the wire: the enabled one from an
    // unconfigured account, the in-progress one from a put that changes the configuration.
    let router = emulator();
    let reported = vec![
        get(&router).await.state().map(str::to_string),
        put(
            &router,
            customer_key(&key_arn(REGION, "0a0a0a0a-1b1b-2c2c-3d3d-4e4e4e4e4e4e")),
        )
        .await
        .state()
        .map(str::to_string),
    ];
    assert_eq!(
        reported,
        vec![
            Some(CONFIGURATION_STATE_ENABLED.to_string()),
            Some(CONFIGURATION_STATE_UPDATE_IN_PROGRESS.to_string()),
        ],
        "expected the enabled state before any put and the in-progress state from a change, \
         got {reported:?}"
    );
}
