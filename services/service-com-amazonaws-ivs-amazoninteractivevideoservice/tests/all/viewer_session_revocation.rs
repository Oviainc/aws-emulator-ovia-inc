// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `BatchStartViewerSessionRevocation`, driven over the wire.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with
//! a restJson1 request, exactly as a signed `aws ivs` call arrives — so what it
//! asserts is the emulator's HTTP behaviour.
//!
//! The rules under test, each exercised over a SET of inputs rather than one request:
//!
//! 1. The call performs `StartViewerSessionRevocation` for EVERY pair it carries and
//!    succeeds with HTTP 200 whatever the mix of pairs: a pair naming a channel the
//!    store holds produces no error entry, and a pair naming a channel it does not
//!    hold produces exactly one entry, in request order, naming that pair's
//!    `channelArn` and `viewerId` with the code the single-pair operation models for
//!    a channel that does not exist (`ResourceNotFoundException`).
//! 2. A started revocation is PERSISTED per `(channelArn, viewerId)` pair, and its
//!    version threshold is the maximum over the calls: "All versions less than or
//!    equal to the specified version will be revoked" (the model doc on
//!    `viewerSessionVersionsLessThanOrEqualTo`), so a later, lower version cannot
//!    un-revoke what an earlier call revoked, and a repeated call changes nothing.
//! 3. A request outside a modelled constraint is refused with a 400
//!    `ValidationException` and records nothing — the list is `len 1..20`, each
//!    pair's `channelArn` carries the `ChannelArn` pattern, `viewerId` is `len 1..40`
//!    and the version's range is `0..`.
//! 4. Revocation is tracked per pair, so distinct viewers and distinct channels never
//!    share a threshold, and nothing about a channel's own record changes.
//! 5. Caller-driven growth of the revocation store is capped
//!    (`MAX_VIEWER_SESSION_REVOCATIONS`): past the cap a pair the store does not
//!    already track is reported as that pair's own failure instead of being recorded,
//!    while a pair it does track is still revocable.

use serde_json::{json, Value};

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state::ComAmazonawsIvsAmazoninteractivevideoserviceState;

// The crate's shared IVS harness: one emulator over one shared engine, driven
// through `MultiServiceRouter::dispatch`, plus the store accessor every suite that
// needs to read persisted state uses.
use super::channel_lifecycle::{text, Ivs};

/// A well-formed `ChannelArn` (the modelled pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:channel/[a-zA-Z0-9-]+$`) that names no channel any
/// of these tests creates, so it is the "channel does not exist" case rather than a
/// refused request.
const ABSENT_CHANNEL_ARN: &str = "arn:aws:ivs:us-east-1:000000000000:channel/absent0000001";

/// A second one, so a case can carry more than one missing pair.
const OTHER_ABSENT_CHANNEL_ARN: &str = "arn:aws:ivs:us-east-1:000000000000:channel/absent0000002";

/// The per-item `code` a missing channel is reported with: the single-pair
/// `StartViewerSessionRevocation` models `ResourceNotFoundException` ("Request
/// references a resource which does not exist") for exactly that failure.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The per-item `code` a pair past the store's revocation cap is reported with.
const QUOTA_CODE: &str = "ServiceQuotaExceededException";

/// What this suite needs on top of the shared harness. An inherent impl rather than
/// a second harness: the type is this test crate's own.
impl Ivs {
    /// Create a channel named `name` and return its ARN.
    async fn named_channel(&self, name: &str) -> String {
        text(
            &self.ok("CreateChannel", json!({ "name": name })).await["channel"],
            "arn",
        )
    }

    /// `BatchStartViewerSessionRevocation` over `sessions`, required to succeed, with
    /// its `errors` list as `(channelArn, viewerId, code)` triples in wire order.
    async fn revoke(&self, sessions: Value) -> Vec<(String, String, String)> {
        let body = self
            .ok(
                "BatchStartViewerSessionRevocation",
                json!({ "viewerSessions": sessions }),
            )
            .await;
        let reported = body
            .get("errors")
            .unwrap_or_else(|| panic!("the response must carry an errors list, got {body}"))
            .as_array()
            .unwrap_or_else(|| panic!("errors must be a list, got {body}"))
            .clone();
        for entry in &reported {
            assert!(
                !text(entry, "message").is_empty(),
                "case {entry}: a reported per-item error must carry a message"
            );
        }
        reported
            .iter()
            .map(|entry| {
                (
                    text(entry, "channelArn"),
                    text(entry, "viewerId"),
                    text(entry, "code"),
                )
            })
            .collect()
    }

