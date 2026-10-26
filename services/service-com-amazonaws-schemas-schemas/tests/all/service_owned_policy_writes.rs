// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHO may write a registry's resource-based policy.
//!
//! The rule: what is in a service-owned registry is AWS's, so a caller may read it but
//! not write to it — attaching a policy to one (`PutResourcePolicy`) and removing one
//! from it (`DeleteResourcePolicy`) are refused 403 `ForbiddenException` with the
//! service's one sentence for that refusal, exactly as every other write to a
//! service-owned registry is, and the registry is left exactly as the store seeded it.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over:
//! the policy operation the request calls, the size, nesting, whitespace and member
//! count of the document a write carries, and whether the request states a `RevisionId`
//! precondition. Each row also drives the SAME write against a registry the caller
//! created, so every row shows the refusal is about who owns the registry and not about
//! what the write would change.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`, and
//! the `/v1/policy` route from `resource_policy`. Nothing here touches the state struct
//! directly, so a handler that refused without leaving the store alone would still have
//! to make the reads that follow agree.

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::{
    AWS_MANAGED_REGISTRIES, CODE_FORBIDDEN, CODE_RESOURCE_NOT_FOUND,
};

use crate::registry_schema_lifecycle::{create_registry, registry_uri, Answer, Emulator};
use crate::resource_policy::policy_uri;

/// The baseline policy every caller-owned registry in this suite starts out carrying,
/// so a row's write to it has a stored policy to supersede or remove.
const BASELINE: &str = r#"{"Version":"2012-10-17","Statement":[]}"#;

/// A revision id shaped like the ones this emulator mints, that no write ever minted:
/// a precondition no store can satisfy.
const UNMINTED_REVISION: &str = "ffffffff-0000-4000-8000-ffffffffffff";

/// One write to a registry's resource-based policy, as the request puts it on the wire.
#[derive(Debug)]
enum Write {
    /// `PutResourcePolicy`: the document, and the `RevisionId` precondition if the
    /// request states one.
    Put {
        document: String,
        revision: Option<&'static str>,
    },
    /// `DeleteResourcePolicy`.
    Delete,
}

/// One row: a policy write, and the status it takes against a registry the CALLER owns
/// — the control that says the refusal a service-owned registry answers is about
/// ownership and nothing else.
#[derive(Debug)]
struct Case {
    case: &'static str,
    write: Write,
    /// The status this same write answers for a caller-owned registry that already
    /// carries [`BASELINE`].
    own_status: u16,
}

/// The table. Rows differ along the operation called, the document's size, nesting,
/// whitespace and member count, and whether a `RevisionId` precondition is stated.
fn write_cases() -> Vec<Case> {
    vec![
        Case {
            case: "attach the smallest JSON object there is",
            write: Write::Put {
                document: "{}".to_string(),
                revision: None,
            },
            own_status: 200,
        },
        Case {
            case: "attach a single-statement policy",
            write: Write::Put {
                document: json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Sid": "AllowDescribe",
                        "Effect": "Allow",
                        "Principal": { "AWS": "000000000000" },
                        "Action": ["schemas:DescribeRegistry"],
                        "Resource": "*"
                    }]
                })
                .to_string(),
                revision: None,
            },
            own_status: 200,
        },
        Case {
            case: "attach a pretty-printed document, with newlines and tabs",
            write: Write::Put {
                document: "{\n\t\"Version\" : \"2012-10-17\",\n\t\"Statement\" : [ ]\n}"
                    .to_string(),
                revision: None,
            },
            own_status: 200,
        },
        Case {
            case: "attach a document of several kilobytes",
            write: Write::Put {
                document: format!(
                    r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","Principal":"*","Action":"schemas:*","Resource":"*","Sid":"{}"}}]}}"#,
                    "x".repeat(8_000)
                ),
                revision: None,
            },
            own_status: 200,
        },
        Case {
            // Who owns the registry is settled before any precondition on its stored
            // policy is: a service-owned registry answers the ownership refusal, where
            // a caller's own registry answers the 412 the stale revision earns.
            case: "attach conditioned on a revision no write ever minted",
            write: Write::Put {
                document: BASELINE.to_string(),
                revision: Some(UNMINTED_REVISION),
            },
            own_status: 412,
        },
        Case {
            case: "attach conditioned on the empty revision",
            write: Write::Put {
                document: BASELINE.to_string(),
                revision: Some(""),
            },
            own_status: 412,
        },
        Case {
            // A service-owned registry holds no policy to remove, and the refusal is
            // still the ownership one: a caller may not reach that registry's policy at
            // all, so it is never told whether one is there.
            case: "remove the policy",
            write: Write::Delete,
            own_status: 204,
        },
    ]
}

