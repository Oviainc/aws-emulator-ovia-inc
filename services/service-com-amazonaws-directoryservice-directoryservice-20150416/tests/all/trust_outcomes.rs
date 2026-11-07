// SPDX-License-Identifier: CC0-1.0

//! What a trust relationship SETTLES on: the outcome the clock gives a `CreateTrust`, and
//! what a caller can do with a relationship that did not establish.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`crate::harness`), so nothing here reaches into
//! the state struct and an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test:
//!
//! * the Amazon Web Services side of a trust can only be established against a peer the
//!   remote domain has, so a create naming a domain the global DNS cannot resolve is
//!   ACCEPTED and then driven `Creating` -> `Failed` with a non-empty `TrustStateReason`,
//!   while a create naming a resolvable domain settles on `Created` — asserted over a
//!   table of remote domain names spanning the reserved, never-resolvable top-level
//!   domains and the ordinary ones, in both spellings a domain name has (mixed case, and
//!   the fully qualified form with its root dot);
//! * a verification can only be started on a relationship there is something to verify
//!   about, so `VerifyTrust` from a state the relationship did not establish in — or from
//!   its teardown — is refused with `InvalidParameterException` naming that state;
//! * the teardown `DeleteTrust` starts settles on the modelled `Deleted` state, which a
//!   caller can read, before the relationship leaves the service altogether.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::PAST_ONE_SPAN;
use crate::harness::{call, ok, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// A `TrustPassword` the modelled pattern `^(\p{LD}|\p{Punct}| )+$` admits, within its
/// `length 1..128`.
const TRUST_PASSWORD: &str = "Tru5t-P@ssword 1";

/// One value of the modelled `TrustDirection` catalog, which every row shares: the outcome
/// under test is the domain's, not the direction's.
const TRUST_DIRECTION: &str = "Two-Way";

/// The `TrustState` values of the modelled enum this suite names.
const TRUST_STATE_CREATING: &str = "Creating";
const TRUST_STATE_CREATED: &str = "Created";
const TRUST_STATE_FAILED: &str = "Failed";
const TRUST_STATE_VERIFYING: &str = "Verifying";
const TRUST_STATE_DELETING: &str = "Deleting";
const TRUST_STATE_DELETED: &str = "Deleted";

/// The `TrustStateReason` a trust whose creation failed carries
/// (`state::TRUST_CREATE_FAILURE_REASON`), as the recorded body for that outcome words it.
const TRUST_CREATE_FAILURE_REASON: &str =
    "An internal service error has been encountered while creating the trust. Please retry \
     the operation.";

/// The suffix `crate::state::refusal` appends to every message of this service.
const REQUEST_ID_SEPARATOR: &str = " : RequestId: ";

/// This emulator's documented per-directory trust guard
/// (`state::MAX_TRUSTS_PER_DIRECTORY`), which bounds what the trust store holds for one
/// directory.
const TRUST_GUARD: usize = 50;

/// A Managed Microsoft AD directory in service, the only type a trust is supported for.
async fn directory(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let id = create_microsoft_ad(router, minimal_microsoft_ad(name), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    id
}

/// `CreateTrust` for `remote_domain_name`, asserting it was ACCEPTED — the outcome is the
/// clock's, not the create's — and returning the `TrustId`.
async fn create_trust(
    router: &MultiServiceRouter,
    directory_id: &str,
    remote_domain_name: &str,
    case: &str,
) -> String {
    let answer = ok(
        router,
        "CreateTrust",
        json!({
            "DirectoryId": directory_id,
            "RemoteDomainName": remote_domain_name,
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION,
        }),
        case,
    )
    .await;
    string(&answer, "TrustId", case)
}

/// The one `Trust` `DescribeTrusts` reports for `trust_id`.
async fn describe_one(router: &MultiServiceRouter, trust_id: &str, case: &str) -> Value {
    let answer = ok(
        router,
        "DescribeTrusts",
        json!({ "TrustIds": [trust_id] }),
        case,
    )
    .await;
    let trusts = answer["Trusts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Trusts, got {answer}"));
    assert_eq!(
        trusts.len(),
        1,
        "case {case:?}: expected exactly one trust for {trust_id}, got {trusts:?}"
    );
    trusts[0].clone()
}

/// RULE: a trust settles on `Created` when its remote domain can be resolved and on
/// `Failed`, with the reason that outcome carries, when it cannot — and either way the
/// create is accepted and answers while the relationship is still being established.
#[tokio::test]
async fn a_trust_settles_on_the_outcome_its_remote_domain_allows() {
    let router = service();
    let directory_id = directory(&router, "corp.example.com", "the outcome").await;

    // The rows differ along the remote domain the trust points at: the top-level domains
    // the IETF reserves as names that are never installed in the global DNS (RFC 6761's
    // `invalid`, `test` and `localhost`, and RFC 2606's `example`), which no peer can
    // answer for, against ordinary resolvable names — including `example.com`, a
    // DELEGATED name whose reservation is for documentation only — in the spellings a
    // domain name has: a label under the TLD, several labels, mixed case, and the fully
    // qualified form with its root dot.
    let rows: [(&str, &str, bool); 10] = [
        ("a reserved invalid domain", "peer.sim2real.invalid", false),
        ("a reserved test domain", "freshly-minted.test", false),
        ("a reserved localhost domain", "peer.localhost", false),
        ("a reserved example top-level domain", "corp.example", false),
        (
            "a reserved domain in mixed case",
            "Mixed.SIM2REAL.Invalid",
            false,
        ),
        (
            "a reserved domain fully qualified",
            "peer.other.test.",
            false,
        ),
        ("a delegated documentation domain", "ad.example.com", true),
        ("an ordinary domain", "corp.sim2real.net", true),
        ("an ordinary domain in mixed case", "AD2.Example.COM", true),
        (
            "an ordinary domain fully qualified",
            "rooted.example.org.",
            true,
        ),
    ];

    let mut created: Vec<(&str, String, bool)> = Vec::new();
    for (case, remote_domain_name, resolvable) in rows {
        let trust_id = create_trust(&router, &directory_id, remote_domain_name, case).await;
        // The create "initiates the creation", so the relationship is still being
        // established when it answers — whatever outcome the clock will give it.
        let transient = describe_one(&router, &trust_id, case).await;
        assert_eq!(
            transient["TrustState"],
            json!(TRUST_STATE_CREATING),
            "case {case:?}: expected the transient state right after the create, got \
             {transient}"
        );
        assert!(
            transient["TrustStateReason"].is_null(),
            "case {case:?}: expected no TrustStateReason while the create is under way, got \
             {transient}"
        );
        created.push((case, trust_id, resolvable));
    }

    // The transition the create owed has fallen due — by the clock, not by the read.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (case, trust_id, resolvable) in &created {
        let settled = describe_one(&router, trust_id, case).await;
        let expected = if *resolvable {
            TRUST_STATE_CREATED
        } else {
            TRUST_STATE_FAILED
        };
        assert_eq!(
            settled["TrustState"],
            json!(expected),
            "case {case:?}: expected the trust to settle on {expected}, got {settled}"
        );
        if *resolvable {
            assert!(
                settled["TrustStateReason"].is_null(),
                "case {case:?}: expected no reason for a state the relationship reached as \
                 asked, got {settled}"
            );
        } else {
            assert_eq!(
                settled["TrustStateReason"],
                json!(TRUST_CREATE_FAILURE_REASON),
                "case {case:?}: expected the failed trust to state why, got {settled}"
            );
        }

        // A read does not change what the next caller sees.
        let again = describe_one(&router, trust_id, case).await;
        assert_eq!(
            again, settled,
            "case {case:?}: expected a repeated read to report the same trust, got {again}"
        );
    }
}

