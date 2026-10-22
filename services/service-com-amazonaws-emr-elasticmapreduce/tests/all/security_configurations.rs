// SPDX-License-Identifier: CC0-1.0

//! EMR security configurations, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, so these are the same bytes an SDK client sends. Nothing
//! reaches into the service's state struct: a security configuration is only ever read
//! back through `DescribeSecurityConfiguration`, which is what makes these tests
//! evidence that the responses are computed from persisted state.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::{
    AUDIT_TARGET, MAX_SECURITY_CONFIGURATIONS, MAX_SECURITY_CONFIGURATION_BYTES,
};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

// The wire harness, the JSON walker and the refusal assertion are this test target's
// shared ones — this suite adds only the security-configuration request shapes.
use super::job_flow_lifecycle::{assert_refused, at, epoch_secs, Emr, REQUEST_TARGET};

/// `CreateSecurityConfiguration$Name` targets `XmlString`, whose documented length
/// bound is `len 0..10280`
/// (`smithy_query(query="operation", operation="CreateSecurityConfiguration")`).
const NAME_MAX_LEN: usize = 10280;

/// Every value `S3EncryptionConfiguration.EncryptionMode` may take, with the companions
/// that mode needs. (Amazon EMR Management Guide, "Encryption options" / the security
/// configuration JSON reference — the same catalog the handler enforces.)
const S3_ENCRYPTION_MODES: [&str; 4] = ["SSE-S3", "SSE-KMS", "CSE-KMS", "CSE-Custom"];

/// The access key id the audit case presents, which no other suite uses, so its records
/// can be told apart from another test's in a process-wide capture.
const AUDIT_AKID: &str = "AKIDEMRAUDITCASE";

/// The security-configuration calls, over the shared [`Emr`] harness rather than a copy
/// of it.
struct SecurityConfigurations {
    emr: Emr,
}

impl SecurityConfigurations {
    fn new() -> Self {
        Self { emr: Emr::new() }
    }

    /// The same, with every request presenting `akid` as its access key id.
    fn with_akid(akid: &'static str) -> Self {
        Self {
            emr: Emr::with_akid(akid),
        }
    }

    async fn create(&self, name: &str, document: &str) -> (u16, Value) {
        self.emr
            .call(
                "CreateSecurityConfiguration",
                json!({ "Name": name, "SecurityConfiguration": document }),
            )
            .await
    }

    async fn create_ok(&self, name: &str, document: &str) -> Value {
        self.emr
            .ok(
                "CreateSecurityConfiguration",
                json!({ "Name": name, "SecurityConfiguration": document }),
            )
            .await
    }

    async fn describe(&self, name: &str) -> (u16, Value) {
        self.emr
            .call("DescribeSecurityConfiguration", json!({ "Name": name }))
            .await
    }

    async fn describe_ok(&self, name: &str) -> Value {
        self.emr
            .ok("DescribeSecurityConfiguration", json!({ "Name": name }))
            .await
    }

    async fn delete(&self, name: &str) -> (u16, Value) {
        self.emr
            .call("DeleteSecurityConfiguration", json!({ "Name": name }))
            .await
    }
}

/// The `CreationDateTime` of a create or describe payload, as epoch seconds.
fn creation_secs(case: &str, payload: &Value) -> f64 {
    at(payload, &["CreationDateTime"])
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a CreationDateTime in {payload}"))
}

/// A legal `S3EncryptionConfiguration` for `mode`, carrying what that mode needs.
fn s3_encryption(mode: &str) -> Value {
    match mode {
        "SSE-S3" => json!({ "EncryptionMode": "SSE-S3" }),
        "SSE-KMS" | "CSE-KMS" => json!({
            "EncryptionMode": mode,
            "AwsKmsKey": "arn:aws:kms:us-east-1:123456789012:key/2a2b-3c3d",
        }),
        "CSE-Custom" => json!({
            "EncryptionMode": "CSE-Custom",
            "EncryptionKeyProviderClass": "com.example.MyKeyProvider",
            "S3Object": "s3://my-bucket/my-key-provider.jar",
        }),
        other => panic!("no legal S3EncryptionConfiguration written for mode {other:?}"),
    }
}

/// A legal document of exactly `bytes` bytes: an object carrying one padded comment.
fn padded_document(bytes: usize) -> String {
    let envelope = "{\"Comment\":\"\"}";
    let padding = bytes
        .checked_sub(envelope.len())
        .expect("a document at least as long as its envelope");
    format!("{{\"Comment\":\"{}\"}}", "p".repeat(padding))
}

