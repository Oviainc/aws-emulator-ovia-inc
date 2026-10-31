// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE rule every `@idempotencyToken` operation of this
//! service that DEFAULTS a member follows: a retry is compared on the request's parameters
//! as the operation RESOLVES them, so a member the operation defaults itself counts as the
//! value the request would record and not as the spelling it was asked by.
//!
//! "If you retry a request with the same client request token and the same parameters after
//! the original request has completed successfully, the result of the original request is
//! returned" (`smithy_query(query="shape", shape="ClientRequestToken")`). Two requests that
//! would record the identical resource therefore ask for the same thing, however each
//! spelled a member the operation gives a default to: omitting the member and sending the
//! default value explicitly are ONE request, so the second is the replayed success rather
//! than the modelled `ConflictException` (HTTP 409). Resolving a member does not flatten the
//! comparison: a value that differs from the one the original request recorded is still
//! another request and is still refused.
//!
//! The rules pinned here, each over a TABLE of spellings rather than one recorded request:
//!
//! 1. `CreateLensVersion` holds `IsMajorVersion` to the rule: the rows span every pair of
//!    spellings the member takes — omitted, `false` and `true` — so each pair that would
//!    publish the same version replays and each pair that would publish a different one is
//!    the 409, with exactly one version published either way.
//! 2. `CreateWorkload` holds its defaulted members to the same rule: the rows span the list
//!    members it defaults, each retried with the default value the create records for an
//!    absent member.
//! 3. Resolving those members does not flatten the fingerprint: a `CreateWorkload` retry
//!    carrying a value other than the recorded default is still another request.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the `CreateWorkload` builder are the shared
//! `super::workload_harness`; the import and publish wire helpers are the ones
//! `super::custom_lens_import` and `super::custom_lens_versions` already spell.

use serde_json::{json, Value};

use super::custom_lens_import::{
    import, imported_arn, lens_document, member, wait_for_the_import_to_land,
};
use super::custom_lens_versions::create_version;
use super::workload_harness::{created_id, minimal_create, stored_workload_count, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The version every row of rule 1 publishes: one name, so the only parameter a row varies
/// is the spelling of `IsMajorVersion`. Inside the modelled `@length(min: 1, max: 32)`
/// (`smithy_query(query="operation", operation="CreateLensVersion")`).
const PUBLISHED_VERSION: &str = "1.0";

/// Import a fresh custom lens for one row and answer its ARN.
async fn import_lens(client: &Client, case: &str) -> String {
    let document = lens_document("Defaulted member lens", None, &[("governance", 1, 2)]).to_string();
    let answer = import(client, &document, None, &format!("import-{case}"), None).await;
    imported_arn(&answer, case)
}

/// The versions a stored lens holds, each with the major-version flag it was PUBLISHED
/// with, sorted by version name. Read straight from the shared state: nothing over the wire
/// reports a published version's `IsMajorVersion` (`ListLensVersions` is not an operation of
/// this service), and the flag is what a publish records for the member under test.
fn published_versions(client: &Client, alias: &str) -> Vec<(String, bool)> {
    let store = client.store();
    let store = store.lock();
    let mut published: Vec<(String, bool)> = store
        .lens_by_alias(alias)
        .map(|record| {
            record
                .versions
                .iter()
                .map(|(version, published)| (version.clone(), published.is_major_version))
                .collect()
        })
        .unwrap_or_default();
    published.sort();
    published
}

/// The refusal a token replayed over a DIFFERENT request gets: 409 `ConflictException`
/// worded by the token it was spent under.
fn assert_token_conflict(case: &str, answer: &Answer, token: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ConflictException",
        "case {case:?}: expected ConflictException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!(
            "[Conflict] A request with ClientRequestToken {token} was already submitted with \
             different parameters."
        ),
        "case {case:?}: expected the replayed-token conflict message, got {answer:?}"
    );
}

/// Rule 1. The rows are every pair of spellings `IsMajorVersion` takes: a pair that would
/// publish the same version is one request, a pair that would publish a different one is
/// not.
#[tokio::test]
async fn a_publish_retry_is_compared_on_the_is_major_version_it_would_record() {
    // (case, the spelling the original publish used, the spelling the retry uses). The
    // member is a plain optional boolean with no modelled default
    // (`smithy_query(query="operation", operation="CreateLensVersion")`: `"IsMajorVersion":
    // {"target": "IsMajorVersion", "type": "boolean"}`), so an omitted member reaches the
    // handler absent and the publish records the value it defaults to.
    let spellings: [(&str, Option<bool>); 3] =
        [("omitted", None), ("false", Some(false)), ("true", Some(true))];

    for (first_label, first_spelling) in spellings {
        for (second_label, retried_spelling) in spellings {
            let case = format!("{first_label}-then-{second_label}");
            let client = Client::new();
            let arn = import_lens(&client, &case).await;
            wait_for_the_import_to_land();

            let token = format!("tok-{case}");
            let first = create_version(
                &client,
                &arn,
                PUBLISHED_VERSION,
                first_spelling,
                &token,
            )
            .await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: the first publish must succeed, got {first:?}"
            );
            // What the first publish RECORDED for the member, which is what the retry is
            // compared against: an absent member records the value the operation defaults
            // it to.
            let recorded = first_spelling.unwrap_or(false);
            let published = vec![(PUBLISHED_VERSION.to_string(), recorded)];
            assert_eq!(
                published_versions(&client, &arn),
                published,
                "case {case:?}: expected the publish to record IsMajorVersion {recorded}"
            );

            let second = create_version(
                &client,
                &arn,
                PUBLISHED_VERSION,
                retried_spelling,
                &token,
            )
            .await;
            // The retry is the same request exactly when it would publish the same version.
            if retried_spelling.unwrap_or(false) == recorded {
                assert_eq!(
                    second.status, 200,
                    "case {case:?}: a retry that would publish the same version must replay, \
                     got {second:?}"
                );
                assert_eq!(
                    second.body, first.body,
                    "case {case:?}: a replay must report the original publish ({:?}), got \
                     {second:?}",
                    first.body
                );
            } else {
                assert_token_conflict(&case, &second, &token);
                assert_eq!(
                    member(&second, "ResourceType"),
                    "LENS",
                    "case {case:?}: the conflict must name the resource type, got {second:?}"
                );
            }

            // Either way exactly ONE version was published, still carrying the flag the
            // original publish recorded: a replay publishes nothing and a refusal writes
            // nothing.
            assert_eq!(
                published_versions(&client, &arn),
                published,
                "case {case:?}: one token must publish exactly one version, unchanged"
            );
        }
    }
}

