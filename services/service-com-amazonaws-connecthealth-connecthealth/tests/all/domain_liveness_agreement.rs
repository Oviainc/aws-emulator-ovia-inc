// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ONE domain-liveness rule every path that needs a
//! live domain goes through.
//!
//! The RULE under test, asserted over a table rather than one recorded request: a domain
//! is LIVE exactly while the record it holds reports `ACTIVE`, and every path whose
//! contract needs a live domain answers from that single rule — so the domain's own
//! readers and writers (`ListSubscriptions`, `CreateSubscription`), the readers and
//! writers of what it contains (`GetSubscription`, `ActivateSubscription`), the
//! ARN-addressed tagging operations that resolve a subscription (`ListTagsForResource`,
//! `TagResource`) and the idempotent-repeat resolution of `CreateDomain` either all treat
//! the domain as live or all treat it as gone, at the same instant. The domain's OWN
//! record outlives its liveness, so its ARN keeps resolving for as long as the service
//! holds the record — the one answer that is deliberately not governed by this rule.
//!
//! The rows differ along what the rule generalises over: whether the domain's deletion
//! completes inside the `DeleteDomain` call (a domain with no web application, whose
//! record is purged at once) or passes through the non-terminal `DELETING` (a domain
//! owning a web application, torn down outside this service), how many subscriptions the
//! domain holds, whether the create the repeat is compared against carried tags, and the
//! region every request is signed for.
//!
//! Sources for every bound and catalog written by hand here:
//!
//! * `smithy_query(query="shape",shape="DomainStatus",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `ACTIVE | DELETING | DELETED`, so `DELETING` is a state of its own and a domain in
//!   it is no longer `ACTIVE`.
//! * `smithy_query(query="operation",operation="ListSubscriptions,CreateSubscription,GetSubscription,ActivateSubscription,CreateDomain,ListTagsForResource,TagResource",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `GET /domains/{domainId}/subscriptions` and `GET
//!   /domains/{domainId}/subscriptions/{subscriptionId}` answering 200, `POST
//!   /domains/{domainId}/subscriptions` answering 201, `POST
//!   /domains/{domainId}/subscriptions/{subscriptionId}/activate` answering 200, each with
//!   the modelled `ResourceNotFoundException` (404), `POST /domain` answering 201 and
//!   `@idempotent`, `GET /tags/{resourceArn}` answering 200 and `POST /tags/{resourceArn}`
//!   answering 204.

use serde_json::{json, Value};

use crate::arn_resolution_during_domain_deletion::web_app_setup;
use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::resource_tagging::tags_path;

/// What every path that needs a LIVE domain answered at one instant.
///
/// Each field is "did this path treat the domain as live?", so the whole observation is
/// uniform and the rule is asserted by comparing the fields with one another.
#[derive(Debug, PartialEq, Eq)]
struct Liveness {
    /// `ListSubscriptions` resolved the domain for reading, which needs the domain alone.
    domain_reader: bool,
    /// `CreateSubscription` resolved the domain for writing, which needs the domain alone.
    domain_writer: bool,
    /// `GetSubscription` resolved a subscription under the domain.
    subscription_reader: bool,
    /// `ActivateSubscription` resolved the same subscription for writing.
    subscription_writer: bool,
    /// `ListTagsForResource` resolved the subscription's ARN.
    arn_reader: bool,
    /// `TagResource` resolved the subscription's ARN.
    arn_writer: bool,
    /// A byte-identical repeat of the `CreateDomain` request resolved to the domain the
    /// first call created, rather than minting a second domain.
    create_repeat: bool,
}

impl Liveness {
    /// The ONE answer every field must carry, or `None` when they disagree — which is
    /// exactly the disagreement this suite exists to catch.
    fn agreed(&self) -> Option<bool> {
        let fields = [
            self.domain_reader,
            self.domain_writer,
            self.subscription_reader,
            self.subscription_writer,
            self.arn_reader,
            self.arn_writer,
            self.create_repeat,
        ];
        fields
            .iter()
            .all(|live| *live == fields[0])
            .then_some(fields[0])
    }
}