/// THE RULE: `CreateSecurityConfiguration` stores the document under its name and
/// stamps the instant it was created; `DescribeSecurityConfiguration` answers that same
/// name, the document VERBATIM and that same instant, for every document shape and name
/// the model admits.
///
/// The table spans the document's sections (none, encryption at rest, encryption in
/// transit, both, authentication, instance metadata), each encryption mode and provider
/// type, the document's formatting (compact vs pretty-printed), and the name's form and
/// both published length bounds.
#[tokio::test]
async fn created_security_configurations_are_read_back_verbatim() {
    struct Case {
        label: &'static str,
        name: String,
        document: String,
    }

    let max_name = "n".repeat(NAME_MAX_LEN);
    let mut cases = vec![
        Case {
            label: "an empty document",
            name: "empty-document".to_string(),
            document: "{}".to_string(),
        },
        Case {
            label: "a pretty-printed document, stored byte for byte",
            name: "pretty printed".to_string(),
            document: "{\n\t\"EncryptionConfiguration\": {\n\t\t\"EnableAtRestEncryption\": false,\n\t\t\"EnableInTransitEncryption\": false\n\t}\n}"
                .to_string(),
        },
        Case {
            label: "encryption in transit with PEM certificates",
            name: "in-transit-pem".to_string(),
            document: json!({
                "EncryptionConfiguration": {
                    "EnableInTransitEncryption": true,
                    "InTransitEncryptionConfiguration": {
                        "TLSCertificateConfiguration": {
                            "CertificateProviderType": "PEM",
                            "S3Object": "s3://my-bucket/my-certs.zip",
                        }
                    },
                }
            })
            .to_string(),
        },
        Case {
            label: "encryption in transit with a custom certificate provider",
            name: "in-transit-custom".to_string(),
            document: json!({
                "EncryptionConfiguration": {
                    "EnableInTransitEncryption": true,
                    "InTransitEncryptionConfiguration": {
                        "TLSCertificateConfiguration": {
                            "CertificateProviderType": "Custom",
                            "CertificateProviderClass": "com.example.MyCertProvider",
                            "S3Object": "s3://my-bucket/my-cert-provider.jar",
                        }
                    },
                }
            })
            .to_string(),
        },
        Case {
            label: "local disk encryption with a KMS key, EBS volumes included",
            name: "at-rest-local-kms".to_string(),
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "AtRestEncryptionConfiguration": {
                        "LocalDiskEncryptionConfiguration": {
                            "EncryptionKeyProviderType": "AwsKms",
                            "AwsKmsKey": "arn:aws:kms:us-east-1:123456789012:key/1a1b-2c2d",
                            "EnableEbsEncryption": true,
                        }
                    },
                }
            })
            .to_string(),
        },
        Case {
            label: "local disk encryption with a custom key provider",
            name: "at-rest-local-custom".to_string(),
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "AtRestEncryptionConfiguration": {
                        "LocalDiskEncryptionConfiguration": {
                            "EncryptionKeyProviderType": "Custom",
                            "EncryptionKeyProviderClass": "com.example.MyKeyProvider",
                            "S3Object": "s3://my-bucket/my-key-provider.jar",
                        }
                    },
                }
            })
            .to_string(),
        },
        Case {
            label: "authentication only, with no encryption section at all",
            name: "kerberos only".to_string(),
            document: json!({
                "AuthenticationConfiguration": {
                    "KerberosConfiguration": {
                        "Provider": "ClusterDedicatedKdc",
                        "ClusterDedicatedKdcConfiguration": { "TicketLifetimeInHours": 24 },
                    }
                }
            })
            .to_string(),
        },
        Case {
            label: "the instance metadata service only",
            name: "imds-v2".to_string(),
            document: json!({
                "InstanceMetadataServiceConfiguration": {
                    "MinimumInstanceMetadataServiceVersion": 2,
                    "HttpPutResponseHopLimit": 1,
                }
            })
            .to_string(),
        },
        Case {
            label: "a name at the published minimum length of 0",
            name: String::new(),
            document: "{}".to_string(),
        },
        Case {
            label: "a name at the published maximum length of 10280",
            name: max_name,
            document: "{}".to_string(),
        },
    ];

    // Every S3 encryption mode the catalog above names, each with both encryption modes
    // switched on so the whole `EncryptionConfiguration` section is exercised.
    for mode in S3_ENCRYPTION_MODES {
        cases.push(Case {
            label: "at rest on S3, every documented encryption mode",
            name: format!("at-rest-s3-{mode}"),
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "EnableInTransitEncryption": true,
                    "AtRestEncryptionConfiguration": {
                        "S3EncryptionConfiguration": s3_encryption(mode),
                    },
                    "InTransitEncryptionConfiguration": {
                        "TLSCertificateConfiguration": {
                            "CertificateProviderType": "PEM",
                            "S3Object": "s3://my-bucket/my-certs.zip",
                        }
                    },
                }
            })
            .to_string(),
        });
    }

    let emr = SecurityConfigurations::new();
    for case in &cases {
        let case_label = format!("{} ({} bytes of name)", case.label, case.name.len());
        let before = epoch_secs(SystemTime::now());
        let created = emr.create_ok(&case.name, &case.document).await;
        let after = epoch_secs(SystemTime::now()) + 1.0;

        assert_eq!(
            at(&created, &["Name"]).as_str(),
            Some(case.name.as_str()),
            "case {case_label:?}: CreateSecurityConfiguration must answer the requested name, got {created}"
        );
        let created_at = creation_secs(&case_label, &created);
        assert!(
            created_at >= before && created_at <= after,
            "case {case_label:?}: expected a CreationDateTime inside [{before}, {after}], got {created_at}"
        );

        let described = emr.describe_ok(&case.name).await;
        assert_eq!(
            at(&described, &["Name"]).as_str(),
            Some(case.name.as_str()),
            "case {case_label:?}: DescribeSecurityConfiguration must answer the stored name, got {described}"
        );
        assert_eq!(
            at(&described, &["SecurityConfiguration"]).as_str(),
            Some(case.document.as_str()),
            "case {case_label:?}: expected the document back verbatim, got {described}"
        );
        assert_eq!(
            creation_secs(&case_label, &described),
            created_at,
            "case {case_label:?}: expected the creation instant {created_at} the create answered, got {described}"
        );
    }

    // Every configuration created above is still readable afterwards: a later create
    // never displaces an earlier one.
    for case in &cases {
        let described = emr.describe_ok(&case.name).await;
        assert_eq!(
            at(&described, &["SecurityConfiguration"]).as_str(),
            Some(case.document.as_str()),
            "case {:?}: expected the document to survive the later creates, got {described}",
            case.label
        );
    }
}

