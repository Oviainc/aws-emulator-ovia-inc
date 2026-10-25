// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ARNs a request hands MediaPackageV2 for ANOTHER service's
//! resource, driven over HTTP through the same router the emulator boots and reusing the
//! restJson1 client in `channel_lifecycle`.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one request: a member
//! the model documents as the ARN of another AWS resource is accepted only when it is a
//! well-formed ARN of that service and resource type — and, for the secrets MediaPackage
//! itself has to read, one naming the Region the request is served in and the account its
//! resources belong to — a list of such ARNs may not name the same resource twice, and a
//! value outside that is refused with `ValidationException` (400) carrying the
//! `ValidationExceptionType` for which of those it failed, with nothing stored.
//!
//! The members this covers, each a plain string whose model states a `@length` and no
//! `@pattern` (`smithy_query(query="shape",shape="CdnAuthConfiguration")`,
//! `shape="SpekeKeyProvider"`), so the request boundary admits any text of the right size:
//!
//! 1. `CdnAuthConfiguration$CdnIdentifierSecretArns` — a Secrets Manager secret, in this
//!    Region and account, named once per list (`MALFORMED_SECRET_ARN`,
//!    `SECRET_FROM_DIFFERENT_REGION`, `SECRET_FROM_DIFFERENT_ACCOUNT`,
//!    `DUPLICATED_SECRET`).
//! 2. `CdnAuthConfiguration$SecretsRoleArn` — an IAM role (`ROLE_ARN_INVALID_FORMAT`).
//! 3. `SpekeKeyProvider$RoleArn`, on the create AND the update of an origin endpoint — the
//!    same role rule, since it is the role the service assumes to reach the key provider.
//!
//! How MANY secrets a list may carry is the model's own `@length` on
//! `CdnIdentifierSecretArns`, which `origin_endpoint_policy` covers; `SpekeKeyProvider`'s
//! `CertificateArn` states its own `@pattern`, so the boundary reads that one.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};

/// The Region the shared client signs every request for, and therefore the Region a secret
/// this service reads has to live in.
const REGION: &str = "us-west-2";

/// The account every resource this emulator serves belongs to
/// (`engine::EMULATOR_ACCOUNT_ID`), and therefore the account such a secret has to live in.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The parents this suite's endpoints hang under.
const GROUP: &str = "arn-group";
const CHANNEL: &str = "arn-channel";

