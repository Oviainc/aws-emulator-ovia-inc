// SPDX-License-Identifier: CC0-1.0

//! The encryption posture of a repository creation template, end to end over the wire.
//!
//! The rule under test: a repository creation template always carries a resolved
//! encryption configuration — the one the create named, or the service default
//! encryption type when it named none — and every operation that reports a template
//! (create, describe, update, delete) reports that stored configuration, which an update
//! replaces only when it names one.

use serde_json::{json, Value};

use crate::wire::{ok, REGISTRY_ID};

/// The encryption types the model enumerates, from
/// `smithy_query(query="shape", shape="EncryptionType")` — the target of
/// `EncryptionConfigurationForRepositoryCreationTemplate$encryptionType`, which the model
/// marks required. The whole catalog, so a value the implementation drops is visible.
const ENCRYPTION_TYPES: [&str; 3] = ["AES256", "KMS", "KMS_DSSE"];

/// The encryption type ECR resolves to when a request names no configuration: server-side
/// encryption with Amazon S3-managed keys, the same posture `CreateRepository` defaults a
/// repository to.
///
/// Spelled out here on purpose rather than imported from the crate: these cases assert
/// the expected value independently of the implementation's own constant, so collapsing
/// this onto `crate::state::DEFAULT_ENCRYPTION_TYPE` would make the assertion
/// unfalsifiable.
const DEFAULT_ENCRYPTION_TYPE: &str = "AES256";

/// A KMS key ARN matching `KmsKeyForRepositoryCreationTemplate`'s pattern
/// (`^$|arn:aws[a-z0-9-]*:kms:[a-z0-9-]+:[0-9]{12}:key\/[a-z0-9-]+$`).
const KMS_KEY_ARN: &str = "arn:aws:kms:eu-west-1:000000000000:key/abc-123";