/// THE RULE: a name longer than the published bound is refused, not stored.
#[tokio::test]
async fn a_name_past_the_published_length_bound_is_refused() {
    let emr = SecurityConfigurations::new();
    let too_long = "n".repeat(NAME_MAX_LEN + 1);
    let (status, payload) = emr.create(&too_long, "{}").await;
    assert_eq!(
        status, 400,
        "case {:?}: expected HTTP 400, got {status} with {payload}",
        "a name one byte past the maximum"
    );

    let (status, payload) = emr.describe(&too_long).await;
    assert_eq!(
        status, 400,
        "case {:?}: a refused create must persist nothing, got {status} with {payload}",
        "a name one byte past the maximum"
    );
}

/// THE RULE: a security configuration whose `EncryptionConfiguration` does not hold
/// together — a mode switched on without the configuration it needs, or a provider or
/// encryption mode EMR does not define — is refused with `InvalidRequestException` and
/// nothing is stored; one that does hold together is stored.
///
/// The table spans both encryption modes, every section that can be missing, every
/// hand-written catalog's values and an entry outside each, and both the JSON-shape and
/// the JSON-syntax failures.
#[tokio::test]
async fn illegal_encryption_semantics_are_refused_and_legal_ones_accepted() {
    struct Case {
        label: &'static str,
        document: String,
        legal: bool,
    }

    let tls_pem = json!({
        "TLSCertificateConfiguration": {
            "CertificateProviderType": "PEM",
            "S3Object": "s3://my-bucket/my-certs.zip",
        }
    });

    let mut cases = vec![
        Case {
            label: "not JSON at all",
            document: "{\"EncryptionConfiguration\":".to_string(),
            legal: false,
        },
        Case {
            label: "valid JSON that is not an object",
            document: "\"EnableAtRestEncryption\"".to_string(),
            legal: false,
        },
        Case {
            label: "EncryptionConfiguration that is not an object",
            document: json!({ "EncryptionConfiguration": "all of it" }).to_string(),
            legal: false,
        },
        Case {
            label: "EnableAtRestEncryption as a string rather than a flag",
            document: json!({ "EncryptionConfiguration": { "EnableAtRestEncryption": "true" } })
                .to_string(),
            legal: false,
        },
        Case {
            label: "in transit enabled with no InTransitEncryptionConfiguration",
            document: json!({ "EncryptionConfiguration": { "EnableInTransitEncryption": true } })
                .to_string(),
            legal: false,
        },
        Case {
            label: "in transit enabled with no TLSCertificateConfiguration",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableInTransitEncryption": true,
                    "InTransitEncryptionConfiguration": {},
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            label: "a PEM certificate provider with no S3Object",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableInTransitEncryption": true,
                    "InTransitEncryptionConfiguration": {
                        "TLSCertificateConfiguration": { "CertificateProviderType": "PEM" }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            label: "a custom certificate provider with no CertificateProviderClass",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableInTransitEncryption": true,
                    "InTransitEncryptionConfiguration": {
                        "TLSCertificateConfiguration": {
                            "CertificateProviderType": "Custom",
                            "S3Object": "s3://my-bucket/my-cert-provider.jar",
                        }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            // "JKS" is outside the CertificateProviderType catalog (PEM, Custom).
            label: "a certificate provider type EMR does not define",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableInTransitEncryption": true,
                    "InTransitEncryptionConfiguration": {
                        "TLSCertificateConfiguration": {
                            "CertificateProviderType": "JKS",
                            "S3Object": "s3://my-bucket/my-certs.jks",
                        }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            label: "at rest enabled with no AtRestEncryptionConfiguration",
            document: json!({ "EncryptionConfiguration": { "EnableAtRestEncryption": true } })
                .to_string(),
            legal: false,
        },
        Case {
            label: "at rest enabled with an empty AtRestEncryptionConfiguration",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "AtRestEncryptionConfiguration": {},
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            label: "an S3 encryption configuration with no EncryptionMode",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "AtRestEncryptionConfiguration": {
                        "S3EncryptionConfiguration": {
                            "AwsKmsKey": "arn:aws:kms:us-east-1:123456789012:key/1a1b-2c2d",
                        }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            // "SSE-C" is outside the EncryptionMode catalog.
            label: "an S3 encryption mode EMR does not define",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "AtRestEncryptionConfiguration": {
                        "S3EncryptionConfiguration": { "EncryptionMode": "SSE-C" }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            label: "CSE-Custom with no EncryptionKeyProviderClass",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "AtRestEncryptionConfiguration": {
                        "S3EncryptionConfiguration": {
                            "EncryptionMode": "CSE-Custom",
                            "S3Object": "s3://my-bucket/my-key-provider.jar",
                        }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            label: "local disk encryption by KMS with no AwsKmsKey",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "AtRestEncryptionConfiguration": {
                        "LocalDiskEncryptionConfiguration": {
                            "EncryptionKeyProviderType": "AwsKms",
                        }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            label: "a custom local disk key provider with no EncryptionKeyProviderClass",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "AtRestEncryptionConfiguration": {
                        "LocalDiskEncryptionConfiguration": {
                            "EncryptionKeyProviderType": "Custom",
                            "S3Object": "s3://my-bucket/my-key-provider.jar",
                        }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            // "AwsKmsCustom" is outside the EncryptionKeyProviderType catalog
            // (AwsKms, Custom).
            label: "a local disk key provider type EMR does not define",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "AtRestEncryptionConfiguration": {
                        "LocalDiskEncryptionConfiguration": {
                            "EncryptionKeyProviderType": "AwsKmsCustom",
                            "AwsKmsKey": "arn:aws:kms:us-east-1:123456789012:key/1a1b-2c2d",
                        }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            label: "an inconsistent section the flags leave switched off",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": false,
                    "AtRestEncryptionConfiguration": {
                        "S3EncryptionConfiguration": { "EncryptionMode": "SSE-KMS" }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            label: "an inconsistent in-transit section the flags leave switched off",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableInTransitEncryption": false,
                    "InTransitEncryptionConfiguration": {
                        "TLSCertificateConfiguration": { "CertificateProviderType": "JKS" }
                    },
                }
            })
            .to_string(),
            legal: false,
        },
        Case {
            label: "both encryption modes switched off",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": false,
                    "EnableInTransitEncryption": false,
                }
            })
            .to_string(),
            legal: true,
        },
        Case {
            label: "no encryption section at all",
            document: json!({
                "InstanceMetadataServiceConfiguration": {
                    "MinimumInstanceMetadataServiceVersion": 2,
                }
            })
            .to_string(),
            legal: true,
        },
        Case {
            label: "in transit enabled, with the certificates it needs",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableInTransitEncryption": true,
                    "InTransitEncryptionConfiguration": tls_pem,
                }
            })
            .to_string(),
            legal: true,
        },
    ];

    // Each documented S3 encryption mode, carrying what that mode needs, is accepted.
    for mode in S3_ENCRYPTION_MODES {
        cases.push(Case {
            label: "at rest enabled, with the key material that mode needs",
            document: json!({
                "EncryptionConfiguration": {
                    "EnableAtRestEncryption": true,
                    "AtRestEncryptionConfiguration": {
                        "S3EncryptionConfiguration": s3_encryption(mode),
                    },
                }
            })
            .to_string(),
            legal: true,
        });
    }

    let emr = SecurityConfigurations::new();
    for (index, case) in cases.iter().enumerate() {
        let name = format!("case-{index}");
        let case_label = format!("{} [{}]", case.label, case.document);
        let (status, payload) = emr.create(&name, &case.document).await;

        if case.legal {
            assert_eq!(
                status, 200,
                "case {case_label:?}: expected HTTP 200, got {status} with {payload}"
            );
            let described = emr.describe_ok(&name).await;
            assert_eq!(
                at(&described, &["SecurityConfiguration"]).as_str(),
                Some(case.document.as_str()),
                "case {case_label:?}: expected the accepted document back verbatim, got {described}"
            );
            continue;
        }

        assert_refused(&case_label, status, &payload, "InvalidRequestException");
        let (status, payload) = emr.describe(&name).await;
        assert_refused(
            &format!("{case_label} (nothing persisted)"),
            status,
            &payload,
            "InvalidRequestException",
        );
    }
}

/// THE RULE: the name identifies a security configuration, so a create under a name the
/// store already holds is refused — identical document or not — and the stored document
/// and its creation instant are left exactly as they were.
#[tokio::test]
async fn a_second_create_under_a_taken_name_is_refused_and_changes_nothing() {
    let original = json!({
        "EncryptionConfiguration": {
            "EnableAtRestEncryption": true,
            "AtRestEncryptionConfiguration": {
                "S3EncryptionConfiguration": { "EncryptionMode": "SSE-S3" }
            },
        }
    })
    .to_string();

    let emr = SecurityConfigurations::new();
    let created = emr.create_ok("taken", &original).await;
    let created_at = creation_secs("the first create", &created);

    // The documents a second create may carry: the same bytes, the same JSON with other
    // formatting, a different configuration, and an illegal one.
    for (label, document) in [
        ("the identical document", original.clone()),
        (
            "the same JSON, pretty-printed",
            serde_json::to_string_pretty(
                &serde_json::from_str::<Value>(&original).expect("the document is JSON"),
            )
            .expect("the document re-serializes"),
        ),
        ("a different document", "{}".to_string()),
        (
            "an illegal document",
            json!({ "EncryptionConfiguration": { "EnableAtRestEncryption": true } }).to_string(),
        ),
    ] {
        // Not a rename: the name is the same every time, which is the point.
        let (status, payload) = emr.create("taken", &document).await;
        assert_refused(label, status, &payload, "InvalidRequestException");

        let described = emr.describe_ok("taken").await;
        assert_eq!(
            at(&described, &["SecurityConfiguration"]).as_str(),
            Some(original.as_str()),
            "case {label:?}: the stored document must be untouched, got {described}"
        );
        assert_eq!(
            creation_secs(label, &described),
            created_at,
            "case {label:?}: the creation instant must be untouched, got {described}"
        );
    }

    // A free name is still available, and carries its own document and instant.
    let other = emr.create_ok("free", "{}").await;
    assert_eq!(
        at(&other, &["Name"]).as_str(),
        Some("free"),
        "case {:?}: expected the requested name back, got {other}",
        "a free name"
    );
    assert_eq!(
        at(&emr.describe_ok("free").await, &["SecurityConfiguration"]).as_str(),
        Some("{}"),
        "case {:?}: expected its own document, not the other one",
        "a free name"
    );
}

/// THE RULE: `DeleteSecurityConfiguration` removes exactly the named configuration, so
/// a later describe of that name and a second delete of it are both refused, every
/// other configuration is untouched, and the freed name can be created again.
#[tokio::test]
async fn delete_removes_one_configuration_and_is_not_idempotent() {
    let emr = SecurityConfigurations::new();
    let names = ["first", "second", "third"];
    for name in names {
        emr.create_ok(name, &json!({ "Name": name }).to_string())
            .await;
    }

    let (status, payload) = emr.delete("second").await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200, got {status} with {payload}",
        "deleting a configuration that exists"
    );

    let (status, payload) = emr.describe("second").await;
    assert_refused(
        "describing a deleted configuration",
        status,
        &payload,
        "InvalidRequestException",
    );

    let (status, payload) = emr.delete("second").await;
    assert_refused(
        "deleting a configuration twice",
        status,
        &payload,
        "InvalidRequestException",
    );

    let (status, payload) = emr.delete("never created").await;
    assert_refused(
        "deleting a name nothing was created under",
        status,
        &payload,
        "InvalidRequestException",
    );

    for name in ["first", "third"] {
        let described = emr.describe_ok(name).await;
        assert_eq!(
            at(&described, &["SecurityConfiguration"]).as_str(),
            Some(json!({ "Name": name }).to_string().as_str()),
            "case {name:?}: a neighbour's delete must leave this one alone, got {described}"
        );
    }

    // The freed name is creatable again, and what comes back is the NEW document.
    let recreated = emr.create_ok("second", "{}").await;
    let recreated_at = creation_secs("re-creating a deleted name", &recreated);
    let described = emr.describe_ok("second").await;
    assert_eq!(
        at(&described, &["SecurityConfiguration"]).as_str(),
        Some("{}"),
        "case {:?}: expected the new document, got {described}",
        "re-creating a deleted name"
    );
    assert_eq!(
        creation_secs("re-creating a deleted name", &described),
        recreated_at,
        "case {:?}: expected the new creation instant, got {described}",
        "re-creating a deleted name"
    );
}

