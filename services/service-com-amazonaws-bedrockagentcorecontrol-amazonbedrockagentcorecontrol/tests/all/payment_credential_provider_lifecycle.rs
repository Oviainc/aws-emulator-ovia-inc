// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the payment credential provider
//! (`CreatePaymentCredentialProvider`, `GetPaymentCredentialProvider`).
//!
//! The rule these tables assert: a create stores each of the vendor's two secrets where
//! its `*Source` says — the SERVICE's own Secrets Manager secret for `MANAGED`, the
//! caller's own secret for `EXTERNAL` — and reports each one by the ARN Secrets Manager
//! holds it at, with the JSON key its value is extracted under and never the value
//! itself. Every read of the provider reports exactly that.
//!
//! Secrets Manager is mounted beside this service on ONE engine, the shape
//! `core/router/src/lib.rs` mounts them in, so every secret a provider reports is one
//! that service really holds and this suite reads the credential back out of it.
//! `tests/all/payment_credential_provider_external_secret.rs` adds KMS and pins what a
//! secret this service cannot decrypt, or cannot find, does to the create.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// `PaymentCredentialProviderVendorType`'s full value list, `CoinbaseCDP | StripePrivy`
/// (`smithy_query shape=PaymentCredentialProviderVendorType`).
const VENDOR_COINBASE_CDP: &str = "CoinbaseCDP";
const VENDOR_STRIPE_PRIVY: &str = "StripePrivy";

/// The `PaymentProviderConfigurationInput` / `…Output` member each vendor's configuration
/// is carried in (`smithy_query shape=PaymentProviderConfigurationInput`).
const COINBASE_MEMBER: &str = "coinbaseCdpConfiguration";
const PRIVY_MEMBER: &str = "stripePrivyConfiguration";

/// `SecretSourceType`'s full value list, `MANAGED | EXTERNAL`
/// (`smithy_query shape=SecretSourceType`).
const SOURCE_MANAGED: &str = "MANAGED";
const SOURCE_EXTERNAL: &str = "EXTERNAL";

/// This service and Secrets Manager over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router }
    }

    /// POST one `restJson1` operation of this service, signed for [`REGION`].
    async fn control_plane(&self, uri: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("host", "bedrock-agentcore-control.amazonaws.com")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        answer(self.router.dispatch(request).await)
    }

    /// POST one `awsJson1_1` Secrets Manager operation, signed for [`REGION`].
    async fn secrets_manager(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://secretsmanager.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("secretsmanager.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/secretsmanager/aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        answer(self.router.dispatch(request).await)
    }

    async fn secrets_manager_ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let (status, parsed) = self.secrets_manager(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Secrets Manager {operation}, got {status} \
             ({parsed})"
        );
        parsed
    }

    /// Create a secret of the CALLER's own holding `value`, and answer the ARN Secrets
    /// Manager reported for it.
    async fn create_caller_secret(&self, name: &str, value: &str, case: &str) -> String {
        let created = self
            .secrets_manager_ok(
                "CreateSecret",
                &json!({
                    "Name": name,
                    "SecretString": value,
                    // Secrets Manager stakes the version a create stores on a
                    // `ClientRequestToken` the caller names ("We recommend a UUID-type
                    // value"), so each secret carries one derived from its own name.
                    "ClientRequestToken": client_request_token(name),
                }),
                case,
            )
            .await;
        created["ARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateSecret reported no ARN in {created}"))
            .to_string()
    }

    /// The value Secrets Manager holds the secret `secret_id` at, as the JSON document a
    /// secret's value is.
    async fn secret_document(&self, secret_id: &str, case: &str) -> Value {
        let read = self
            .secrets_manager_ok("GetSecretValue", &json!({"SecretId": secret_id}), case)
            .await;
        let text = read["SecretString"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: the secret {secret_id} carries no SecretString in {read}")
        });
        serde_json::from_str(text).unwrap_or_else(|_| json!(text))
    }

    async fn create(&self, body: &Value) -> (u16, Value) {
        self.control_plane("/identities/CreatePaymentCredentialProvider", body)
            .await
    }

    async fn get(&self, name: &str) -> (u16, Value) {
        self.control_plane(
            "/identities/GetPaymentCredentialProvider",
            &json!({"name": name}),
        )
        .await
    }
}