/// One row of rules 2 and 3: a `CreateWorkload` member the create defaults, the default
/// value it records for an absent member, and — when one exists for that member — a value
/// that is NOT that default.
struct DefaultedMember {
    case: &'static str,
    member: &'static str,
    default_value: Value,
    different: Option<Value>,
}

/// The members `CreateWorkload` defaults, with the value each one records when the request
/// omits it. The lists default to empty (`CreateWorkload` resolves each absent list to no
/// entries); `PillarPriorities` defaults to the framework's own pillar order, read from the
/// one place that order lives (`state::default_pillar_priorities`), so this table stays
/// right however the framework's pillars are revised. `Tags` is not a row: its default is no
/// tags and `TagMap` is `@length(min: 1)` (`smithy_query(query="shape", shape="TagMap")`),
/// so an empty map cannot be spelled at all — the boundary refuses it before the handler.
fn defaulted_members() -> Vec<DefaultedMember> {
    vec![
        DefaultedMember {
            case: "account-ids",
            member: "AccountIds",
            default_value: json!([]),
            different: Some(json!(["123456789012"])),
        },
        DefaultedMember {
            case: "applications",
            member: "Applications",
            default_value: json!([]),
            different: Some(json!([
                "arn:aws:servicecatalog:us-west-2:123456789012:/applications/abcdefghijklmnopqrstuvwxy"
            ])),
        },
        DefaultedMember {
            case: "non-aws-regions",
            member: "NonAwsRegions",
            default_value: json!([]),
            different: Some(json!(["on-premises-dc1"])),
        },
        DefaultedMember {
            case: "profile-arns",
            member: "ProfileArns",
            default_value: json!([]),
            // A non-empty value has to name a STORED profile, which this table does not
            // mint; the conflict side of the rule is pinned by the rows that can.
            different: None,
        },
        DefaultedMember {
            case: "review-template-arns",
            member: "ReviewTemplateArns",
            default_value: json!([]),
            // A non-empty value has to name a stored review template, for the reason above.
            different: None,
        },
        DefaultedMember {
            case: "pillar-priorities-empty",
            member: "PillarPriorities",
            default_value: json!([]),
            different: Some(json!(state::default_pillar_priorities()
                .into_iter()
                .rev()
                .collect::<Vec<String>>())),
        },
        DefaultedMember {
            case: "pillar-priorities-framework-order",
            member: "PillarPriorities",
            default_value: json!(state::default_pillar_priorities()),
            different: None,
        },
    ]
}

/// Rule 2. A retry that spells a defaulted member at the value the create recorded for its
/// absence is the same request and replays the workload the token created.
#[tokio::test]
async fn a_create_workload_retry_spelling_a_defaulted_member_replays_the_workload() {
    for row in defaulted_members() {
        let case = row.case;
        let client = Client::new();
        let name = format!("wl-{case}");
        let token = format!("tok-{case}");

        // The original request omits the member entirely.
        let created = client.create(minimal_create(&name, &token)).await;
        let original = created_id(&created, case);

        let mut retried = minimal_create(&name, &token);
        retried[row.member] = row.default_value.clone();
        let replay = client.create(retried).await;
        assert_eq!(
            replay.status, 200,
            "case {case:?}: a retry spelling {} as {:?} must replay, got {replay:?}",
            row.member, row.default_value
        );
        assert_eq!(
            member(&replay, "WorkloadId"),
            original,
            "case {case:?}: expected the original WorkloadId {original:?}, got {replay:?}"
        );
        assert_eq!(
            stored_workload_count(&client),
            1,
            "case {case:?}: one token must have created exactly one workload, got {}",
            stored_workload_count(&client)
        );
    }
}

/// Rule 3. Resolving a defaulted member does not flatten the comparison: a value other than
/// the one the original create recorded is still another request.
#[tokio::test]
async fn a_create_workload_retry_with_a_non_default_value_is_still_a_conflict() {
    for row in defaulted_members() {
        let Some(different) = row.different.clone() else {
            continue;
        };
        let case = row.case;
        assert_ne!(
            different, row.default_value,
            "case {case:?}: the row's non-default value must differ from the default, got \
             {different:?}"
        );

        let client = Client::new();
        let name = format!("wl-diff-{case}");
        let token = format!("tok-diff-{case}");
        let created = client.create(minimal_create(&name, &token)).await;
        let original = created_id(&created, case);

        let mut retried = minimal_create(&name, &token);
        retried[row.member] = different.clone();
        let refused = client.create(retried).await;
        assert_token_conflict(case, &refused, &token);
        assert_eq!(
            member(&refused, "ResourceId"),
            original,
            "case {case:?}: the conflict must name the workload the token created, got {refused:?}"
        );
        assert_eq!(
            stored_workload_count(&client),
            1,
            "case {case:?}: the refusal must have created no second workload, got {}",
            stored_workload_count(&client)
        );
    }
}