/// THE RULE: what the service stores is bounded — a document past the accepted size and
/// a create past the account's security-configuration quota are refused with the
/// operation's client error and persist nothing, while the bounds themselves are
/// accepted and freeing a slot makes room again.
#[tokio::test]
async fn the_stored_document_and_the_store_are_bounded_at_their_limits() {
    let emr = SecurityConfigurations::new();

    // The document-size bound: one byte under, exactly at, and one byte past it.
    for (label, bytes, legal) in [
        (
            "one byte under the document limit",
            MAX_SECURITY_CONFIGURATION_BYTES - 1,
            true,
        ),
        (
            "exactly at the document limit",
            MAX_SECURITY_CONFIGURATION_BYTES,
            true,
        ),
        (
            "one byte past the document limit",
            MAX_SECURITY_CONFIGURATION_BYTES + 1,
            false,
        ),
    ] {
        let document = padded_document(bytes);
        assert_eq!(
            document.len(),
            bytes,
            "case {label:?}: the test document must be exactly {bytes} bytes"
        );
        let name = format!("sized-{bytes}");
        let (status, payload) = emr.create(&name, &document).await;
        if legal {
            assert_eq!(
                status, 200,
                "case {label:?}: expected HTTP 200, got {status} with a {} byte payload",
                payload.to_string().len()
            );
            assert_eq!(
                at(&emr.describe_ok(&name).await, &["SecurityConfiguration"]).as_str(),
                Some(document.as_str()),
                "case {label:?}: expected the document back verbatim"
            );
        } else {
            assert_refused(label, status, &payload, "InvalidRequestException");
            let (status, payload) = emr.describe(&name).await;
            assert_refused(
                &format!("{label} (nothing persisted)"),
                status,
                &payload,
                "InvalidRequestException",
            );
        }
    }

    // The quota on how many are stored: filling it is accepted, the next create is
    // refused and stores nothing, and deleting one frees the slot.
    let emr = SecurityConfigurations::new();
    for index in 0..MAX_SECURITY_CONFIGURATIONS {
        emr.create_ok(&format!("quota-{index:06}"), "{}").await;
    }
    let (status, payload) = emr.create("one too many", "{}").await;
    assert_refused(
        "a create past the quota",
        status,
        &payload,
        "InvalidRequestException",
    );
    let (status, payload) = emr.describe("one too many").await;
    assert_refused(
        "a create past the quota (nothing persisted)",
        status,
        &payload,
        "InvalidRequestException",
    );

    let (status, payload) = emr.delete("quota-000000").await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200, got {status} with {payload}",
        "freeing a slot"
    );
    let created = emr.create_ok("one too many", "{}").await;
    assert_eq!(
        at(&created, &["Name"]).as_str(),
        Some("one too many"),
        "case {:?}: the freed slot must be usable, got {created}",
        "freeing a slot"
    );
}