/// The `Message` of an error envelope.
fn error_message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The `ValidationExceptionType` discriminator of an error envelope.
fn exception_type(body: &Value) -> &str {
    body.get("ValidationExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// A secret ARN as Secrets Manager writes one, in this Region and account.
fn secret(name: &str) -> String {
    format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:{name}")
}

/// A role ARN as IAM writes one.
fn role(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// An emulator with the group and channel this suite's endpoints live in.
async fn emulator_with_parents() -> Emulator {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(status, 200, "CreateChannelGroup expected 200, got {body}");
    let (status, body) = emulator
        .create_channel(GROUP, CHANNEL, json!({"InputType": "HLS"}))
        .await;
    assert_eq!(status, 200, "CreateChannel expected 200, got {body}");
    emulator
}

/// One row of the CDN authorization table: the block a put submits, and what the service
/// must answer for it.
struct CdnCase {
    label: &'static str,
    /// `CdnIdentifierSecretArns`, built per row so a row can vary the count as well as the
    /// values.
    secrets: Vec<String>,
    secrets_role_arn: String,
    /// `None` for a block the service can act on; otherwise the `ValidationExceptionType`
    /// the refusal must carry and the member path it must name.
    refused: Option<(&'static str, &'static str)>,
}

/// The wire paths of the two members, as the refusals name them.
const SECRETS_MEMBER: &str = "cdnAuthConfiguration.cdnIdentifierSecretArns";
const ROLE_MEMBER: &str = "cdnAuthConfiguration.secretsRoleArn";

/// The rows. They differ along what the rule generalises over: the ARN's form (not an ARN,
/// too few fields, another vendor, another resource type, another case), the scope it names
/// (Region, account), the list's count and the POSITION of the offending entry in it,
/// whether the list repeats a value, and which of the two members carries the bad value.
///
/// Every malformed value is at least twenty characters long, because
/// `CdnIdentifierSecretArn` is `@length 20..2048` and `SecretsRoleArn` `@length 20..2048`:
/// a shorter one never reaches a handler, the request boundary having already refused it.
fn cdn_cases() -> Vec<CdnCase> {
    vec![
        CdnCase {
            label: "one secret and a role, both as their services write them",
            secrets: vec![secret("cdn-one-AbCdEf")],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: None,
        },
        CdnCase {
            label: "three secrets, and a role carrying a path",
            secrets: vec![
                secret("cdn-one-AbCdEf"),
                secret("cdn-two-GhIjKl"),
                secret("cdn-three-MnOpQr"),
            ],
            secrets_role_arn: role("service-role/MediaPackageCdnAuth"),
            refused: None,
        },
        CdnCase {
            // AWS writes a resource field in either form, `<type>/<id>` and `<type>:<id>`,
            // so both name a secret (`service_common::arn`).
            label: "a secret whose resource field uses the slash form",
            secrets: vec![format!(
                "arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret/cdn-slash-form"
            )],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: None,
        },
        CdnCase {
            label: "a secret that is not an ARN at all",
            secrets: vec!["cdn-auth-secret-but-not-an-arn".to_string()],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("MALFORMED_SECRET_ARN", SECRETS_MEMBER)),
        },
        CdnCase {
            label: "a secret ARN missing its account field",
            secrets: vec![format!(
                "arn:aws:secretsmanager:{REGION}:secret:cdn-missing-field"
            )],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("MALFORMED_SECRET_ARN", SECRETS_MEMBER)),
        },
        CdnCase {
            label: "another vendor's ARN in the secret list",
            secrets: vec!["arn:aws:s3:::cdn-auth-bucket/secret-object".to_string()],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("MALFORMED_SECRET_ARN", SECRETS_MEMBER)),
        },
        CdnCase {
            label: "another resource type of Secrets Manager",
            secrets: vec![format!(
                "arn:aws:secretsmanager:{REGION}:{ACCOUNT}:rotation/cdn-one"
            )],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("MALFORMED_SECRET_ARN", SECRETS_MEMBER)),
        },
        CdnCase {
            // An ARN namespace is a fixed lowercase token, so a value differing in case
            // names another vendor.
            label: "the vendor field in another case",
            secrets: vec![format!(
                "arn:aws:SecretsManager:{REGION}:{ACCOUNT}:secret:cdn-one"
            )],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("MALFORMED_SECRET_ARN", SECRETS_MEMBER)),
        },
        CdnCase {
            label: "a well-formed secret ARN, then a malformed one",
            secrets: vec![
                secret("cdn-one-AbCdEf"),
                "second-entry-is-not-an-arn-at-all".to_string(),
            ],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("MALFORMED_SECRET_ARN", SECRETS_MEMBER)),
        },
        CdnCase {
            label: "a secret in another Region",
            secrets: vec![format!(
                "arn:aws:secretsmanager:eu-central-1:{ACCOUNT}:secret:cdn-one"
            )],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("SECRET_FROM_DIFFERENT_REGION", SECRETS_MEMBER)),
        },
        CdnCase {
            label: "a secret ARN naming no Region at all",
            secrets: vec![format!(
                "arn:aws:secretsmanager::{ACCOUNT}:secret:cdn-one-no-region"
            )],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("SECRET_FROM_DIFFERENT_REGION", SECRETS_MEMBER)),
        },
        CdnCase {
            label: "a secret in another account",
            secrets: vec![format!(
                "arn:aws:secretsmanager:{REGION}:111122223333:secret:cdn-one"
            )],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("SECRET_FROM_DIFFERENT_ACCOUNT", SECRETS_MEMBER)),
        },
        CdnCase {
            label: "the same secret twice, side by side",
            secrets: vec![secret("cdn-one-AbCdEf"), secret("cdn-one-AbCdEf")],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("DUPLICATED_SECRET", SECRETS_MEMBER)),
        },
        CdnCase {
            label: "the same secret twice, with another between them",
            secrets: vec![
                secret("cdn-one-AbCdEf"),
                secret("cdn-two-GhIjKl"),
                secret("cdn-one-AbCdEf"),
            ],
            secrets_role_arn: role("MediaPackageCdnAuth"),
            refused: Some(("DUPLICATED_SECRET", SECRETS_MEMBER)),
        },
        CdnCase {
            label: "a role that is not an ARN at all",
            secrets: vec![secret("cdn-one-AbCdEf")],
            secrets_role_arn: "MediaPackageCdnAuthorizationRole".to_string(),
            refused: Some(("ROLE_ARN_INVALID_FORMAT", ROLE_MEMBER)),
        },
        CdnCase {
            label: "an IAM user, which is no role to assume",
            secrets: vec![secret("cdn-one-AbCdEf")],
            secrets_role_arn: format!("arn:aws:iam::{ACCOUNT}:user/not-a-role-but-long-enough"),
            refused: Some(("ROLE_ARN_INVALID_FORMAT", ROLE_MEMBER)),
        },
        CdnCase {
            label: "a secret ARN where the role belongs",
            secrets: vec![secret("cdn-one-AbCdEf")],
            secrets_role_arn: secret("cdn-one-AbCdEf"),
            refused: Some(("ROLE_ARN_INVALID_FORMAT", ROLE_MEMBER)),
        },
    ]
}