/// The service's one sentence for a caller's write to a service-owned registry, as
/// every handler that refuses one words it.
fn service_owned_sentence(registry: &str) -> String {
    format!(
        "Registry {registry} is an AWS-managed registry; its contents are owned by AWS \
         and cannot be created, updated or deleted."
    )
}

/// Send one row's write to `registry`.
async fn send(emulator: &Emulator, write: &Write, registry: &str) -> Answer {
    match write {
        Write::Put { document, revision } => {
            let mut body = json!({ "Policy": document });
            if let Some(revision) = revision {
                body["RevisionId"] = json!(revision);
            }
            emulator
                .call("PUT", &policy_uri(Some(registry)), Some(body))
                .await
        }
        Write::Delete => {
            emulator
                .call("DELETE", &policy_uri(Some(registry)), None)
                .await
        }
    }
}

/// RULE: what is in a service-owned registry is AWS's, so a caller may read it but not
/// write to it — every write to one of its resource-based policy is refused 403
/// `ForbiddenException` with the service's sentence for that refusal, the registry still
/// holds no policy afterwards, and the same write to a registry the caller created is
/// served.
#[tokio::test]
async fn writing_a_service_owned_registrys_policy_is_refused_and_stores_nothing() {
    for case in write_cases() {
        // Source: `crate::state::AWS_MANAGED_REGISTRIES` — the service-owned registries
        // every account sees before any call is made.
        for registry in AWS_MANAGED_REGISTRIES {
            let emulator = Emulator::new();
            let label = format!("{}/{registry}", case.case);

            let refused = send(&emulator, &case.write, registry).await;
            assert_eq!(
                refused.status, 403,
                "case {label:?}: expected 403 from a write to a service-owned registry's \
                 policy, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type(),
                "ForbiddenException",
                "case {label:?}: expected the ForbiddenException shape, got {:?}",
                refused.header("x-amzn-errortype")
            );
            assert_eq!(
                refused.error_code(),
                "ForbiddenException",
                "case {label:?}: expected the ForbiddenException wire code, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.str_at("Code"),
                CODE_FORBIDDEN,
                "case {label:?}: expected the {CODE_FORBIDDEN} condition code, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.str_at("Message"),
                service_owned_sentence(registry),
                "case {label:?}: wrong sentence for a write to a service-owned registry"
            );

            // The refusal is the one every OTHER write to this registry answers: one
            // refusal class, one sentence.
            let sibling = emulator
                .call(
                    "PUT",
                    &registry_uri(registry),
                    Some(json!({ "Description": "rewritten by a caller" })),
                )
                .await;
            assert_eq!(
                sibling.str_at("Message"),
                refused.str_at("Message"),
                "case {label:?}: the policy write must be refused in the same words as \
                 UpdateRegistry refuses the same registry, got {:?}",
                sibling.body
            );

            // Nothing was written: the registry still carries no policy, and reading is
            // still allowed — the refusal is on the write, not on the resource.
            let read = emulator.call("GET", &policy_uri(Some(registry)), None).await;
            assert_eq!(
                read.status, 404,
                "case {label:?}: a service-owned registry must still carry no policy, \
                 got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.str_at("Code"),
                CODE_RESOURCE_NOT_FOUND,
                "case {label:?}: expected the {CODE_RESOURCE_NOT_FOUND} condition code \
                 from GetResourcePolicy, got {:?}",
                read.body
            );

            // And the registry itself is still there to read.
            let described = emulator.call("GET", &registry_uri(registry), None).await;
            assert_eq!(
                described.status, 200,
                "case {label:?}: the service-owned registry is still readable, got {} {:?}",
                described.status, described.body
            );
        }

        // The control: the SAME write against a registry this caller created, carrying
        // the baseline policy, takes the status the write itself earns.
        let emulator = Emulator::new();
        let own = "owned-registry";
        create_registry(&emulator, own).await;
        let seeded = emulator
            .call(
                "PUT",
                &policy_uri(Some(own)),
                Some(json!({ "Policy": BASELINE })),
            )
            .await;
        assert_eq!(
            seeded.status, 200,
            "case {:?}: expected 200 seeding the caller's own policy, got {} {:?}",
            case.case, seeded.status, seeded.body
        );

        let answered = send(&emulator, &case.write, own).await;
        assert_eq!(
            answered.status, case.own_status,
            "case {:?}: expected {} from the same write to a registry the caller owns, \
             got {} {:?}",
            case.case, case.own_status, answered.status, answered.body
        );
    }
}
