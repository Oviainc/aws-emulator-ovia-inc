// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of WHICH stored resource a caller-supplied `ResourceArn` names, over
//! every taggable kind this service holds and over the operations that resolve one:
//! `TagResource`, `ListTagsForResource`, `UntagResource` and
//! `BatchDeleteInstrumentationConfigurations` by ARN list.
//!
//! The rule under test: an ARN of this service's namespace and this account names the stored
//! resource its RESOURCE field identifies, whatever REGION or PARTITION the ARN carries —
//! this process holds one account's and one Region's records, so those two fields render the
//! writer's SigV4 credential scope rather than any part of a resource's identity, and both
//! modelled ARN patterns leave the Region unconstrained
//! (`smithy_query(query="shape",shape="InstrumentationConfigurationArn,ServiceLevelObjectiveArn")`
//! -> `^arn:[^:]+:application-signals:[^:]+:…` and
//! `^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/…`). The fields that ARE part
//! of the identity still narrow the lookup: an ARN of another vendor, of another account, or
//! naming another resource of this service is the modelled `ResourceNotFoundException` in
//! every spelling.
//!
//! Every assertion is made over a TABLE: the rows span the resource KIND (an SLO, and
//! instrumentation configurations whose service and environment carry the `/` and `:` the
//! modelled ARN doc calls out) crossed with the ARN spellings the rule generalises over. The
//! request helpers are the ones `slo_lifecycle`, `resource_tags`, `tag_attachment` and
//! `instrumentation_configuration_tags` already drive this service's `restJson1` surface
//! with, so every suite speaks to one harness.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::instrumentation_configuration_tags::{created, get_configuration, Case, SNAPSHOT};
use crate::resource_tags::{expected_map, list_tags_raw, reported_tags, tag, tag_list, untag};
use crate::slo_lifecycle::{arn_uri_segment, call, create, router, sli_config, ACCOUNT, REGION};
use crate::tag_attachment::tag_resource;

/// The two `InstrumentationType` values
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// The `BatchDeleteErrorCode` a per-item failure carries for an ARN that names no stored
/// configuration (`smithy_query(query="shape",shape="BatchDeleteErrorCode")` ->
/// `["ResourceNotFoundException", "AccessDeniedException", "InternalServiceException"]`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// One spelling of an ARN's scope fields: the PARTITION and REGION a caller may write, which
/// the rule says name no part of a resource's identity.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Spelling {
    pub(crate) label: &'static str,
    pub(crate) partition: &'static str,
    pub(crate) region: &'static str,
}

/// The scope spellings the rule is asserted over.
///
/// The regions are AWS Region labels other than the one the suite's credential scope mints
/// ARNs in, including the longest label a model's ARN pattern admits (`[a-z0-9-]{1,20}`, the
/// bound `service_common::arn` reads a Region under), and the partitions are the two the
/// modelled `ServiceLevelObjectiveArn` pattern spells
/// (`^arn:(aws|aws-us-gov):application-signals:…`).
pub(crate) fn spellings() -> Vec<Spelling> {
    vec![
        Spelling {
            label: "the region the create minted the ARN in",
            partition: "aws",
            region: REGION,
        },
        Spelling {
            label: "another region of the same partition",
            partition: "aws",
            region: "us-east-1",
        },
        Spelling {
            label: "a region on the other side of the world",
            partition: "aws",
            region: "ap-southeast-2",
        },
        Spelling {
            label: "the longest Region label a modelled ARN pattern admits",
            partition: "aws",
            region: "ap-southeast-12345678",
        },
        Spelling {
            label: "the other partition the modelled SLO ARN pattern spells",
            partition: "aws-us-gov",
            region: "us-gov-west-1",
        },
    ]
}