/// ONE probe: the operation being driven, how it is addressed, and the status its model
/// declares for success.
struct Probe<'a> {
    what: &'a str,
    method: &'a str,
    path: &'a str,
    body: Option<&'a Value>,
    success: u16,
}

/// Dispatch one probe and report whether the addressed resource RESOLVED: `true` when
/// the operation answered its modelled success, `false` when it answered the service's
/// `ResourceNotFoundException`. Any other answer fails the case.
async fn resolved(emulator: &Emulator, case: &str, phase: &str, probe: Probe<'_>) -> bool {
    let Probe {
        what,
        method,
        path,
        body,
        success,
    } = probe;
    let (status, response) = emulator.send(method, path, body).await;
    if status == success {
        return true;
    }
    assert_eq!(
        (status, error_code(&response).as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?} at {phase:?}: {what} must answer {success} or the service's 404, \
         got {status} with {response}"
    );
    false
}

/// Observe every path that needs a live domain, once, and assert they AGREE.
///
/// Returns the one answer they agreed on.
async fn liveness(
    emulator: &Emulator,
    case: &str,
    phase: &str,
    domain_id: &str,
    create_request: &Value,
    subscriptions: &[(String, String)],
) -> bool {
    let empty_body = json!({});
    let mut observed: Option<Liveness> = None;
    let mut answer: Option<bool> = None;
    for (index, (subscription_id, subscription_arn)) in subscriptions.iter().enumerate() {
        let subscription_path = format!("/domains/{domain_id}/subscriptions/{subscription_id}");
        let activate_path = format!("{subscription_path}/activate");
        let domain_collection = format!("/domains/{domain_id}/subscriptions");
        let subscription_tags = tags_path(subscription_arn);
        let phase_tag = json!({ "tags": { "phase": phase } });
        // The two paths whose contract needs the DOMAIN and nothing under it, so the
        // domain's own liveness is what they answer from.
        let domain_reader = resolved(
            emulator,
            case,
            phase,
            Probe {
                what: "ListSubscriptions",
                method: "GET",
                path: &domain_collection,
                body: None,
                success: 200,
            },
        )
        .await;
        let domain_writer = resolved(
            emulator,
            case,
            phase,
            Probe {
                what: "CreateSubscription",
                method: "POST",
                path: &domain_collection,
                body: Some(&empty_body),
                success: 201,
            },
        )
        .await;
        let subscription_reader = resolved(
            emulator,
            case,
            phase,
            Probe {
                what: "GetSubscription",
                method: "GET",
                path: &subscription_path,
                body: None,
                success: 200,
            },
        )
        .await;
        let subscription_writer = resolved(
            emulator,
            case,
            phase,
            Probe {
                what: "ActivateSubscription",
                method: "POST",
                path: &activate_path,
                body: Some(&empty_body),
                success: 200,
            },
        )
        .await;
        let arn_reader = resolved(
            emulator,
            case,
            phase,
            Probe {
                what: "ListTagsForResource on the subscription ARN",
                method: "GET",
                path: &subscription_tags,
                body: None,
                success: 200,
            },
        )
        .await;
        let arn_writer = resolved(
            emulator,
            case,
            phase,
            Probe {
                what: "TagResource on the subscription ARN",
                method: "POST",
                path: &subscription_tags,
                body: Some(&phase_tag),
                success: 204,
            },
        )
        .await;
        // The repeat is observed beside them, so the whole observation is taken at one
        // instant: a repeat that resolves answers the domain the first create made, one
        // that does not mints another domain of its own.
        let (status, repeated) = emulator.send("POST", "/domain", Some(create_request)).await;
        assert_eq!(
            status, 201,
            "case {case:?} at {phase:?}: CreateDomain is always served, got {status} \
             with {repeated}"
        );
        let create_repeat = string_at(&repeated, "domainId") == domain_id;

        let here = Liveness {
            domain_reader,
            domain_writer,
            subscription_reader,
            subscription_writer,
            arn_reader,
            arn_writer,
            create_repeat,
        };
        let agreed = here.agreed().unwrap_or_else(|| {
            panic!(
                "case {case:?} at {phase:?} #{index}: every path that needs a live domain \
                 must answer from ONE liveness rule, got {here:?}"
            )
        });
        if let Some(previous) = &observed {
            assert_eq!(
                previous, &here,
                "case {case:?} at {phase:?} #{index}: the rule is the domain's, so it \
                 cannot depend on which subscription is addressed, got {here:?} against \
                 {previous:?}"
            );
        }
        observed = Some(here);
        answer = Some(agreed);
    }
    answer.unwrap_or_else(|| {
        panic!("case {case:?} at {phase:?}: the domain must hold at least one subscription")
    })
}