fn answer(response: http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A UUID-shaped `ClientRequestToken` derived from a secret's name.
fn client_request_token(name: &str) -> String {
    let digits: String = name
        .bytes()
        .map(|byte| format!("{:02x}", byte % 16))
        .collect::<String>()
        .chars()
        .cycle()
        .take(32)
        .collect();
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `PaymentCredentialProviderArnType`'s form,
/// `^arn:(aws|aws-us-gov):acps:…:token-vault/…/paymentcredentialprovider/…$`
/// (`smithy_query shape=CreatePaymentCredentialProvider`): computed from the request's
/// Region, the emulator account, the account's default token vault and the name.
fn provider_arn(name: &str) -> String {
    format!(
        "arn:aws:acps:{REGION}:000000000000:token-vault/default/paymentcredentialprovider/{name}"
    )
}

/// Who manages one secret of a vendor configuration, and what it holds.
struct Slot {
    /// The member the secret is sent as, which is also the prefix of the three output
    /// members reporting it (`…Arn`, `…JsonKey`, `…Source`).
    member: &'static str,
    /// Whether the CALLER manages it.
    external: bool,
    /// Whether the request spells the `*Source` member out rather than leaving it to its
    /// default state.
    source_sent: bool,
    /// Whether an external secret is referenced by its whole ARN rather than by name.
    reference_by_arn: bool,
    /// The value the secret holds.
    value: &'static str,
    /// The JSON key the caller extracts an external secret's value under.
    json_key: &'static str,
}

/// One row of the table: one vendor configuration, its two secrets and its tags.
struct Row {
    case: &'static str,
    name: &'static str,
    vendor: &'static str,
    union_member: &'static str,
    identifiers: Value,
    tags: Option<Value>,
    slots: [Slot; 2],
}

/// Every combination of vendor and secret source the model admits is stored where its
/// source says and reported by the ARN its owner holds it at.
///
/// The rows differ along the vendor (which decides the union member the configuration is
/// carried in), along who manages each of that vendor's two secrets, along whether the
/// request spells `*Source` out or leaves it to its default state, along how an external
/// secret is referenced (by name or by whole ARN) and along whether the create carried
/// tags.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "Coinbase CDP, both secrets managed with no source member sent",
            name: "coinbase-managed",
            vendor: VENDOR_COINBASE_CDP,
            union_member: COINBASE_MEMBER,
            identifiers: json!({"apiKeyId": "cdp-api-key-1"}),
            tags: None,
            slots: [
                Slot {
                    member: "apiKeySecret",
                    external: false,
                    source_sent: false,
                    reference_by_arn: false,
                    value: "YXBpLWtleS1zZWNyZXQ=",
                    json_key: "",
                },
                Slot {
                    member: "walletSecret",
                    external: false,
                    source_sent: false,
                    reference_by_arn: false,
                    value: "d2FsbGV0LXNlY3JldA==",
                    json_key: "",
                },
            ],
        },
        Row {
            case: "Coinbase CDP, the API key external by name and the wallet managed",
            name: "coinbase_mixed",
            vendor: VENDOR_COINBASE_CDP,
            union_member: COINBASE_MEMBER,
            identifiers: json!({"apiKeyId": "cdp-api-key-2"}),
            tags: Some(json!({"team": "payments", "env": "test"})),
            slots: [
                Slot {
                    member: "apiKeySecret",
                    external: true,
                    source_sent: true,
                    reference_by_arn: false,
                    value: "ZXh0ZXJuYWwtYXBpLWtleQ==",
                    json_key: "apiKeySecret",
                },
                Slot {
                    member: "walletSecret",
                    external: false,
                    source_sent: true,
                    reference_by_arn: false,
                    value: "bWl4ZWQtd2FsbGV0",
                    json_key: "",
                },
            ],
        },
        Row {
            case: "Coinbase CDP, both secrets external, one referenced by its whole ARN",
            name: "coinbase-external",
            vendor: VENDOR_COINBASE_CDP,
            union_member: COINBASE_MEMBER,
            identifiers: json!({"apiKeyId": "cdp-api-key-3"}),
            tags: None,
            slots: [
                Slot {
                    member: "apiKeySecret",
                    external: true,
                    source_sent: true,
                    reference_by_arn: true,
                    value: "ZXh0ZXJuYWwtYnktYXJu",
                    json_key: "key",
                },
                Slot {
                    member: "walletSecret",
                    // An external secret is implied by the reference alone, so this row
                    // leaves `walletSecretSource` out and still gets EXTERNAL.
                    external: true,
                    source_sent: false,
                    reference_by_arn: false,
                    value: "ZXh0ZXJuYWwtd2FsbGV0",
                    json_key: "wallet",
                },
            ],
        },
        Row {
            case: "Stripe Privy, both secrets managed, the private key wallet-auth prefixed",
            name: "privy-managed",
            vendor: VENDOR_STRIPE_PRIVY,
            union_member: PRIVY_MEMBER,
            identifiers: json!({
                "appId": "privy-app-1",
                "authorizationId": "privy-authorization-1",
            }),
            tags: Some(json!({"owner": "checkout"})),
            slots: [
                Slot {
                    member: "appSecret",
                    external: false,
                    source_sent: false,
                    reference_by_arn: false,
                    value: "cHJpdnktYXBwLXNlY3JldA==",
                    json_key: "",
                },
                Slot {
                    member: "authorizationPrivateKey",
                    external: false,
                    source_sent: true,
                    reference_by_arn: false,
                    // `DefaultStripePrivyAuthorizationPrivateKeyType`'s pattern admits a
                    // `wallet-auth:` prefix (`smithy_query
                    // shape=StripePrivyConfigurationInput`).
                    value: "wallet-auth:cHJpdmF0ZS1rZXk=",
                    json_key: "",
                },
            ],
        },
        Row {
            case: "Stripe Privy, the app secret external and the private key managed",
            name: "privy-mixed",
            vendor: VENDOR_STRIPE_PRIVY,
            union_member: PRIVY_MEMBER,
            identifiers: json!({
                "appId": "privy-app-2",
                "authorizationId": "privy-authorization-2",
            }),
            tags: Some(json!({})),
            slots: [
                Slot {
                    member: "appSecret",
                    external: true,
                    source_sent: true,
                    reference_by_arn: false,
                    value: "ZXh0ZXJuYWwtYXBwLXNlY3JldA==",
                    json_key: "appSecret",
                },
                Slot {
                    member: "authorizationPrivateKey",
                    external: false,
                    source_sent: false,
                    reference_by_arn: false,
                    value: "bWl4ZWQtcHJpdmF0ZS1rZXk=",
                    json_key: "",
                },
            ],
        },
        Row {
            case: "Stripe Privy, both secrets external",
            name: "privy-external",
            vendor: VENDOR_STRIPE_PRIVY,
            union_member: PRIVY_MEMBER,
            identifiers: json!({
                "appId": "privy-app-3",
                "authorizationId": "privy-authorization-3",
            }),
            tags: None,
            slots: [
                Slot {
                    member: "appSecret",
                    external: true,
                    source_sent: true,
                    reference_by_arn: false,
                    value: "ZXh0ZXJuYWwtcHJpdnktYXBw",
                    json_key: "secret",
                },
                Slot {
                    member: "authorizationPrivateKey",
                    external: true,
                    source_sent: true,
                    reference_by_arn: true,
                    value: "ZXh0ZXJuYWwtcHJpdnkta2V5",
                    json_key: "privateKey",
                },
            ],
        },
    ]
}