/// `arn` rewritten with this spelling's partition and region, leaving every other field —
/// the vendor, the account and the whole resource field, colons included — exactly as the
/// service minted it.
///
/// An ARN is six colon-separated fields with the resource LAST, so splitting into six parts
/// keeps a resource field that itself carries `:` (which an instrumentation configuration's
/// environment may, per the modelled `InstrumentationConfigurationArn` doc) whole.
pub(crate) fn respelled(arn: &str, spelling: &Spelling) -> String {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "expected the minted ARN {arn:?} to carry six fields, got {fields:?}"
    );
    format!(
        "arn:{}:{}:{}:{}:{}",
        spelling.partition, fields[2], spelling.region, fields[4], fields[5]
    )
}

/// One stored resource the rule is asserted over: its label, the ARN the create minted, and
/// the tags the create persisted.
#[derive(Clone, Debug)]
struct Subject {
    label: &'static str,
    arn: String,
    tags: BTreeMap<String, String>,
}

/// A configuration row whose service and environment exercise what an ARN's resource field
/// may carry.
fn configuration_case(
    label: &'static str,
    instrumentation_type: &'static str,
    service: &'static str,
    environment: &'static str,
    language: &'static str,
    tags: Option<Vec<(String, String)>>,
) -> Case {
    Case {
        label,
        instrumentation_type,
        service,
        environment,
        location: json!({
            "Language": language,
            "ClassName": "Cart",
            "FilePath": "src/main/java/com/example/Cart.java",
            "LineNumber": 142,
        }),
        tags,
    }
}

/// The instrumentation configurations the rule is asserted over: both instrumentation types,
/// and service and environment names carrying the `/` and `:` the modelled
/// `InstrumentationConfigurationArn` doc says they may ("service and environment can contain
/// '/' and ':'"), which is what a whole-resource-field comparison has to keep intact.
fn configuration_cases() -> Vec<Case> {
    vec![
        configuration_case(
            "a BREAKPOINT configuration whose environment carries ':' and '/'",
            BREAKPOINT,
            "checkout",
            "eks:cluster-prod/namespace",
            "Java",
            Some(vec![tag("team", "checkout"), tag("env", "prod")]),
        ),
        configuration_case(
            "a PROBE configuration whose service carries '/'",
            PROBE,
            "pricing/eu",
            "default",
            "Python",
            Some(vec![tag("team", "pricing")]),
        ),
        configuration_case(
            "a BREAKPOINT configuration created with no Tags member at all",
            BREAKPOINT,
            "search",
            "ecs:cluster/search-svc",
            "Javascript",
            None,
        ),
    ]
}