    /// The version threshold the store holds for `(channel_arn, viewer_id)`, or
    /// `None` when no revocation was started for that pair.
    ///
    /// Read straight from this service's store because IVS models no operation that
    /// reports a viewer-session revocation back.
    fn threshold(&self, channel_arn: &str, viewer_id: &str) -> Option<i32> {
        self.with_store(|store: &ComAmazonawsIvsAmazoninteractivevideoserviceState| {
            store
                .viewer_session_revocations
                .get(&(channel_arn.to_string(), viewer_id.to_string()))
                .copied()
        })
    }

    /// How many pairs the store holds a revocation for.
    fn revocation_count(&self) -> usize {
        self.with_store(
            |store: &ComAmazonawsIvsAmazoninteractivevideoserviceState| {
                store.viewer_session_revocations.len()
            },
        )
    }
}

/// One pair of the `viewerSessions` list.
fn pair(channel_arn: &str, viewer_id: &str, version: Option<i32>) -> Value {
    let mut session = json!({ "channelArn": channel_arn, "viewerId": viewer_id });
    if let Some(version) = version {
        session["viewerSessionVersionsLessThanOrEqualTo"] = json!(version);
    }
    session
}

/// RULE 1: the call succeeds whatever the mix of pairs, and reports exactly the pairs
/// whose channel the store does not hold, in request order.
///
/// The rows differ along what the rule generalises over: how many pairs the request
/// carries (1, 2, 3 and the modelled maximum of 20), which of them are missing
/// (none, the first, the last, a middle one, all), whether a pair repeats, and
/// whether the optional version is sent at all.
#[tokio::test]
async fn every_pair_is_attempted_and_only_the_missing_channels_are_reported() {
    // `present` is an index into the channels the case creates; `absent` is an ARN
    // that names none. Each row is (case, pairs, expected missing positions).
    struct Case {
        name: &'static str,
        /// One entry per pair: `Some(index)` picks a created channel, `None` means
        /// `ABSENT_CHANNEL_ARN`.
        channels: Vec<Option<usize>>,
        /// The viewer id of each pair, so a row can repeat a pair or vary ids.
        viewers: Vec<&'static str>,
        /// The version each pair sends, `None` to leave it out.
        versions: Vec<Option<i32>>,
    }

    let twenty_channels: Vec<Option<usize>> = (0..20).map(Some).collect();
    let twenty_viewers: Vec<&str> = vec![
        "v00", "v01", "v02", "v03", "v04", "v05", "v06", "v07", "v08", "v09", "v10", "v11", "v12",
        "v13", "v14", "v15", "v16", "v17", "v18", "v19",
    ];
    let mut twenty_mixed = twenty_channels.clone();
    twenty_mixed[3] = None;
    twenty_mixed[19] = None;

    let cases = vec![
        Case {
            name: "one present pair",
            channels: vec![Some(0)],
            viewers: vec!["viewer-a"],
            versions: vec![Some(0)],
        },
        Case {
            name: "one missing pair",
            channels: vec![None],
            viewers: vec!["viewer-a"],
            versions: vec![Some(7)],
        },
        Case {
            name: "missing first, present second",
            channels: vec![None, Some(0)],
            viewers: vec!["viewer-a", "viewer-b"],
            versions: vec![None, Some(2)],
        },
        Case {
            name: "present first, missing last",
            channels: vec![Some(0), None],
            viewers: vec!["viewer-a", "viewer-b"],
            versions: vec![Some(1), None],
        },
        Case {
            name: "missing in the middle of three",
            channels: vec![Some(0), None, Some(1)],
            viewers: vec!["viewer-a", "viewer-b", "viewer-c"],
            versions: vec![Some(3), Some(4), None],
        },
        Case {
            name: "every pair missing",
            channels: vec![None, None],
            viewers: vec!["viewer-a", "viewer-b"],
            versions: vec![None, Some(9)],
        },
        Case {
            name: "the same present pair twice",
            channels: vec![Some(0), Some(0)],
            viewers: vec!["viewer-a", "viewer-a"],
            versions: vec![Some(2), Some(5)],
        },
        Case {
            name: "the same missing pair twice",
            channels: vec![None, None],
            viewers: vec!["viewer-a", "viewer-a"],
            versions: vec![Some(1), Some(1)],
        },
        Case {
            name: "the modelled maximum of twenty present pairs",
            channels: twenty_channels,
            viewers: twenty_viewers.clone(),
            versions: vec![None; 20],
        },
        Case {
            name: "twenty pairs with two missing channels",
            channels: twenty_mixed,
            viewers: twenty_viewers,
            versions: (0..20).map(Some).collect(),
        },
    ];

    for case in cases {
        let ivs = Ivs::new();
        let needed = case.channels.iter().flatten().copied().max().unwrap_or(0) + 1;
        let mut arns = Vec::new();
        for index in 0..needed {
            arns.push(ivs.named_channel(&format!("channel-{index}")).await);
        }

        let sessions: Vec<Value> = case
            .channels
            .iter()
            .zip(&case.viewers)
            .zip(&case.versions)
            .map(|((channel, viewer), version)| {
                let arn = channel.map_or(ABSENT_CHANNEL_ARN, |index| arns[index].as_str());
                pair(arn, viewer, *version)
            })
            .collect();

        let expected: Vec<(String, String, String)> = case
            .channels
            .iter()
            .zip(&case.viewers)
            .filter(|(channel, _)| channel.is_none())
            .map(|(_, viewer)| {
                (
                    ABSENT_CHANNEL_ARN.to_string(),
                    (*viewer).to_string(),
                    NOT_FOUND_CODE.to_string(),
                )
            })
            .collect();

        let reported = ivs.revoke(json!(sessions)).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected the errors {expected:?}, got {reported:?}",
            case.name
        );

        // And the accepted pairs — and only those — are what the store now holds.
        let accepted: Vec<(String, String)> = case
            .channels
            .iter()
            .zip(&case.viewers)
            .filter_map(|(channel, viewer)| {
                channel.map(|index| (arns[index].clone(), (*viewer).to_string()))
            })
            .collect();
        let mut distinct = accepted.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            ivs.revocation_count(),
            distinct.len(),
            "case {:?}: the store must hold one revocation per distinct accepted pair \
             ({distinct:?})",
            case.name
        );
        for (arn, viewer) in &accepted {
            assert!(
                ivs.threshold(arn, viewer).is_some(),
                "case {:?}: the pair ({arn}, {viewer}) was accepted, so its revocation \
                 must be persisted",
                case.name
            );
        }
        assert_eq!(
            ivs.threshold(ABSENT_CHANNEL_ARN, "viewer-a"),
            None,
            "case {:?}: a reported pair must persist nothing",
            case.name
        );
    }
}