/// One row of the resolution table: the case name, the `encryptionConfiguration` the
/// create carries (`None` when it names none), and the configuration every operation that
/// reports the template must then report.
type ResolutionCase = (&'static str, Option<Value>, Value);

/// The rows differ along what the rule generalises over: whether the request names a
/// configuration at all, which enumerated encryption type it names, and whether a key
/// accompanies it.
fn resolution_cases() -> Vec<ResolutionCase> {
    let mut cases: Vec<ResolutionCase> = vec![(
        "no encryptionConfiguration at all, which resolves to the default",
        None,
        json!({"encryptionType": DEFAULT_ENCRYPTION_TYPE}),
    )];
    // One row per encryption type the model enumerates, named without a key — the
    // keyless form every type admits.
    for encryption_type in ENCRYPTION_TYPES {
        cases.push((
            encryption_type,
            Some(json!({"encryptionType": encryption_type})),
            json!({"encryptionType": encryption_type}),
        ));
    }
    // The KMS types are the ones a caller's key belongs to; `AES256` encrypts with an
    // Amazon S3-managed key and is refused a key (asserted in the create suite).
    for encryption_type in ["KMS", "KMS_DSSE"] {
        cases.push((
            encryption_type,
            Some(json!({"encryptionType": encryption_type, "kmsKey": KMS_KEY_ARN})),
            json!({"encryptionType": encryption_type, "kmsKey": KMS_KEY_ARN}),
        ));
    }
    cases
}

/// The `encryptionConfiguration` of the template `body` reports under `member`, or
/// `null` when the member is absent — so an assertion names the member that is missing
/// rather than panicking on the lookup.
fn reported(body: &Value, member: &str) -> Value {
    body.pointer(&format!("/{member}/encryptionConfiguration"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The one template the registry holds for `prefix`, as
/// `DescribeRepositoryCreationTemplates` reports it.
async fn described(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    prefix: &str,
) -> Value {
    let body = ok(
        router,
        case,
        "DescribeRepositoryCreationTemplates",
        &json!({"prefixes": [prefix]}),
    )
    .await;
    let listed = body["repositoryCreationTemplates"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {body:?}"))
        .clone();
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: exactly one template must answer to {prefix:?}, got {listed:?}"
    );
    listed[0].clone()
}

/// The create resolves the encryption configuration, and all four operations that report
/// a template report exactly that resolved configuration: the create, the describe, an
/// update that names no configuration (so the posture is preserved) and the delete that
/// reports the template it removed.
#[tokio::test]
async fn every_template_reporting_operation_reports_the_resolved_encryption() {
    for (case, requested, expected) in resolution_cases() {
        let router = crate::wire::emulator();
        let prefix = "team/app";
        let mut request = json!({"prefix": prefix, "appliedFor": ["PULL_THROUGH_CACHE"]});
        if let Some(requested) = requested.as_ref() {
            request["encryptionConfiguration"] = requested.clone();
        }

        let created = ok(&router, case, "CreateRepositoryCreationTemplate", &request).await;
        assert_eq!(
            created["registryId"].as_str(),
            Some(REGISTRY_ID),
            "case {case:?}: the create must report this registry, got {:?}",
            created["registryId"]
        );
        assert_eq!(
            reported(&created, "repositoryCreationTemplate"),
            expected,
            "case {case:?}: expected the create to report {expected:?}, got {:?}",
            reported(&created, "repositoryCreationTemplate")
        );

        // The describe reads the stored record, so it reports the same configuration.
        let listed = described(&router, case, prefix).await;
        assert_eq!(
            listed["encryptionConfiguration"], expected,
            "case {case:?}: expected the describe to report {expected:?}, got {:?}",
            listed["encryptionConfiguration"]
        );

        // An update that names no encryptionConfiguration keeps the stored one, and says
        // so in its answer.
        let updated = ok(
            &router,
            case,
            "UpdateRepositoryCreationTemplate",
            &json!({"prefix": prefix, "imageTagMutability": "IMMUTABLE"}),
        )
        .await;
        assert_eq!(
            reported(&updated, "repositoryCreationTemplate"),
            expected,
            "case {case:?}: expected the update to preserve and report {expected:?}, got {:?}",
            reported(&updated, "repositoryCreationTemplate")
        );
        let after_update = described(&router, case, prefix).await;
        assert_eq!(
            after_update["encryptionConfiguration"], expected,
            "case {case:?}: expected the stored template to keep {expected:?}, got {:?}",
            after_update["encryptionConfiguration"]
        );

        // The delete answers with the template it removed, encryption posture included.
        let deleted = ok(
            &router,
            case,
            "DeleteRepositoryCreationTemplate",
            &json!({"prefix": prefix}),
        )
        .await;
        assert_eq!(
            reported(&deleted, "repositoryCreationTemplate"),
            expected,
            "case {case:?}: expected the delete to report {expected:?}, got {:?}",
            reported(&deleted, "repositoryCreationTemplate")
        );
    }
}

/// An update that names an encryption configuration replaces the stored one outright, and
/// the registry reports the new posture from then on; an update whose configuration pairs
/// a caller's key with the S3-managed type is refused and changes nothing. The rows are
/// every ordered pair of encryption types the model enumerates, including a move to the
/// type already stored.
#[tokio::test]
async fn an_update_replaces_the_stored_encryption_configuration() {
    for from in ENCRYPTION_TYPES {
        for to in ENCRYPTION_TYPES {
            let case = format!("{from} -> {to}");
            let router = crate::wire::emulator();
            let prefix = "team/app";
            ok(
                &router,
                &case,
                "CreateRepositoryCreationTemplate",
                &json!({
                    "prefix": prefix,
                    "appliedFor": ["REPLICATION"],
                    "encryptionConfiguration": {"encryptionType": from},
                }),
            )
            .await;

            let expected = json!({"encryptionType": to});
            let updated = ok(
                &router,
                &case,
                "UpdateRepositoryCreationTemplate",
                &json!({
                    "prefix": prefix,
                    "encryptionConfiguration": {"encryptionType": to},
                }),
            )
            .await;
            assert_eq!(
                reported(&updated, "repositoryCreationTemplate"),
                expected,
                "case {case:?}: expected the update to report {expected:?}, got {:?}",
                reported(&updated, "repositoryCreationTemplate")
            );

            let listed = described(&router, &case, prefix).await;
            assert_eq!(
                listed["encryptionConfiguration"], expected,
                "case {case:?}: expected the stored template to carry {expected:?}, got {:?}",
                listed["encryptionConfiguration"]
            );

            // The replace path checks the same pairing the create does: a caller's key
            // belongs to the KMS types, not to the S3-managed `AES256`, so an update to
            // `AES256` carrying one is refused and the stored posture is left alone.
            let refusal_case = format!("{case} with a kmsKey beside AES256");
            crate::wire::refused(
                &router,
                &refusal_case,
                "UpdateRepositoryCreationTemplate",
                &json!({
                    "prefix": prefix,
                    "encryptionConfiguration": {
                        "encryptionType": "AES256",
                        "kmsKey": KMS_KEY_ARN,
                    },
                }),
                "InvalidParameterException",
            )
            .await;
            let after_refusal = described(&router, &refusal_case, prefix).await;
            assert_eq!(
                after_refusal["encryptionConfiguration"], expected,
                "case {refusal_case:?}: a refused update must leave {expected:?} stored, got {:?}",
                after_refusal["encryptionConfiguration"]
            );
        }
    }
}

/// A template created with no encryption configuration is stored with the resolved
/// default, so a repository-creation-template listing of the whole registry reports the
/// member for every template whatever the create named. The rows differ in how many
/// templates the registry holds and in which of them named a configuration.
#[tokio::test]
async fn a_listing_reports_the_member_for_every_template() {
    let router = crate::wire::emulator();
    // prefix -> the configuration the create named (None for the defaulting rows).
    let seeds: [(&str, Option<Value>); 4] = [
        ("alpha", None),
        ("beta", Some(json!({"encryptionType": "KMS"}))),
        ("gamma", None),
        (
            "delta",
            Some(json!({"encryptionType": "KMS_DSSE", "kmsKey": KMS_KEY_ARN})),
        ),
    ];

    for (prefix, requested) in &seeds {
        let mut request = json!({"prefix": prefix, "appliedFor": ["CREATE_ON_PUSH"]});
        if let Some(requested) = requested.as_ref() {
            request["encryptionConfiguration"] = requested.clone();
        }
        ok(
            &router,
            prefix,
            "CreateRepositoryCreationTemplate",
            &request,
        )
        .await;
    }

    let all = ok(
        &router,
        "every template",
        "DescribeRepositoryCreationTemplates",
        &json!({}),
    )
    .await;
    let listed = all["repositoryCreationTemplates"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of templates, got {all:?}"))
        .clone();
    assert_eq!(
        listed.len(),
        seeds.len(),
        "every created template must be listed once, got {listed:?}"
    );
    for (prefix, requested) in &seeds {
        let expected = requested
            .clone()
            .unwrap_or_else(|| json!({"encryptionType": DEFAULT_ENCRYPTION_TYPE}));
        let template = listed
            .iter()
            .find(|template| template["prefix"].as_str() == Some(*prefix))
            .unwrap_or_else(|| panic!("case {prefix:?}: the listing must carry it: {listed:?}"));
        assert_eq!(
            template["encryptionConfiguration"], expected,
            "case {prefix:?}: expected the listing to report {expected:?}, got {:?}",
            template["encryptionConfiguration"]
        );
    }
}