/// Create the SLO called `name` carrying `tags` and hand back the ARN the service minted.
async fn create_slo(router: &MultiServiceRouter, name: &str, tags: &[(String, String)]) -> String {
    let (status, created) = create(
        router,
        json!({
            "Name": name,
            "SliConfig": sli_config("checkout", "GET /cart", 100.0),
            "Tags": tag_list(tags),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected the setup create to succeed, got {status} with {created}"
    );
    created["Slo"]["Arn"]
        .as_str()
        .expect("the create answers the stored SLO's ARN")
        .to_string()
}

/// Every taggable kind this service holds, stored on `router`, each with the ARN its own
/// create minted — never an ARN the test spelled.
async fn subjects(router: &MultiServiceRouter) -> Vec<Subject> {
    let slo_tags = vec![tag("env", "prod"), tag("team", "payments")];
    let mut subjects = vec![Subject {
        label: "a service level objective",
        arn: create_slo(router, "arn-identity-slo", &slo_tags).await,
        tags: expected_map(&slo_tags),
    }];
    for case in configuration_cases() {
        let (arn, _hash) = created(router, &case).await;
        subjects.push(Subject {
            label: case.label,
            arn,
            tags: case.expected(),
        });
    }
    subjects
}

/// `POST /batch-delete-instrumentation-configurations` by ARN list.
async fn batch_delete_arns(
    router: &MultiServiceRouter,
    instrumentation_type: &str,
    arns: &[String],
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/batch-delete-instrumentation-configurations",
        Some(json!({
            "DeletionTarget": {
                "ResourceArns": {
                    "InstrumentationType": instrumentation_type,
                    "ResourceArns": arns,
                }
            }
        })),
    )
    .await
}

/// Every tagging operation resolves a stored resource of EITHER kind through an ARN carrying
/// any region or partition: the attachment lands on the resource the create minted the ARN
/// for, the reader reports it for that resource's own ARN too, and the removal takes it off
/// the same tag set.
#[tokio::test]
async fn a_tagging_operation_resolves_an_arn_whatever_scope_it_carries() {
    let router = router();
    for subject in subjects(&router).await {
        for spelling in spellings() {
            let spelled = respelled(&subject.arn, &spelling);
            let case = format!("{:?} addressed by {:?}", subject.label, spelling.label);

            // The key records which spelling attached it, so an attachment that landed
            // somewhere else is visible in the resource's own tag set.
            let attached = tag(&format!("scope-{}", spelling.region), spelling.partition);
            let (status, body) =
                tag_resource(&router, &spelled, std::slice::from_ref(&attached)).await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200 attaching through {spelled}, got {status} with {body}"
            );

            let mut expected = subject.tags.clone();
            expected.insert(attached.0.clone(), attached.1.clone());

            // Read back through BOTH spellings: the resource the attachment landed on is the
            // one the create minted the ARN for.
            for (who, arn) in [("the spelling it was attached by", &spelled), ("its own minted ARN", &subject.arn)] {
                let (status, body) = list_tags_raw(&router, &arn_uri_segment(arn)).await;
                assert_eq!(
                    status, 200,
                    "case {case}: expected HTTP 200 reading through {who} ({arn}), got {status} with {body}"
                );
                assert_eq!(
                    reported_tags(&body),
                    expected,
                    "case {case}: expected {who} to report {expected:?}, got {:?}",
                    reported_tags(&body)
                );
            }

            let (status, body) =
                untag(&router, &spelled, std::slice::from_ref(&attached.0)).await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200 detaching through {spelled}, got {status} with {body}"
            );
            let (status, body) = list_tags_raw(&router, &arn_uri_segment(&subject.arn)).await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200 from the post-untag read, got {status} with {body}"
            );
            assert_eq!(
                reported_tags(&body),
                subject.tags,
                "case {case}: expected the removal to leave {:?}, got {:?}",
                subject.tags,
                reported_tags(&body)
            );
        }
    }
}

/// The fields that ARE part of an ARN's identity still narrow the lookup, in every spelling
/// of the scope fields that are not: an ARN of another vendor, of another account, or naming
/// another resource of this service is the modelled `ResourceNotFoundException` (404) however
/// its region and partition are written, and the refused call attaches nothing.
#[tokio::test]
async fn an_arn_differing_in_the_identity_is_not_found_in_any_scope() {
    let router = router();
    for subject in subjects(&router).await {
        // Each row changes exactly ONE field of the identity the ARN spells.
        let other_vendor = subject.arn.replace("application-signals", "cloudwatch");
        let other_account = subject.arn.replace(ACCOUNT, "111111111111");
        let other_resource = format!("{}-not-stored", subject.arn);
        let missing = [
            ("another service's ARN", other_vendor),
            ("another account's ARN", other_account),
            ("another resource of this service", other_resource),
        ];

        for (label, arn) in missing {
            for spelling in spellings() {
                let spelled = respelled(&arn, &spelling);
                let case =
                    format!("{label:?} for {:?} spelled with {:?}", subject.label, spelling.label);

                let (status, error) = list_tags_raw(&router, &arn_uri_segment(&spelled)).await;
                assert_eq!(
                    status, 404,
                    "case {case}: expected HTTP 404 for {spelled}, got {status} with {error}"
                );
                assert_eq!(
                    error["__type"].as_str(),
                    Some("ResourceNotFoundException"),
                    "case {case}: expected a ResourceNotFoundException, got {error}"
                );
                assert_eq!(
                    error["ResourceId"].as_str(),
                    Some(spelled.as_str()),
                    "case {case}: expected the refusal to name the ARN the request carried, got {error}"
                );

                let (status, error) =
                    tag_resource(&router, &spelled, &[tag("leaked", "yes")]).await;
                assert_eq!(
                    status, 404,
                    "case {case}: expected HTTP 404 attaching to {spelled}, got {status} with {error}"
                );
            }
        }

        // Nothing a refused call carried reached the stored resource.
        let (status, body) = list_tags_raw(&router, &arn_uri_segment(&subject.arn)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the stored resource to still answer 200, got {status} with {body}",
            subject.label
        );
        assert_eq!(
            reported_tags(&body),
            subject.tags,
            "case {:?}: expected the refused calls to change nothing, got {:?}",
            subject.label,
            reported_tags(&body)
        );
    }
}