#[tokio::test]
async fn every_vendor_configuration_is_stored_where_its_source_says_and_reported_by_arn() {
    let emulator = Emulator::new();

    for row in rows() {
        let mut configuration = row.identifiers.clone();
        // What the caller's own secrets are, before the provider references them: each
        // one really created in Secrets Manager, which is also the only place the ARN a
        // reference resolves to can come from.
        let mut owners_arns: [Option<String>; 2] = [None, None];
        for (index, slot) in row.slots.iter().enumerate() {
            if slot.external {
                let secret_name = format!("caller/{}/{}", row.name, slot.member);
                let owners_arn = emulator
                    .create_caller_secret(
                        &secret_name,
                        &json!({ slot.json_key: slot.value }).to_string(),
                        row.case,
                    )
                    .await;
                let referenced = if slot.reference_by_arn {
                    owners_arn.clone()
                } else {
                    secret_name
                };
                configuration[format!("{}Config", slot.member)] =
                    json!({"secretId": referenced, "jsonKey": slot.json_key});
                if slot.source_sent {
                    configuration[format!("{}Source", slot.member)] = json!(SOURCE_EXTERNAL);
                }
                owners_arns[index] = Some(owners_arn);
            } else {
                configuration[slot.member] = json!(slot.value);
                if slot.source_sent {
                    configuration[format!("{}Source", slot.member)] = json!(SOURCE_MANAGED);
                }
            }
        }

        let mut request = json!({
            "name": row.name,
            "credentialProviderVendor": row.vendor,
            "providerConfigurationInput": { row.union_member: configuration },
        });
        if let Some(tags) = row.tags.clone() {
            request["tags"] = tags;
        }

        let (status, created) = emulator.create(&request).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected HTTP 201 from CreatePaymentCredentialProvider, got {status} \
             ({created})",
            row.case
        );
        assert_eq!(
            (
                created["name"].clone(),
                created["credentialProviderVendor"].clone(),
                created["credentialProviderArn"].clone(),
            ),
            (
                json!(row.name),
                json!(row.vendor),
                json!(provider_arn(row.name)),
            ),
            "case {:?}: the create reports the provider it minted, got {created}",
            row.case
        );

        let reported = &created["providerConfigurationOutput"][row.union_member];
        assert_eq!(
            reported.get("apiKeyId").or_else(|| reported.get("appId")),
            row.identifiers
                .get("apiKeyId")
                .or_else(|| row.identifiers.get("appId")),
            "case {:?}: the vendor's identifiers are reported as sent, got {created}",
            row.case
        );

        for (index, slot) in row.slots.iter().enumerate() {
            let expected_source = if slot.external {
                SOURCE_EXTERNAL
            } else {
                SOURCE_MANAGED
            };
            assert_eq!(
                reported[format!("{}Source", slot.member)],
                json!(expected_source),
                "case {:?} ({}): the effective source is reported, got {created}",
                row.case,
                slot.member
            );
            let secret_arn = reported[format!("{}Arn", slot.member)]["secretArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?} ({}): no secret ARN reported in {created}",
                        row.case, slot.member
                    )
                })
                .to_string();
            let json_key = reported[format!("{}JsonKey", slot.member)]
                .as_str()
                .unwrap_or_default()
                .to_string();

            if let Some(owners_arn) = &owners_arns[index] {
                // The caller's own secret: reported by the ARN its owner holds it at,
                // which is not composable from the name the request carried, and under
                // the JSON key the caller named.
                assert_eq!(
                    (secret_arn.as_str(), json_key.as_str()),
                    (owners_arn.as_str(), slot.json_key),
                    "case {:?} ({}): the caller's secret is reported by the ARN Secrets \
                     Manager holds it at, got {created}",
                    row.case,
                    slot.member
                );
            } else {
                // A secret the SERVICE manages: it really exists in Secrets Manager, and
                // it holds the value the request handed over, under the JSON key the
                // provider reports.
                assert_eq!(
                    json_key, slot.member,
                    "case {:?} ({}): a managed secret is extracted under the member's own \
                     name, got {created}",
                    row.case, slot.member
                );
                let document = emulator.secret_document(&secret_arn, row.case).await;
                assert_eq!(
                    document[&json_key],
                    json!(slot.value),
                    "case {:?} ({}): the managed secret {secret_arn} holds the value the \
                     create was given, got {document}",
                    row.case,
                    slot.member
                );
            }
        }

        // The round trip: the same provider through the reader, which adds the two
        // timestamps and the tags the create carried.
        let (status, fetched) = emulator.get(row.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetPaymentCredentialProvider, got {status} \
             ({fetched})",
            row.case
        );
        for member in [
            "name",
            "credentialProviderVendor",
            "credentialProviderArn",
            "providerConfigurationOutput",
        ] {
            assert_eq!(
                fetched[member], created[member],
                "case {:?}: the read reports the same {member} as the create, got {fetched}",
                row.case
            );
        }
        let created_time = fetched["createdTime"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {:?}: createdTime is an epoch-seconds number, got {fetched}",
                row.case
            )
        });
        assert_eq!(
            fetched["lastUpdatedTime"].as_f64(),
            Some(created_time),
            "case {:?}: a provider nothing has updated was last updated when it was created, \
             got {fetched}",
            row.case
        );
        let expected_tags = match row.tags.clone() {
            Some(tags) if tags != json!({}) => tags,
            // A provider carrying no tag reports no `tags` member at all.
            _ => Value::Null,
        };
        assert_eq!(
            fetched.get("tags").cloned().unwrap_or(Value::Null),
            expected_tags,
            "case {:?}: the read reports exactly the tags the create carried, got {fetched}",
            row.case
        );

        // No response carries a secret VALUE: the output configuration is "no raw
        // secrets, only ARNs".
        let bodies = format!("{created}{fetched}");
        for slot in &row.slots {
            assert!(
                !bodies.contains(slot.value),
                "case {:?} ({}): a response carried the secret value {:?}, in {bodies}",
                row.case,
                slot.member,
                slot.value
            );
        }
    }

    // Each provider is still readable after the others were created: one store, keyed by
    // the name each create carried.
    for row in rows() {
        let (status, fetched) = emulator.get(row.name).await;
        assert_eq!(
            (status, fetched["credentialProviderArn"].clone()),
            (200, json!(provider_arn(row.name))),
            "case {:?}: the provider is still readable beside the others, got {fetched}",
            row.case
        );
    }
}