impl CdnCase {
    /// The block as the request carries it.
    fn block(&self) -> Value {
        json!({
            "CdnIdentifierSecretArns": self.secrets,
            "SecretsRoleArn": self.secrets_role_arn,
        })
    }
}

/// The policy sub-resource's path.
fn policy_path(endpoint: &str) -> String {
    format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{endpoint}/policy")
}

/// RULE — `PutOriginEndpointPolicy` reads every ARN of the CDN authorization block before
/// it stores anything: a block the service could never read is refused with the
/// `ValidationExceptionType` for what is wrong with it, and the endpoint keeps the CDN
/// authorization it already had.
#[tokio::test]
async fn a_put_policy_reads_the_cdn_authorization_arns_before_storing_them() {
    let emulator = emulator_with_parents().await;
    let endpoint = "cdn-ep";
    let (status, body) = emulator
        .create_endpoint(
            GROUP,
            CHANNEL,
            endpoint,
            json!({"ContainerType": "TS", "HlsManifests": [{"ManifestName": "index"}]}),
        )
        .await;
    assert_eq!(status, 200, "CreateOriginEndpoint expected 200, got {body}");

    let policy = r#"{"Statement":[]}"#;
    // The baseline every refusal is measured against: a block the service accepts, stored
    // before the table runs and re-asserted after each row.
    let baseline = json!({
        "CdnIdentifierSecretArns": [secret("baseline-AbCdEf")],
        "SecretsRoleArn": role("MediaPackageCdnAuthBaseline"),
    });
    let path = policy_path(endpoint);
    let put = |block: Value| {
        emulator.call(
            "POST",
            &path,
            Some(json!({"Policy": policy, "CdnAuthConfiguration": block})),
        )
    };
    let (status, body) = put(baseline.clone()).await;
    assert_eq!(status, 200, "the baseline block expected 200, got {status} {body}");

    for case in cdn_cases() {
        let label = case.label;
        let (status, body) = put(case.block()).await;
        let (read_status, read) = emulator.call("GET", &policy_path(endpoint), None).await;
        assert_eq!(
            read_status, 200,
            "case {label:?}: the policy must still be readable, got {read_status} {read}"
        );

        match case.refused {
            None => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: a block the service can act on expected 200, got \
                     {status} {body}"
                );
                assert_eq!(
                    read["CdnAuthConfiguration"],
                    case.block(),
                    "case {label:?}: the stored block must be the submitted one, got {read}"
                );
                // Put the baseline back, so the next row's refusal is measured against it.
                let (status, body) = put(baseline.clone()).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: restoring the baseline expected 200, got {status} {body}"
                );
            }
            Some((expected_type, member)) => {
                assert_eq!(
                    status, 400,
                    "case {label:?}: expected 400, got {status} {body}"
                );
                assert_eq!(
                    error_type(&body),
                    "ValidationException",
                    "case {label:?}: expected ValidationException, got {body}"
                );
                assert_eq!(
                    exception_type(&body),
                    expected_type,
                    "case {label:?}: expected ValidationExceptionType {expected_type}, got \
                     {body}"
                );
                assert!(
                    error_message(&body).contains(member),
                    "case {label:?}: the refusal must name {member}, got {body}"
                );
                assert_eq!(
                    read["CdnAuthConfiguration"], baseline,
                    "case {label:?}: a refused put must leave the stored block as it was, \
                     got {read}"
                );
            }
        }
    }
}

/// A segment whose SPEKE key provider names `role_arn`, with everything else it requires
/// (`EncryptionMethod`, `EncryptionContractConfiguration`, `ResourceId`, `DrmSystems`,
/// `Url` — all required members of `Encryption` / `SpekeKeyProvider`). The enum values come
/// from the model's `TsEncryptionMethod`, `PresetSpeke20Audio`, `PresetSpeke20Video` and
/// `DrmSystem` shapes.
fn segment_with_key_provider_role(role_arn: &str) -> Value {
    json!({
        "Encryption": {
            "EncryptionMethod": {"TsEncryptionMethod": "AES_128"},
            "SpekeKeyProvider": {
                "EncryptionContractConfiguration": {
                    "PresetSpeke20Audio": "PRESET_AUDIO_1",
                    "PresetSpeke20Video": "PRESET_VIDEO_1",
                },
                "ResourceId": "MovieNight20171126093045",
                "DrmSystems": ["WIDEVINE"],
                "RoleArn": role_arn,
                "Url": "https://speke.example.com/copyProtection",
            },
        },
    })
}