/// RULE: `VerifyTrust` needs a relationship there is something to verify about, so it is
/// refused from a state the relationship did not establish in, naming that state, and the
/// refusal changes nothing.
#[tokio::test]
async fn a_verification_is_refused_from_a_state_a_trust_cannot_be_verified_from() {
    let router = service();
    let directory_id = directory(&router, "corp.example.com", "verification").await;

    let failed = create_trust(
        &router,
        &directory_id,
        "unreachable.sim2real.invalid",
        "a failed trust",
    )
    .await;
    let torn_down = create_trust(
        &router,
        &directory_id,
        "teardown.example.com",
        "a trust being torn down",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "DeleteTrust",
        json!({ "TrustId": &torn_down }),
        "a trust being torn down",
    )
    .await;

    // The rows are the states a relationship cannot be verified from: the failure its
    // creation settled on, and the teardown a delete started.
    for (case, trust_id, state) in [
        ("a failed trust", &failed, TRUST_STATE_FAILED),
        ("a trust being torn down", &torn_down, TRUST_STATE_DELETING),
    ] {
        let before = describe_one(&router, trust_id, case).await;
        assert_eq!(
            before["TrustState"],
            json!(state),
            "case {case:?}: expected the trust to be in {state} before the verification, got \
             {before}"
        );

        let (status, parsed) = call(&router, "VerifyTrust", json!({ "TrustId": trust_id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the verification to be refused, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {parsed}"
        );
        let message = parsed["Message"].as_str().unwrap_or_default();
        let (text, _) = message
            .split_once(REQUEST_ID_SEPARATOR)
            .unwrap_or((message, ""));
        assert_eq!(
            text,
            format!("Cannot verify trust in state: {state}"),
            "case {case:?}: expected the refusal to name the state it refused from, got \
             {message:?}"
        );

        // The refusal changed nothing about the relationship.
        let after = describe_one(&router, trust_id, case).await;
        assert_eq!(
            after["TrustState"], before["TrustState"],
            "case {case:?}: expected the refused verification to leave the state alone, got \
             {after}"
        );
    }

    // A relationship that DID establish is verifiable, which is what makes the refusals
    // above about the state rather than about the operation.
    let established = create_trust(
        &router,
        &directory_id,
        "verifiable.example.com",
        "an established trust",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "VerifyTrust",
        json!({ "TrustId": &established }),
        "an established trust",
    )
    .await;
    let verifying = describe_one(&router, &established, "an established trust").await;
    assert_eq!(
        verifying["TrustState"],
        json!(TRUST_STATE_VERIFYING),
        "case \"an established trust\": expected the verification to start, got {verifying}"
    );
}

/// RULE: the teardown a delete starts settles on the modelled `Deleted` state — which a
/// caller can still read — before the relationship leaves the service.
#[tokio::test]
async fn a_torn_down_trust_settles_on_deleted_before_it_leaves_the_service() {
    let router = service();
    let directory_id = directory(&router, "corp.example.com", "the teardown").await;
    let trust_id = create_trust(
        &router,
        &directory_id,
        "teardown.example.com",
        "the teardown",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "DeleteTrust",
        json!({ "TrustId": &trust_id }),
        "the teardown",
    )
    .await;

    // (case, how long to wait first, the state the relationship is then in)
    for (case, state) in [
        ("right after the delete", TRUST_STATE_DELETING),
        ("once the teardown fell due", TRUST_STATE_DELETED),
    ] {
        let reported = describe_one(&router, &trust_id, case).await;
        assert_eq!(
            reported["TrustState"],
            json!(state),
            "case {case:?}: expected {state}, got {reported}"
        );
        tokio::time::sleep(PAST_ONE_SPAN).await;
    }

    // One span past `Deleted` the relationship is gone from this service: resolving it by
    // id reports that it does not exist, and the directory can trust that domain again.
    let (status, parsed) = call(&router, "DescribeTrusts", json!({ "TrustIds": [&trust_id] })).await;
    assert_eq!(
        status, 400,
        "case \"past the teardown\": expected the trust to be gone, got {status} with {parsed}"
    );
    let message = parsed["Message"].as_str().unwrap_or_default();
    let (text, _) = message
        .split_once(REQUEST_ID_SEPARATOR)
        .unwrap_or((message, ""));
    assert_eq!(
        text,
        format!("Trust: {trust_id} does not exist"),
        "case \"past the teardown\": expected the not-found sentence for the id the request \
         named, got {message:?}"
    );
    create_trust(
        &router,
        &directory_id,
        "teardown.example.com",
        "the domain again",
    )
    .await;
}

/// RULE: a relationship that FAILED stays on record until the directory is deleted, so it
/// counts against the per-directory trust guard — a caller naming one non-resolvable
/// domain after another cannot grow the trust store past that bound.
#[tokio::test]
async fn failed_trusts_count_against_the_documented_per_directory_guard() {
    let router = service();
    let directory_id = directory(&router, "corp.example.com", "the guard").await;

    // Every create names a domain that cannot be resolved, so every one of them settles
    // on `Failed` — and the guard still fills up.
    for index in 0..TRUST_GUARD {
        create_trust(
            &router,
            &directory_id,
            &format!("guard{index}.invalid"),
            "at the guard",
        )
        .await;
    }
    // One span past the last create, every relationship has failed.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let settled = ok(
        &router,
        "DescribeTrusts",
        json!({ "DirectoryId": &directory_id }),
        "at the guard",
    )
    .await;
    let failed = settled["Trusts"]
        .as_array()
        .map(|trusts| {
            trusts
                .iter()
                .filter(|trust| trust["TrustState"] == json!(TRUST_STATE_FAILED))
                .count()
        })
        .unwrap_or_default();
    assert_eq!(
        failed, TRUST_GUARD,
        "case \"at the guard\": expected every create to have settled on {TRUST_STATE_FAILED}, \
         got {failed} of {TRUST_GUARD}"
    );

    // One past the guard is refused, whatever state the records it counted are in, and
    // stores nothing.
    let (status, parsed) = call(
        &router,
        "CreateTrust",
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": "onepast.invalid",
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": TRUST_DIRECTION,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the guard\": expected the create to be refused, got {status} with \
         {parsed}"
    );
    let after = ok(
        &router,
        "DescribeTrusts",
        json!({ "DirectoryId": &directory_id }),
        "one past the guard",
    )
    .await;
    assert_eq!(
        after["Trusts"].as_array().map(Vec::len).unwrap_or_default(),
        TRUST_GUARD,
        "case \"one past the guard\": expected the refused create to store nothing, got {after}"
    );
}