/// A Coinbase CDP configuration whose two secrets the service manages.
fn coinbase_managed(api_key: &str, wallet: &str) -> Value {
    json!({
        "coinbaseCdpConfiguration": {
            "apiKeyId": "cdp-api-key",
            "apiKeySecret": api_key,
            "walletSecret": wallet,
        }
    })
}

/// A second create of a name this account already holds is refused, and leaves the
/// provider that holds the name exactly as it was.
#[tokio::test]
async fn a_name_already_held_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    let (status, created) = emulator
        .create(&json!({
            "name": "held-name",
            "credentialProviderVendor": VENDOR_COINBASE_CDP,
            "providerConfigurationInput": coinbase_managed("Zmlyc3Qtc2VjcmV0", "Zmlyc3Qtd2FsbGV0"),
        }))
        .await;
    assert_eq!(
        status, 201,
        "the provider a later conflict must leave untouched is created, got {status} ({created})"
    );

    // (case, the vendor and configuration the second create carries)
    let rows: [(&str, Value, Value); 2] = [
        (
            "the same vendor and a different configuration",
            json!(VENDOR_COINBASE_CDP),
            coinbase_managed("c2Vjb25kLXNlY3JldA==", "c2Vjb25kLXdhbGxldA=="),
        ),
        (
            "another vendor entirely",
            json!(VENDOR_STRIPE_PRIVY),
            json!({
                "stripePrivyConfiguration": {
                    "appId": "second-app",
                    "authorizationId": "second-authorization",
                    "appSecret": "c2Vjb25kLWFwcC1zZWNyZXQ=",
                    "authorizationPrivateKey": "c2Vjb25kLWtleQ==",
                }
            }),
        ),
    ];

    for (case, vendor, configuration) in rows {
        let (status, answer) = emulator
            .create(&json!({
                "name": "held-name",
                "credentialProviderVendor": vendor,
                "providerConfigurationInput": configuration,
            }))
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected HTTP 409 ConflictException, got {status} ({answer})"
        );

        let (status, fetched) = emulator.get("held-name").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the provider holding the name is still readable, got {status} \
             ({fetched})"
        );
        assert_eq!(
            fetched["providerConfigurationOutput"], created["providerConfigurationOutput"],
            "case {case:?}: the refused create left the stored configuration as it was, got \
             {fetched}"
        );
    }
}