/// RULE 2: the stored threshold is the maximum of the versions revoked so far, so a
/// lower version never un-revokes and a repeat is idempotent.
///
/// The rows differ along the sequence of versions: ascending, descending, repeated,
/// the modelled minimum of 0, and the largest `ViewerSessionVersion` an `integer`
/// holds.
#[tokio::test]
async fn the_revoked_version_threshold_only_ever_rises() {
    let cases: Vec<(&str, Vec<i32>, i32)> = vec![
        ("a single call", vec![5], 5),
        ("the modelled minimum", vec![0], 0),
        ("ascending", vec![1, 2, 3], 3),
        ("descending", vec![7, 3, 1], 7),
        ("a repeat of the same version", vec![4, 4, 4], 4),
        ("down then up", vec![9, 2, 11], 11),
        ("the top of the modelled range", vec![0, i32::MAX, 1], i32::MAX),
    ];

    for (case, versions, expected) in cases {
        let ivs = Ivs::new();
        let arn = ivs.named_channel("revoked").await;
        for version in &versions {
            let reported = ivs
                .revoke(json!([pair(&arn, "viewer-a", Some(*version))]))
                .await;
            assert!(
                reported.is_empty(),
                "case {case:?} version {version}: a present channel reports no error, \
                 got {reported:?}"
            );
        }
        assert_eq!(
            ivs.threshold(&arn, "viewer-a"),
            Some(expected),
            "case {case:?}: after revoking {versions:?} the threshold must be {expected}, \
             got {:?}",
            ivs.threshold(&arn, "viewer-a")
        );
    }
}

