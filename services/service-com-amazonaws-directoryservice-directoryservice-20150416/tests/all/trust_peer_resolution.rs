// SPDX-License-Identifier: CC0-1.0

//! Which remote domain a trust can be ESTABLISHED with: the peer side of the relationship,
//! resolved against the directories this service holds.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`crate::harness`), so nothing here reaches into
//! the state struct and an answer that is not computed from persisted state cannot pass.
//!
//! The rule under test: the Amazon Web Services side of a trust is established only against
//! a peer the remote domain actually has, so `CreateTrust` settles `Creating` -> `Created`
//! when a peer answers for the domain it names — a domain THIS service serves, which is the
//! domain of a directory it stores, or an external name the global DNS can hold — and
//! `Creating` -> `Failed`, with the reason that outcome carries, when none does. The sibling
//! suite `tests/all/trust_outcomes.rs` asserts that rule over EXTERNAL names; this one
//! asserts the half the directories this service holds decide, which is what a trust between
//! two of its own directories depends on.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{PAST_ONE_SPAN, PAST_ONE_SPAN_ONLY};
use crate::harness::{ok, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// A `TrustPassword` the modelled pattern `^(\p{LD}|\p{Punct}| )+$` admits, within its
/// `length 1..128`.
const TRUST_PASSWORD: &str = "Tru5t-P@ssword 1";

/// One value of the modelled `TrustDirection` catalog, which every row shares: the outcome
/// under test is the remote domain's, not the direction's.
const TRUST_DIRECTION: &str = "Two-Way";

/// The `TrustState` values of the modelled enum this suite names
/// (`smithy_query(query="shape", shape="TrustState")`).
const TRUST_STATE_CREATING: &str = "Creating";
const TRUST_STATE_CREATED: &str = "Created";
const TRUST_STATE_FAILED: &str = "Failed";

/// The domain of the peer directory sitting under a top-level domain the IETF reserves as
/// never installed in the global DNS (RFC 6761's `test`), so no external peer could answer
/// for it: this service serves it because it holds the directory.
const RESERVED_PEER_DOMAIN: &str = "peer.sim2real.test";

/// The domain of the peer directory sitting under an ordinary top-level domain, so the row
/// that tells the two peers apart is the name rather than the peer's presence.
const ORDINARY_PEER_DOMAIN: &str = "peer.sim2real.net";

/// A Managed Microsoft AD directory, the only type a trust is supported for. The stage chain
/// its create owes is NOT waited out here: a caller that creates several directories starts
/// every chain and then waits once.
async fn directory(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    create_microsoft_ad(router, minimal_microsoft_ad(name), case).await
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

/// The state `trust_id` is in, as `DescribeTrusts` reports it.
async fn trust_state(router: &MultiServiceRouter, trust_id: &str, case: &str) -> String {
    let reported = describe_one(router, trust_id, case).await;
    reported["TrustState"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a TrustState, got {reported}"))
        .to_string()
}

/// RULE: a trust settles on `Created` when a peer answers for its remote domain and on
/// `Failed` otherwise — and a domain this service itself serves HAS a peer, whatever its
/// name looks like.
#[tokio::test]
async fn a_trust_settles_on_the_outcome_its_peer_allows() {
    let router = service();

    // (case, the remote domain the trust points at, whether a peer answers for it)
    //
    // The rows differ along what decides the outcome: whether this service serves the
    // domain, whether the name is one the global DNS can hold, and the spellings one DNS
    // domain has — mixed case and the fully qualified form with its root dot, both of which
    // name the same domain as the plain one.
    let rows: [(&str, String, bool); 7] = [
        (
            "a served domain under a never-installed top-level domain",
            RESERVED_PEER_DOMAIN.to_string(),
            true,
        ),
        (
            "the same served domain in mixed case",
            "Peer.SIM2REAL.Test".to_string(),
            true,
        ),
        (
            "the same served domain fully qualified",
            format!("{RESERVED_PEER_DOMAIN}."),
            true,
        ),
        (
            "a served domain under an ordinary top-level domain",
            ORDINARY_PEER_DOMAIN.to_string(),
            true,
        ),
        (
            "an unserved domain under a never-installed top-level domain",
            "absent.sim2real.test".to_string(),
            false,
        ),
        (
            "an unserved domain under another never-installed top-level domain",
            "absent.sim2real.invalid".to_string(),
            false,
        ),
        (
            "an unserved domain under an ordinary top-level domain",
            "remote.sim2real.net".to_string(),
            true,
        ),
    ];

    // The peers: two directories this service holds, so their own domains are domains it
    // serves.
    directory(&router, RESERVED_PEER_DOMAIN, "the reserved-name peer").await;
    directory(&router, ORDINARY_PEER_DOMAIN, "the ordinary-name peer").await;
    // One holder per row: a directory is one side of at most one trust with a given domain,
    // and three rows are spellings of the one domain.
    let mut holders: Vec<String> = Vec::new();
    for (index, (case, _, _)) in rows.iter().enumerate() {
        holders.push(directory(&router, &format!("holder{index}.sim2real.net"), case).await);
    }
    // Every directory's stage chain started above, so one wait serves them all.
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let mut created: Vec<(&str, String, bool)> = Vec::new();
    for ((case, remote_domain_name, has_a_peer), holder) in rows.iter().zip(&holders) {
        let trust_id = create_trust(&router, holder, remote_domain_name, case).await;
        // The create "initiates the creation", so the relationship is still being
        // established when it answers — whatever outcome the clock will give it.
        let transient = describe_one(&router, &trust_id, case).await;
        assert_eq!(
            transient["TrustState"],
            json!(TRUST_STATE_CREATING),
            "case {case:?}: expected the transient state right after the create, got {transient}"
        );
        created.push((case, trust_id, *has_a_peer));
    }

    // The transition each create owed has fallen due — by the clock, not by the read.
    tokio::time::sleep(PAST_ONE_SPAN_ONLY).await;
    for (case, trust_id, has_a_peer) in &created {
        let expected = if *has_a_peer {
            TRUST_STATE_CREATED
        } else {
            TRUST_STATE_FAILED
        };
        let settled = describe_one(&router, trust_id, case).await;
        assert_eq!(
            settled["TrustState"],
            json!(expected),
            "case {case:?}: expected the trust to settle on {expected}, got {settled}"
        );
        if *has_a_peer {
            assert!(
                settled["TrustStateReason"].is_null(),
                "case {case:?}: expected no reason for a state the relationship reached as \
                 asked, got {settled}"
            );
        } else {
            let reason = settled["TrustStateReason"].as_str().unwrap_or_default();
            assert!(
                !reason.is_empty(),
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

/// RULE (the same one, read the other way): the peer is resolved against the directories
/// this service HOLDS, so a domain it stops serving stops having a peer — the outcome is
/// computed from stored state rather than from the shape of the name the request sent.
#[tokio::test]
async fn a_domain_this_service_stops_serving_stops_having_a_peer() {
    let router = service();
    let peer_domain = "departing.sim2real.test";
    let peer = directory(&router, peer_domain, "the departing peer").await;
    let first_holder = directory(&router, "first.sim2real.net", "the first holder").await;
    let second_holder = directory(&router, "second.sim2real.net", "the second holder").await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // While the peer is held, a trust to its domain establishes.
    let before = create_trust(
        &router,
        &first_holder,
        peer_domain,
        "while the peer is held",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN_ONLY).await;
    let before_state = trust_state(&router, &before, "while the peer is held").await;
    assert_eq!(
        before_state, TRUST_STATE_CREATED,
        "case \"while the peer is held\": expected the trust to establish, got {before_state}"
    );

    // The peer leaves the service, so nothing answers for that domain any more.
    ok(
        &router,
        "DeleteDirectory",
        json!({ "DirectoryId": &peer }),
        "the peer is deleted",
    )
    .await;

    let after = create_trust(
        &router,
        &second_holder,
        peer_domain,
        "once the peer is gone",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN_ONLY).await;
    let after_state = trust_state(&router, &after, "once the peer is gone").await;
    assert_eq!(
        after_state, TRUST_STATE_FAILED,
        "case \"once the peer is gone\": expected a trust to a domain nothing serves to fail, \
         got {after_state}"
    );

    // The relationship that DID establish is untouched by the peer's departure: the peer is
    // resolved when the create's transition falls due, and a settled state is not revisited.
    let still = trust_state(&router, &before, "the established trust").await;
    assert_eq!(
        still, TRUST_STATE_CREATED,
        "case \"the established trust\": expected the settled state to stand, got {still}"
    );
}