/// What the handler refuses a configuration for, and the member each refusal names.
///
/// The rows span the rule both ways: the vendor decides which union member carries the
/// credentials, a secret the CALLER manages needs the reference that names it and that
/// reference has to name a secret of THIS account in the request's own Region, and a
/// secret the SERVICE manages needs the value it is to hold. Every refused request leaves
/// nothing behind.
#[tokio::test]
async fn a_configuration_the_vendor_or_the_secret_source_refuses_is_a_validation_failure() {
    let emulator = Emulator::new();

    // (case, name, vendor, providerConfigurationInput, the member the refusal names)
    let rows: [(&str, &str, &str, Value, &str); 12] = [
        (
            "the Stripe Privy configuration under the Coinbase CDP vendor",
            "mismatch-coinbase",
            VENDOR_COINBASE_CDP,
            json!({
                "stripePrivyConfiguration": {
                    "appId": "app",
                    "authorizationId": "authorization",
                    "appSecret": "c2VjcmV0",
                    "authorizationPrivateKey": "a2V5",
                }
            }),
            "providerConfigurationInput",
        ),
        (
            "the Coinbase CDP configuration under the Stripe Privy vendor",
            "mismatch-privy",
            VENDOR_STRIPE_PRIVY,
            coinbase_managed("c2VjcmV0", "d2FsbGV0"),
            "providerConfigurationInput",
        ),
        (
            "an external API key secret with no reference to it",
            "external-api-key-unreferenced",
            VENDOR_COINBASE_CDP,
            json!({
                "coinbaseCdpConfiguration": {
                    "apiKeyId": "api-key",
                    "apiKeySecretSource": SOURCE_EXTERNAL,
                    "walletSecret": "d2FsbGV0",
                }
            }),
            "apiKeySecretConfig",
        ),
        (
            "an external wallet secret with no reference to it",
            "external-wallet-unreferenced",
            VENDOR_COINBASE_CDP,
            json!({
                "coinbaseCdpConfiguration": {
                    "apiKeyId": "api-key",
                    "apiKeySecret": "c2VjcmV0",
                    "walletSecretSource": SOURCE_EXTERNAL,
                }
            }),
            "walletSecretConfig",
        ),
        (
            "an external app secret with no reference to it",
            "external-app-secret-unreferenced",
            VENDOR_STRIPE_PRIVY,
            json!({
                "stripePrivyConfiguration": {
                    "appId": "app",
                    "authorizationId": "authorization",
                    "appSecretSource": SOURCE_EXTERNAL,
                    "authorizationPrivateKey": "a2V5",
                }
            }),
            "appSecretConfig",
        ),
        (
            "an external authorization private key with no reference to it",
            "external-key-unreferenced",
            VENDOR_STRIPE_PRIVY,
            json!({
                "stripePrivyConfiguration": {
                    "appId": "app",
                    "authorizationId": "authorization",
                    "appSecret": "c2VjcmV0",
                    "authorizationPrivateKeySource": SOURCE_EXTERNAL,
                }
            }),
            "authorizationPrivateKeyConfig",
        ),
        (
            "a managed wallet secret with no value to manage",
            "managed-wallet-valueless",
            VENDOR_COINBASE_CDP,
            json!({
                "coinbaseCdpConfiguration": {
                    "apiKeyId": "api-key",
                    "apiKeySecret": "c2VjcmV0",
                    "walletSecretSource": SOURCE_MANAGED,
                }
            }),
            "walletSecret",
        ),
        (
            "an app secret left to its default empty value",
            "managed-app-secret-empty",
            VENDOR_STRIPE_PRIVY,
            json!({
                "stripePrivyConfiguration": {
                    "appId": "app",
                    "authorizationId": "authorization",
                    "appSecret": "",
                    "authorizationPrivateKey": "a2V5",
                }
            }),
            "appSecret",
        ),
        (
            "an external secret referenced by an ARN of another service's resource",
            "external-wrong-arn",
            VENDOR_COINBASE_CDP,
            json!({
                "coinbaseCdpConfiguration": {
                    "apiKeyId": "api-key",
                    "apiKeySecret": "c2VjcmV0",
                    "walletSecretSource": SOURCE_EXTERNAL,
                    "walletSecretConfig": {
                        "secretId": "arn:aws:s3:::000000000000-wallets",
                        "jsonKey": "wallet",
                    },
                }
            }),
            "walletSecretConfig.secretId",
        ),
        (
            "an external secret referenced by a name no secret can carry",
            "external-wrong-name",
            VENDOR_STRIPE_PRIVY,
            json!({
                "stripePrivyConfiguration": {
                    "appId": "app",
                    "authorizationId": "authorization",
                    "appSecretSource": SOURCE_EXTERNAL,
                    "appSecretConfig": {"secretId": "not a secret name", "jsonKey": "secret"},
                    "authorizationPrivateKey": "a2V5",
                }
            }),
            "appSecretConfig.secretId",
        ),
        (
            "an external secret referenced by the ARN of another account's secret",
            "external-other-account",
            VENDOR_COINBASE_CDP,
            json!({
                "coinbaseCdpConfiguration": {
                    "apiKeyId": "api-key",
                    "apiKeySecret": "c2VjcmV0",
                    "walletSecretSource": SOURCE_EXTERNAL,
                    "walletSecretConfig": {
                        "secretId": format!(
                            "arn:aws:secretsmanager:{REGION}:111122223333:secret:other-account"
                        ),
                        "jsonKey": "wallet",
                    },
                }
            }),
            "walletSecretConfig.secretId",
        ),
        (
            "an external secret referenced by the ARN of another Region's secret",
            "external-other-region",
            VENDOR_STRIPE_PRIVY,
            json!({
                "stripePrivyConfiguration": {
                    "appId": "app",
                    "authorizationId": "authorization",
                    "appSecretSource": SOURCE_EXTERNAL,
                    "appSecretConfig": {
                        "secretId":
                            "arn:aws:secretsmanager:eu-west-1:000000000000:secret:other-region",
                        "jsonKey": "secret",
                    },
                    "authorizationPrivateKey": "a2V5",
                }
            }),
            "appSecretConfig.secretId",
        ),
    ];

    for (case, name, vendor, configuration, member) in rows {
        let (status, answer) = emulator
            .create(&json!({
                "name": name,
                "credentialProviderVendor": vendor,
                "providerConfigurationInput": configuration,
            }))
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} ({answer})"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(&format!("{member}: ")),
            "case {case:?}: the refusal names the member {member:?}, got {answer}"
        );
        assert_eq!(
            answer["fieldList"],
            json!([{"name": member, "message": message.trim_start_matches(&format!("{member}: "))}]),
            "case {case:?}: the refusal reports the member it refused in fieldList, got {answer}"
        );

        let (status, fetched) = emulator.get(name).await;
        assert_eq!(
            (status, error_code(&fetched).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the refused create persisted nothing, got {status} ({fetched})"
        );
    }
}