/// RULE 2, continued: the version is optional and defaults to 0 — "Default: 0" on
/// `viewerSessionVersionsLessThanOrEqualTo`, which the model also declares
/// `@default(0)`.
#[tokio::test]
async fn an_omitted_version_revokes_version_zero() {
    let ivs = Ivs::new();
    let arn = ivs.named_channel("defaulted").await;
    let reported = ivs.revoke(json!([pair(&arn, "viewer-a", None)])).await;
    assert!(
        reported.is_empty(),
        "a present channel reports no error, got {reported:?}"
    );
    assert_eq!(
        ivs.threshold(&arn, "viewer-a"),
        Some(0),
        "an omitted version is the modelled default 0, got {:?}",
        ivs.threshold(&arn, "viewer-a")
    );
}

/// RULE 4: the record is per `(channelArn, viewerId)` pair — two viewers on one
/// channel, and one viewer on two channels, keep independent thresholds — and the
/// channel itself is untouched by a revocation.
#[tokio::test]
async fn revocation_is_tracked_per_channel_and_viewer_pair() {
    let ivs = Ivs::new();
    let first = ivs.named_channel("first").await;
    let second = ivs.named_channel("second").await;
    let before = ivs.ok("GetChannel", json!({ "arn": first })).await;

    let reported = ivs
        .revoke(json!([
            pair(&first, "viewer-a", Some(1)),
            pair(&first, "viewer-b", Some(2)),
            pair(&second, "viewer-a", Some(3)),
        ]))
        .await;
    assert!(
        reported.is_empty(),
        "every channel exists, so nothing is reported, got {reported:?}"
    );

    let expected = [
        (first.clone(), "viewer-a", 1),
        (first.clone(), "viewer-b", 2),
        (second.clone(), "viewer-a", 3),
    ];
    for (arn, viewer, threshold) in &expected {
        assert_eq!(
            ivs.threshold(arn, viewer),
            Some(*threshold),
            "case ({arn}, {viewer}): the pair's own threshold must be {threshold}, got {:?}",
            ivs.threshold(arn, viewer)
        );
    }
    assert_eq!(
        ivs.threshold(&second, "viewer-b"),
        None,
        "case ({second}, viewer-b): a pair nobody revoked must hold nothing, got {:?}",
        ivs.threshold(&second, "viewer-b")
    );
    assert_eq!(
        ivs.revocation_count(),
        expected.len(),
        "the store must hold exactly the three pairs the call named"
    );

    let after = ivs.ok("GetChannel", json!({ "arn": first })).await;
    assert_eq!(
        after, before,
        "a revocation changes nothing about the channel itself, got {after}"
    );
}