/// THE RULE: the creation instant is the instant of the create call — it advances as the
/// clock does, and reading a configuration never changes it, however many readers look
/// at once.
#[tokio::test]
async fn creation_instants_are_current_and_reads_leave_them_alone() {
    let emr = SecurityConfigurations::new();
    let mut stamped: Vec<(String, f64)> = Vec::new();

    for index in 0..3 {
        let name = format!("stamped-{index}");
        let before = epoch_secs(SystemTime::now());
        let created = emr.create_ok(&name, "{}").await;
        let after = epoch_secs(SystemTime::now()) + 1.0;
        let created_at = creation_secs(&name, &created);
        assert!(
            created_at >= before && created_at <= after,
            "case {name:?}: expected a CreationDateTime inside [{before}, {after}], got {created_at}"
        );
        if let Some((earlier_name, earlier)) = stamped.last() {
            assert!(
                created_at >= *earlier,
                "case {name:?}: expected an instant at or after {earlier_name}'s {earlier}, got {created_at}"
            );
        }
        stamped.push((name, created_at));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Concurrent readers of one configuration all see the same answer, and the answer
    // does not move: describing is reporting, not a write.
    for (name, created_at) in &stamped {
        let (first, second, third) = tokio::join!(
            emr.describe_ok(name),
            emr.describe_ok(name),
            emr.describe_ok(name)
        );
        for (index, described) in [&first, &second, &third].into_iter().enumerate() {
            assert_eq!(
                creation_secs(name, described),
                *created_at,
                "case {name:?} (concurrent reader {index}): expected the creation instant {created_at}, got {described}"
            );
            assert_eq!(
                at(described, &["Name"]).as_str(),
                Some(name.as_str()),
                "case {name:?} (concurrent reader {index}): expected its own name, got {described}"
            );
        }
    }

    // And after all that reading, every instant is still the one it was.
    for (name, created_at) in &stamped {
        assert_eq!(
            creation_secs(name, &emr.describe_ok(name).await),
            *created_at,
            "case {name:?}: expected the creation instant {created_at} to survive every read"
        );
    }
}

/// One captured `tracing` event: its target and its fields, rendered as strings.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// Collects an event's fields into strings, whatever type they were recorded as.
struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps every event it is handed, so a test can read the
/// audit records back. Installed ONCE as this test binary's global subscriber (see
/// [`audit_capture`]): a thread-scoped subscriber would not do, because `tracing` caches
/// a callsite's interest globally and a sibling test running with no subscriber would
/// cache the handler's audit callsite as "nobody is listening". Nothing is written to
/// disk.
#[derive(Clone, Default)]
struct Capture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Capture {
    /// Take the audit records `akid` signed for, oldest first, and forget the rest.
    ///
    /// Filtering on the presented access key id is what keeps a sibling suite's calls
    /// out of this one's assertions, since the capture is process-wide.
    fn drain_audit_records(&self, akid: &str) -> Vec<CapturedEvent> {
        let mut events = self.events.lock().expect("the capture buffer");
        let drained: Vec<CapturedEvent> = events
            .iter()
            .filter(|event| {
                event.target == AUDIT_TARGET && event.field("access_key_id") == Some(akid)
            })
            .cloned()
            .collect();
        events.clear();
        drained
    }
}

/// This binary's one capture, installed as the global `tracing` subscriber on first use.
static AUDIT_CAPTURE: std::sync::OnceLock<Capture> = std::sync::OnceLock::new();

fn audit_capture() -> &'static Capture {
    AUDIT_CAPTURE.get_or_init(|| {
        let capture = Capture::default();
        // Installing the global subscriber also rebuilds `tracing`'s callsite interest
        // cache, so a callsite another test already evaluated starts being delivered.
        let _ = tracing::subscriber::set_global_default(capture.clone());
        capture
    })
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // This suite asserts on events, not spans; one id for every span is enough.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut FieldCollector(&mut fields));
        self.events
            .lock()
            .expect("the capture buffer")
            .push(CapturedEvent {
                target: event.metadata().target().to_string(),
                fields,
            });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// THE RULE: every state-changing security-configuration call leaves exactly ONE
/// structured audit record — on its refusal branches too — naming the operation, the
/// resource, the presented access key id, the request target and the outcome, and
/// carrying no part of the caller's document; a call that only reports leaves none.
#[tokio::test]
async fn state_changing_calls_emit_one_audit_record_each() {
    let capture = audit_capture();

    let secret_document = json!({
        "EncryptionConfiguration": {
            "EnableAtRestEncryption": true,
            "AtRestEncryptionConfiguration": {
                "S3EncryptionConfiguration": {
                    "EncryptionMode": "SSE-KMS",
                    "AwsKmsKey": "arn:aws:kms:us-east-1:123456789012:key/not-in-the-log",
                }
            },
        }
    })
    .to_string();

    // This suite's own access key id, which no other suite presents, so the capture's
    // records can be told apart from a sibling test's.
    let emr = SecurityConfigurations::with_akid(AUDIT_AKID);

    // Harness self-test, so a failure below can never be mistaken for a missing record.
    tracing::info!(target: "emulator::audit", access_key_id = AUDIT_AKID, "probe");
    let probe = capture.drain_audit_records(AUDIT_AKID);
    assert_eq!(
        probe.len(),
        1,
        "the capture harness must see an audit record emitted on this thread, got {probe:?}"
    );

    // (label, the call, the resource it touches, the outcome it must record)
    enum Call {
        Create(&'static str, String),
        Delete(&'static str),
    }
    let audited = [
        (
            "a create that stores a configuration",
            Call::Create("audited", secret_document.clone()),
            "audited",
            "security-configuration-created",
        ),
        (
            "a create under a taken name",
            Call::Create("audited", secret_document.clone()),
            "audited",
            "conflict",
        ),
        (
            "a create of an illegal document",
            Call::Create(
                "refused",
                json!({ "EncryptionConfiguration": { "EnableAtRestEncryption": true } })
                    .to_string(),
            ),
            "refused",
            "validation-refused",
        ),
        (
            "a delete that removes a configuration",
            Call::Delete("audited"),
            "audited",
            "security-configuration-deleted",
        ),
        (
            "a delete of a name nothing holds",
            Call::Delete("audited"),
            "audited",
            "not-found",
        ),
    ];

    for (label, call, resource, outcome) in audited {
        let operation = match call {
            Call::Create(name, document) => {
                let _ = emr.create(name, &document).await;
                "CreateSecurityConfiguration"
            }
            Call::Delete(name) => {
                let _ = emr.delete(name).await;
                "DeleteSecurityConfiguration"
            }
        };

        let records = capture.drain_audit_records(AUDIT_AKID);
        assert_eq!(
            records.len(),
            1,
            "case {label:?}: expected exactly one audit record, captured {records:?}"
        );
        let record = &records[0];
        for (field, expected) in [
            ("operation", operation),
            ("resource", resource),
            ("outcome", outcome),
            ("access_key_id", AUDIT_AKID),
            ("request_target", REQUEST_TARGET),
        ] {
            assert_eq!(
                record.field(field),
                Some(expected),
                "case {label:?}: expected {field}={expected:?}, got {record:?}"
            );
        }
        // Metadata only: no part of the caller's document reaches the log.
        let rendered = format!("{record:?}");
        assert!(
            !rendered.contains("not-in-the-log") && !rendered.contains("EncryptionMode"),
            "case {label:?}: the record must carry no part of the document, got {rendered}"
        );
    }

    // A reporting call records nothing: there is no state change to attribute.
    emr.create_ok("reported", "{}").await;
    let _ = capture.drain_audit_records(AUDIT_AKID);
    let (status, _) = emr.describe("reported").await;
    assert_eq!(status, 200, "the reported configuration must be readable");
    let (status, _) = emr.describe("never created").await;
    assert_eq!(status, 400, "an unknown name must still be refused");
    let leaked = capture.drain_audit_records(AUDIT_AKID);
    assert!(
        leaked.is_empty(),
        "case {:?}: a reporting operation must emit no audit record, captured {leaked:?}",
        "DescribeSecurityConfiguration"
    );
}