/// A name no provider of this account holds is the modelled 404, in the sentence AWS words
/// this family's missing resource with.
///
/// The rows differ along the shape of the name, including both bounds of
/// `CredentialProviderName`'s `len 1..128` — which are inside the constraint and so reach
/// the handler rather than the boundary validator.
#[tokio::test]
async fn a_name_naming_no_provider_is_refused() {
    let emulator = Emulator::new();

    let (status, created) = emulator
        .create(&json!({
            "name": "the-one-provider",
            "credentialProviderVendor": VENDOR_COINBASE_CDP,
            "providerConfigurationInput": coinbase_managed("c2VjcmV0", "d2FsbGV0"),
        }))
        .await;
    assert_eq!(
        status, 201,
        "the one provider this account holds is created, got {status} ({created})"
    );

    // `CredentialProviderName`'s length bounds, `len 1..128`
    // (`smithy_query shape=CredentialProviderName`).
    let shortest = "a".to_string();
    let longest = "n".repeat(128);

    // (case, the name read)
    let rows: [(&str, String); 5] = [
        ("a name at the published minimum length", shortest),
        ("a name at the published maximum length", longest),
        (
            "a name that only looks like the stored one",
            "the-one-provider-2".to_string(),
        ),
        (
            "the stored name in another case",
            "The-One-Provider".to_string(),
        ),
        (
            "a name carrying the admitted separators",
            "no_such-provider".to_string(),
        ),
    ];

    for (case, name) in rows {
        let (status, answer) = emulator.get(&name).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?} ({name:?}): expected HTTP 404 ResourceNotFoundException, got {status} \
             ({answer})"
        );
        assert_eq!(
            answer["message"].as_str().unwrap_or_default(),
            "PaymentCredentialProvider not found",
            "case {case:?}: unexpected refusal message, got {answer}"
        );
    }

    let (status, fetched) = emulator.get("the-one-provider").await;
    assert_eq!(
        (status, fetched["credentialProviderArn"].clone()),
        (200, json!(provider_arn("the-one-provider"))),
        "the refused reads left the stored provider readable, got {fetched}"
    );
}