/// RULE 3: a request outside a modelled constraint is a 400 `ValidationException`
/// before the handler runs, and records nothing.
///
/// The rows walk each published bound of each constraint and one past it: the list's
/// `len 1..20`, `viewerId`'s `len 1..40`, the version's `range 0..`, and the
/// `ChannelArn` pattern.
#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused_and_records_nothing() {
    let ivs = Ivs::new();
    let arn = ivs.named_channel("constrained").await;
    let longest_viewer_id = "v".repeat(40);
    let too_long_viewer_id = "v".repeat(41);

    // Rows the model ACCEPTS: both ends of every bound.
    let accepted: Vec<(&str, Value)> = vec![
        ("one pair, the list's minimum", json!([pair(&arn, "v", Some(0))])),
        (
            "twenty pairs, the list's maximum",
            json!((0..20)
                .map(|index| pair(&arn, &format!("viewer-{index}"), Some(index)))
                .collect::<Vec<_>>()),
        ),
        (
            "a viewerId of 1 character, the member's minimum",
            json!([pair(&arn, "v", Some(1))]),
        ),
        (
            "a viewerId of 40 characters, the member's maximum",
            json!([pair(&arn, &longest_viewer_id, Some(1))]),
        ),
        ("version 0, the range's minimum", json!([pair(&arn, "zero", Some(0))])),
    ];
    for (case, sessions) in accepted {
        let reply = ivs
            .call(
                "BatchStartViewerSessionRevocation",
                &json!({ "viewerSessions": sessions }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a request inside every modelled bound must be accepted, got {reply:?}"
        );
    }

    let refused: Vec<(&str, Value)> = vec![
        ("an empty list, one below the minimum", json!({ "viewerSessions": [] })),
        ("no viewerSessions at all", json!({})),
        (
            "twenty-one pairs, one past the maximum",
            json!({ "viewerSessions": (0..21)
                .map(|index| pair(&arn, &format!("viewer-{index}"), Some(0)))
                .collect::<Vec<_>>() }),
        ),
        (
            "a viewerId of 41 characters, one past the maximum",
            json!({ "viewerSessions": [pair(&arn, &too_long_viewer_id, Some(0))] }),
        ),
        (
            "an empty viewerId, one below the minimum",
            json!({ "viewerSessions": [pair(&arn, "", Some(0))] }),
        ),
        (
            "version -1, one below the range",
            json!({ "viewerSessions": [pair(&arn, "viewer-a", Some(-1))] }),
        ),
        (
            "a channelArn outside the modelled pattern",
            json!({ "viewerSessions": [pair("not-an-arn", "viewer-a", Some(0))] }),
        ),
        (
            "a stream-key arn in place of a channel arn",
            json!({ "viewerSessions": [pair(
                "arn:aws:ivs:us-east-1:000000000000:stream-key/abcdef123456",
                "viewer-a",
                Some(0),
            )] }),
        ),
        (
            "a pair with no channelArn",
            json!({ "viewerSessions": [{ "viewerId": "viewer-a" }] }),
        ),
        (
            "a pair with no viewerId",
            json!({ "viewerSessions": [{ "channelArn": arn }] }),
        ),
    ];

    let before = ivs.revocation_count();
    for (case, body) in refused {
        let reply = ivs
            .call("BatchStartViewerSessionRevocation", &body)
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {reply:?}"
        );
    }
    assert_eq!(
        ivs.revocation_count(),
        before,
        "a refused request must record nothing"
    );
}

/// RULE 5: caller-driven growth of the revocation store is capped, and a pair past
/// the cap is reported as that pair's own failure rather than recorded — while a pair
/// the store ALREADY tracks is still revocable, because raising a threshold adds no
/// entry.
///
/// A record whose channel has been deleted can never be revoked against again, so it
/// is dropped before the cap refuses anything: the last rows prove both halves.
#[tokio::test]
async fn tracking_a_new_pair_past_the_cap_is_refused_rather_than_growing_the_store() {
    // Read from the constant the service enforces rather than re-declared here.
    use service_com_amazonaws_ivs_amazoninteractivevideoservice::state::MAX_VIEWER_SESSION_REVOCATIONS;

    /// The modelled maximum number of pairs one call may carry.
    const PER_CALL: usize = 20;

    let ivs = Ivs::new();
    let kept = ivs.named_channel("kept").await;
    let doomed = ivs.named_channel("doomed").await;

    // Fill the store to its cap: all but one callful of pairs on the channel that
    // stays, and the last callful on the channel that is about to be deleted.
    let on_kept = MAX_VIEWER_SESSION_REVOCATIONS - PER_CALL;
    for start in (0..on_kept).step_by(PER_CALL) {
        let sessions: Vec<Value> = (start..start + PER_CALL)
            .map(|index| pair(&kept, &format!("viewer-{index}"), Some(0)))
            .collect();
        let reported = ivs.revoke(json!(sessions)).await;
        assert!(
            reported.is_empty(),
            "case fill at {start}: a pair under the cap is accepted, got {reported:?}"
        );
    }
    let doomed_viewers: Vec<String> = (0..PER_CALL).map(|index| format!("doomed-{index}")).collect();
    let reported = ivs
        .revoke(json!(doomed_viewers
            .iter()
            .map(|viewer| pair(&doomed, viewer, Some(0)))
            .collect::<Vec<_>>()))
        .await;
    assert!(
        reported.is_empty(),
        "case fill to the cap: the last pairs under the cap are accepted, got {reported:?}"
    );
    assert_eq!(
        ivs.revocation_count(),
        MAX_VIEWER_SESSION_REVOCATIONS,
        "case at the cap: the store must hold exactly {MAX_VIEWER_SESSION_REVOCATIONS} pairs"
    );

    // One past the cap: a NEW pair is refused as that pair's own failure, and a pair
    // already tracked is still revocable.
    let reported = ivs
        .revoke(json!([
            pair(&kept, "one-too-many", Some(0)),
            pair(&kept, "viewer-0", Some(5)),
        ]))
        .await;
    assert_eq!(
        reported,
        vec![(
            kept.clone(),
            "one-too-many".to_string(),
            QUOTA_CODE.to_string(),
        )],
        "case past the cap: only the new pair is refused, got {reported:?}"
    );
    assert_eq!(
        ivs.threshold(&kept, "viewer-0"),
        Some(5),
        "case past the cap: an already tracked pair is still revocable, got {:?}",
        ivs.threshold(&kept, "viewer-0")
    );
    assert_eq!(
        ivs.threshold(&kept, "one-too-many"),
        None,
        "case past the cap: a refused pair records nothing"
    );
    assert_eq!(
        ivs.revocation_count(),
        MAX_VIEWER_SESSION_REVOCATIONS,
        "case past the cap: a refused pair must not grow the store"
    );

    // Deleting a channel makes its records dead, so the next new pair fits again.
    let deleted = ivs.call("DeleteChannel", &json!({ "arn": doomed })).await;
    assert_eq!(
        deleted.status, 204,
        "case delete: DeleteChannel must succeed, got {deleted:?}"
    );
    let reported = ivs.revoke(json!([pair(&kept, "after-prune", Some(3))])).await;
    assert!(
        reported.is_empty(),
        "case after the delete: a new pair fits once the dead records are dropped, \
         got {reported:?}"
    );
    assert_eq!(
        ivs.threshold(&kept, "after-prune"),
        Some(3),
        "case after the delete: the new pair is recorded, got {:?}",
        ivs.threshold(&kept, "after-prune")
    );
    assert_eq!(
        ivs.revocation_count(),
        MAX_VIEWER_SESSION_REVOCATIONS - PER_CALL + 1,
        "case after the delete: the deleted channel's {PER_CALL} dead records are gone"
    );
    for viewer in &doomed_viewers {
        assert_eq!(
            ivs.threshold(&doomed, viewer),
            None,
            "case after the delete: ({doomed}, {viewer}) is a dead record and must be gone"
        );
    }
}