/// The role values the key-provider member is judged on, as `(label, value, whether it is a
/// role ARN)`. The same forms the CDN authorization role is judged on, since it is the same
/// rule: one role ARN, one carrying a path, and values that name no role.
fn key_provider_roles() -> Vec<(&'static str, String, bool)> {
    vec![
        ("a role ARN, as IAM writes one", role("SpekeAccess"), true),
        (
            "a role ARN carrying a path",
            role("service-role/SpekeAccess"),
            true,
        ),
        (
            "not an ARN at all",
            "SpekeAccessRoleButNotAnArn".to_string(),
            false,
        ),
        (
            "an IAM user, which is no role to assume",
            format!("arn:aws:iam::{ACCOUNT}:user/not-a-role-but-long-enough"),
            false,
        ),
        (
            "another vendor's ARN",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcd-1234"),
            false,
        ),
    ]
}

/// RULE, one level out — the same reading covers the OTHER role ARN the model documents:
/// `CreateOriginEndpoint` refuses a key provider that names no role and mints nothing, and
/// `UpdateOriginEndpoint` refuses it and leaves the stored endpoint as it was.
#[tokio::test]
async fn an_endpoints_key_provider_must_name_an_iam_role() {
    let emulator = emulator_with_parents().await;
    let manifests = json!([{"ManifestName": "index"}]);

    for (index, (label, role_arn, is_a_role)) in key_provider_roles().into_iter().enumerate() {
        let endpoint = format!("speke-ep-{index}");
        let (status, body) = emulator
            .create_endpoint(
                GROUP,
                CHANNEL,
                &endpoint,
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": manifests,
                    "Segment": segment_with_key_provider_role(&role_arn),
                }),
            )
            .await;

        if is_a_role {
            assert_eq!(
                status, 200,
                "case {label:?}: a create naming a role expected 200, got {status} {body}"
            );
            let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, &endpoint).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the created endpoint must be readable, got {status} {read}"
            );
            assert_eq!(
                read["Segment"]["Encryption"]["SpekeKeyProvider"]["RoleArn"],
                json!(role_arn),
                "case {label:?}: the stored key provider must report the submitted role, \
                 got {read}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {label:?}: a create naming no role expected 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            exception_type(&body),
            "ROLE_ARN_INVALID_FORMAT",
            "case {label:?}: expected ValidationExceptionType ROLE_ARN_INVALID_FORMAT, got \
             {body}"
        );
        assert!(
            error_message(&body).contains("segment.encryption.spekeKeyProvider.roleArn"),
            "case {label:?}: the refusal must name the member it is about, got {body}"
        );
        let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, &endpoint).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a refused create must mint nothing, got {status} {read}"
        );
    }

    // The update is held to the same rule, and a refused one leaves the endpoint — key
    // provider included — exactly as the create stored it.
    let endpoint = "speke-update-ep";
    let stored_role = role("SpekeAccess");
    let (status, created) = emulator
        .create_endpoint(
            GROUP,
            CHANNEL,
            endpoint,
            json!({
                "ContainerType": "TS",
                "HlsManifests": manifests,
                "Segment": segment_with_key_provider_role(&stored_role),
            }),
        )
        .await;
    assert_eq!(status, 200, "CreateOriginEndpoint expected 200, got {created}");

    for (label, role_arn, is_a_role) in key_provider_roles() {
        if is_a_role {
            continue;
        }
        let (status, body) = emulator
            .update_endpoint(
                GROUP,
                CHANNEL,
                endpoint,
                json!({
                    "ContainerType": "TS",
                    "HlsManifests": manifests,
                    "Segment": segment_with_key_provider_role(&role_arn),
                }),
                &[],
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: an update naming no role expected 400, got {status} {body}"
        );
        assert_eq!(
            exception_type(&body),
            "ROLE_ARN_INVALID_FORMAT",
            "case {label:?}: expected ValidationExceptionType ROLE_ARN_INVALID_FORMAT, got \
             {body}"
        );
        let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, endpoint).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the endpoint must still be readable, got {status} {read}"
        );
        assert_eq!(
            read["Segment"]["Encryption"]["SpekeKeyProvider"]["RoleArn"],
            json!(stored_role),
            "case {label:?}: a refused update must leave the stored role as it was, got \
             {read}"
        );
        assert_eq!(
            read["ETag"], created["ETag"],
            "case {label:?}: a refused update must not rotate the entity tag, got {read}"
        );
    }
}
