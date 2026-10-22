// SPDX-License-Identifier: CC0-1.0

//! The console-authorization configuration's whole lifecycle, end to end over HTTP:
//! `PutConsoleAuthorizationConfiguration` enables it for a target,
//! `GetConsoleAuthorizationConfiguration` reports what the put persisted, and
//! `DeleteConsoleAuthorizationConfiguration` releases it — reporting that target with
//! `consoleAuthorizationEnabled` false, after which the target is missing again.
//!
//! The rule under test is the one the three operations state together: the
//! configuration of a target is exactly what the writes left there, so a delete reports
//! the target it released as no longer enabled and every later read of that target —
//! another get, another delete — answers `ResourceNotFoundException`. The rows differ
//! along what the rule generalises over: which alternative of `TargetId`'s pattern the
//! target matches (account id, organization id, organizational root id, at both
//! published length bounds), and whether the request names a target at all.

use serde_json::{json, Value};

use crate::signin_harness::{post, Answer, EMULATOR_ACCOUNT};

/// The scope each form of `TargetId` implies, as `(case, targetId, scope)`.
///
/// Written from `TargetId`'s own pattern `^(\d{12}|o-[a-z0-9]{10}|r-[0-9a-z]{4,32})$` —
/// an account id, an AWS Organizations organization id and an organizational root id —
/// with a root id at each published bound of `TargetId`'s `@length(min: 12, max: 32)`,
/// which is the narrower of the two constraints on a root id.
fn target_cases() -> Vec<(&'static str, Value, String, &'static str)> {
    vec![
        (
            "a 12-digit account id",
            json!({"targetId": "123456789012"}),
            "123456789012".to_string(),
            "ACCOUNT",
        ),
        (
            "no targetId at all, which addresses the caller's own account",
            json!({}),
            EMULATOR_ACCOUNT.to_string(),
            "ACCOUNT",
        ),
        (
            "an organization id",
            json!({"targetId": "o-abc1234567"}),
            "o-abc1234567".to_string(),
            "ORGANIZATION",
        ),
        (
            "a root id at the 12-character length floor",
            json!({"targetId": "r-0123456789"}),
            "r-0123456789".to_string(),
            "ROOT",
        ),
        (
            "a root id at the 32-character length ceiling",
            json!({"targetId": format!("r-{}", "a".repeat(30))}),
            format!("r-{}", "a".repeat(30)),
            "ROOT",
        ),
    ]
}

async fn put_configuration(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: Value,
) -> Answer {
    post(router, "/put-console-authorization-configuration", request).await
}

async fn get_configuration(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: Value,
) -> Answer {
    post(router, "/get-console-authorization-configuration", request).await
}

async fn delete_configuration(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: Value,
) -> Answer {
    post(router, "/delete-console-authorization-configuration", request).await
}

/// The three members every one of the three operations reports, as a comparable triple.
fn reported(answer: &Answer) -> (String, String, Option<bool>) {
    (
        answer.body["targetId"].as_str().unwrap_or_default().to_string(),
        answer.body["scope"].as_str().unwrap_or_default().to_string(),
        answer.body["consoleAuthorizationEnabled"].as_bool(),
    )
}

#[tokio::test]
async fn the_configuration_of_a_target_is_what_the_writes_left_there() {
    let router = crate::signin_harness::emulator();

    for (case, request, target_id, scope) in target_cases() {
        // Nothing is configured for this target yet, so BOTH readers report it missing —
        // the delete included, which is what makes a cleanup path able to tell "there was
        // nothing to release" from "released".
        for (reader, answer) in [
            ("get", get_configuration(&router, request.clone()).await),
            ("delete", delete_configuration(&router, request.clone()).await),
        ] {
            assert_eq!(
                (answer.status, answer.error_code()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: an unconfigured target must answer 404 \
                 ResourceNotFoundException to the {reader}, got {} with {:?}",
                answer.status,
                answer.body
            );
        }

        let written = put_configuration(&router, request.clone()).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a put must be accepted, got {} with {:?}",
            written.status, written.body
        );
        assert_eq!(
            reported(&written),
            (target_id.clone(), scope.to_string(), Some(true)),
            "case {case:?}: the put must report the target it enabled, got {:?}",
            written.body
        );

        // A request of its own observes exactly what the put persisted.
        let read = get_configuration(&router, request.clone()).await;
        assert_eq!(
            (read.status, reported(&read)),
            (200, (target_id.clone(), scope.to_string(), Some(true))),
            "case {case:?}: the get must report what the put persisted, got {} with {:?}",
            read.status,
            read.body
        );

        // The delete reports the target it released, with console authorization no
        // longer enabled — the state it just left the target in.
        let released = delete_configuration(&router, request.clone()).await;
        assert_eq!(
            (released.status, reported(&released)),
            (200, (target_id.clone(), scope.to_string(), Some(false))),
            "case {case:?}: the delete must report the released target as no longer \
             enabled, got {} with {:?}",
            released.status,
            released.body
        );

        // And the release really happened: the target is missing again to every reader,
        // so a repeated delete is a 404 rather than a second success.
        for (reader, answer) in [
            ("get", get_configuration(&router, request.clone()).await),
            ("delete", delete_configuration(&router, request.clone()).await),
        ] {
            assert_eq!(
                (answer.status, answer.error_code()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: after the delete the {reader} must answer 404 \
                 ResourceNotFoundException, got {} with {:?}",
                answer.status,
                answer.body
            );
        }
    }
}

/// A delete releases ONE target: every other configured target is untouched, so the
/// delete cannot be answering from anything but the per-target records.
#[tokio::test]
async fn a_delete_releases_only_the_target_it_names() {
    let router = crate::signin_harness::emulator();
    let cases = target_cases();

    for (case, request, ..) in &cases {
        let written = put_configuration(&router, request.clone()).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a put must be accepted, got {} with {:?}",
            written.status, written.body
        );
    }

    for (index, (case, request, target_id, scope)) in cases.iter().enumerate() {
        let released = delete_configuration(&router, request.clone()).await;
        assert_eq!(
            (released.status, reported(&released)),
            (200, (target_id.clone(), (*scope).to_string(), Some(false))),
            "case {case:?}: the delete must release this target, got {} with {:?}",
            released.status,
            released.body
        );

        // Every target not yet deleted is still enabled; every one already deleted is
        // still gone.
        for (other, other_request, other_target, other_scope) in &cases[index + 1..] {
            let read = get_configuration(&router, other_request.clone()).await;
            assert_eq!(
                (read.status, reported(&read)),
                (
                    200,
                    (other_target.clone(), (*other_scope).to_string(), Some(true))
                ),
                "case {case:?}: deleting it must leave {other:?} enabled, got {} with {:?}",
                read.status,
                read.body
            );
        }
        for (earlier, earlier_request, ..) in &cases[..=index] {
            let read = get_configuration(&router, earlier_request.clone()).await;
            assert_eq!(
                (read.status, read.error_code()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: {earlier:?} was already released and must stay missing, \
                 got {} with {:?}",
                read.status,
                read.body
            );
        }
    }
}