/// A channel the store holds is reported, and a channel it no longer holds is
/// reported as missing: the per-item outcome follows the persisted channel set, not
/// the request.
#[tokio::test]
async fn a_deleted_channel_becomes_a_reported_pair() {
    let ivs = Ivs::new();
    let kept = ivs.named_channel("kept").await;
    let doomed = ivs.named_channel("doomed").await;

    let sessions = json!([
        pair(&kept, "viewer-a", Some(1)),
        pair(&doomed, "viewer-a", Some(1)),
        pair(OTHER_ABSENT_CHANNEL_ARN, "viewer-a", Some(1)),
    ]);
    let reported = ivs.revoke(sessions.clone()).await;
    assert_eq!(
        reported,
        vec![(
            OTHER_ABSENT_CHANNEL_ARN.to_string(),
            "viewer-a".to_string(),
            NOT_FOUND_CODE.to_string(),
        )],
        "case before the delete: only the never-created channel is reported, got {reported:?}"
    );

    // `DeleteChannel` answers 204 with no body, so it is called through `call`.
    let deleted = ivs.call("DeleteChannel", &json!({ "arn": doomed })).await;
    assert_eq!(
        deleted.status, 204,
        "case delete: DeleteChannel must succeed, got {deleted:?}"
    );

    let reported = ivs.revoke(sessions).await;
    assert_eq!(
        reported,
        vec![
            (
                doomed.clone(),
                "viewer-a".to_string(),
                NOT_FOUND_CODE.to_string(),
            ),
            (
                OTHER_ABSENT_CHANNEL_ARN.to_string(),
                "viewer-a".to_string(),
                NOT_FOUND_CODE.to_string(),
            ),
        ],
        "case after the delete: the deleted channel is reported too, got {reported:?}"
    );
    assert_eq!(
        ivs.threshold(&kept, "viewer-a"),
        Some(1),
        "the pair naming the surviving channel is still revoked, got {:?}",
        ivs.threshold(&kept, "viewer-a")
    );
}