/// A batch delete by ARN list resolves a configuration's ARN by the same rule: a spelling
/// carrying any region or partition removes the one stored configuration, reported by the
/// ARN that named it, while an ARN of another account fails that ITEM whatever scope it
/// carries.
#[tokio::test]
async fn a_batch_delete_resolves_an_arn_whatever_scope_it_carries() {
    for spelling in spellings() {
        let router = router();
        let case = configuration_case(
            "a configuration deleted by ARN list",
            PROBE,
            "pricing/eu",
            "eks:cluster-prod/namespace",
            "Java",
            Some(vec![tag("team", "pricing")]),
        );
        let (arn, hash) = created(&router, &case).await;
        let spelled = respelled(&arn, &spelling);
        let other_account = respelled(&arn.replace(ACCOUNT, "111111111111"), &spelling);
        let label = format!("{:?} deleted by {:?}", case.label, spelling.label);

        let (status, body) = batch_delete_arns(
            &router,
            case.instrumentation_type,
            &[spelled.clone(), other_account.clone()],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: expected HTTP 200 from the batch delete, got {status} with {body}"
        );
        assert_eq!(
            body["DeletedCount"].as_i64(),
            Some(1),
            "case {label}: expected exactly the one stored configuration deleted, got {body}"
        );
        assert_eq!(
            body["SuccessfulDeletions"][0]["ResourceArn"].as_str(),
            Some(spelled.as_str()),
            "case {label}: expected the removal reported by the ARN that named it, got {body}"
        );
        assert_eq!(
            body["Errors"][0]["ResourceArn"].as_str(),
            Some(other_account.as_str()),
            "case {label}: expected another account's ARN to fail its own item, got {body}"
        );
        assert_eq!(
            body["Errors"][0]["Code"].as_str(),
            Some(RESOURCE_NOT_FOUND),
            "case {label}: expected the per-item not-found code, got {body}"
        );

        // The delete really removed the configuration the spelling named.
        let (status, error) = get_configuration(&router, case.get_body(&hash)).await;
        assert_eq!(
            status, 404,
            "case {label}: expected the configuration to be gone, got {status} with {error}"
        );
        let (status, error) = list_tags_raw(&router, &arn_uri_segment(&arn)).await;
        assert_eq!(
            status, 404,
            "case {label}: expected its tags to be gone with it, got {status} with {error}"
        );
    }
}

/// The signal type an ARN spells is part of the identity it names, so the resource field is
/// what a lookup compares — not the scope fields around it.
///
/// `SNAPSHOT` is the only `DynamicInstrumentationSignalType` the model declares, so the row
/// that changes it is written here rather than created: an ARN whose signal-type segment
/// names no stored configuration must not resolve to the one stored at that location.
#[tokio::test]
async fn an_arn_naming_another_signal_type_resolves_nothing() {
    let router = router();
    let case = configuration_case(
        "a configuration addressed by another signal type",
        BREAKPOINT,
        "checkout",
        "eks:cluster-prod/namespace",
        "Java",
        Some(vec![tag("team", "checkout")]),
    );
    let (arn, _hash) = created(&router, &case).await;

    for spelling in spellings() {
        let other_signal = respelled(&arn.replace(SNAPSHOT, "OTHER_SIGNAL"), &spelling);
        let (status, error) = list_tags_raw(&router, &arn_uri_segment(&other_signal)).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected HTTP 404 for {other_signal}, got {status} with {error}",
            spelling.label
        );
    }
}