#[tokio::test]
async fn every_path_that_needs_a_live_domain_answers_from_one_rule() {
    // (case, does the domain own a web application?, how many subscriptions, the tags the
    //  create carries, region)
    let rows: Vec<(&str, bool, usize, Value, &str)> = vec![
        (
            "a web-app domain with one subscription",
            true,
            1,
            json!({ "env": "prod" }),
            "us-east-1",
        ),
        (
            "a web-app domain with three subscriptions and an untagged create",
            true,
            3,
            json!({}),
            "eu-central-1",
        ),
        (
            "a domain with no web application, deleted inside the call",
            false,
            2,
            json!({ "env": "staging", "team": "cardiology" }),
            "ap-south-1",
        ),
    ];

    for (case, owns_web_app, subscription_count, tags, region) in &rows {
        let emulator = Emulator::new(region);
        let mut create_request = json!({ "name": "clinic", "tags": tags });
        if *owns_web_app {
            create_request["webAppSetupConfiguration"] = web_app_setup();
        }
        let created = emulator.create_domain(&create_request).await;
        let domain_id = string_at(&created, "domainId");
        let domain_arn = string_at(&created, "arn");

        let mut subscriptions: Vec<(String, String)> = Vec::new();
        for index in 0..*subscription_count {
            let (status, subscription) = emulator
                .send(
                    "POST",
                    &format!("/domains/{domain_id}/subscriptions"),
                    Some(&json!({})),
                )
                .await;
            assert_eq!(
                status, 201,
                "case {case:?} #{index}: CreateSubscription got {status} with {subscription}"
            );
            subscriptions.push((
                string_at(&subscription, "subscriptionId"),
                string_at(&subscription, "arn"),
            ));
        }

        // While the domain reports `ACTIVE` every path treats it as live.
        assert!(
            liveness(
                &emulator,
                case,
                "created",
                &domain_id,
                &create_request,
                &subscriptions,
            )
            .await,
            "case {case:?}: an ACTIVE domain is live for every path"
        );

        let (status, deleted) = emulator
            .send("DELETE", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteDomain got {status} with {deleted}"
        );
        // Which status the delete reports follows from what the domain owns: only the
        // teardown that outlives the call passes through the non-terminal state.
        let expected_status = if *owns_web_app { "DELETING" } else { "DELETED" };
        assert_eq!(
            string_at(&deleted, "status"),
            expected_status,
            "case {case:?}: DeleteDomain must report {expected_status}, got {deleted}"
        );

        // Once the domain has left `ACTIVE` every one of those paths treats it as gone —
        // including the ARN-addressed ones, which is what the tagging operations resolve a
        // subscription through.
        assert!(
            !liveness(
                &emulator,
                case,
                "after DeleteDomain",
                &domain_id,
                &create_request,
                &subscriptions,
            )
            .await,
            "case {case:?}: a domain that has left ACTIVE is live for no path"
        );

        // The contrast the rule does NOT cover: the domain's own ARN is addressable for as
        // long as the service HOLDS the record, whatever status that record reports, so a
        // domain still held in `DELETING` answers its own ARN reader.
        let (status, listed) = emulator.send("GET", &tags_path(&domain_arn), None).await;
        if *owns_web_app {
            assert_eq!(
                status, 200,
                "case {case:?}: the domain's record is still held, so its ARN resolves, \
                 got {status} with {listed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&listed).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: a record purged inside the deleting call resolves nowhere, \
                 got {status} with {listed}"
            );
        }
    }
}
