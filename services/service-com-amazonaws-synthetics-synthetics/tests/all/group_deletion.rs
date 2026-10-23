// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteGroup`, the operation this suite's issue implemented.
//!
//! The rules asserted here, over a set of inputs rather than one recorded request:
//!
//! 1. A delete REMOVES the stored record: afterwards `GetGroup` misses by every
//!    identifier form, the name is free for a new group, and a second delete misses too —
//!    addressed by "the group name, the ARN, or the group ID"
//!    (`DeleteGroup$GroupIdentifier`), for every shape of `GroupName` (`string`,
//!    `@length 1..64`, no `@pattern`, "can include any Unicode characters").
//! 2. A `GroupIdentifier` the store does not hold is answered 404
//!    `ResourceNotFoundException` whose message is BUILT FROM THE REQUEST: the recorded
//!    unit `synthetics.delete-group-nonexistent-resource-not-found` shows the service
//!    answering `Group does not exist: <the GroupIdentifier asked for>`, so what is
//!    asserted is that construction over a table of identifiers rather than one string.
//! 3. "Groups are a global resource that appear in all Regions, but the request to delete
//!    a group must be made from its home Region" (`DeleteGroup`), while `GetGroup` is
//!    usable "from any Region" — so a delete signed for another Region is refused and the
//!    group survives.
//! 4. "The group doesn't need to be empty to be deleted. If there are canaries in the
//!    group, they are not deleted when you delete the group" (`DeleteGroup`).
//! 5. Every `DeleteGroup` call leaves exactly ONE structured audit record carrying the
//!    operation, the caller, the Region and the outcome, and nothing the caller supplied.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the emulator
//! boots, through the harness `canary_lifecycle` defines.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::canary_lifecycle::{create_body, Answer, Synthetics};
use crate::group_lifecycle::{audit_capture, group_not_found_message};

/// How `DeleteGroup` words a delete issued outside the group's home Region: one sentence
/// naming the member and the rule, carrying nothing from the request.
///
/// Written out as a literal ON PURPOSE, rather than imported from
/// `ops::delete_group::DELETE_GROUP_NOT_HOME_REGION`, so the assertion pins the text that
/// goes over the wire: a silent edit of the handler constant has to fail here. The two are
/// intentional mirrors — change one and this test tells you to change the other.
const NOT_HOME_REGION: &str = "GroupIdentifier: a group can be deleted only from its home Region.";

/// `GroupName`'s modelled maximum length (`string`, `@length 1..64`).
const GROUP_NAME_MAX_LEN: usize = 64;

/// The audit target `state::AUDIT_TARGET` names, written as a literal so the assertions
/// pin the wire-visible target rather than whatever the constant holds.
const AUDIT_TARGET: &str = "emulator::audit";

/// The Region the audit test signs for: its own, so its records are identifiable among
/// those a sibling test writes in parallel.
const AUDIT_REGION: &str = "eu-north-1";

/// The non-home Region the audit test signs its refused delete for — also its own, and
/// used by no other test in this binary.
const AUDIT_AWAY_REGION: &str = "me-south-1";

/// Every identifier this suite puts in a request path is one a URI path segment carries
/// AS WRITTEN — RFC 3986's `pchar` (the unreserved characters, plus `:` and `@`, which is
/// what a group ARN needs) — so nothing here escapes a label and nothing here encodes one.
/// A `GroupName` may be any Unicode text, and a name that a path segment cannot carry
/// (spaces, non-ASCII) is addressed through the two identifier forms that are always
/// path-safe, its minted id (`Group$Arn` pins the id to `[0-9a-z]+`) and its ARN: the rule
/// under test is that all three forms resolve to the one stored record, not how a client
/// escapes a URI.
async fn create_group(aws: &Synthetics, body: Value) -> Answer {
    aws.call("POST", "/group", Some(body)).await
}

async fn get_group(aws: &Synthetics, identifier: &str) -> Answer {
    aws.call("GET", &format!("/group/{identifier}"), None).await
}

async fn get_group_from(aws: &Synthetics, region: &str, identifier: &str) -> Answer {
    aws.call_in_region(region, "GET", &format!("/group/{identifier}"), None)
        .await
}

async fn delete_group(aws: &Synthetics, identifier: &str) -> Answer {
    aws.call("DELETE", &format!("/group/{identifier}"), None)
        .await
}

async fn delete_group_from(aws: &Synthetics, region: &str, identifier: &str) -> Answer {
    aws.call_in_region(region, "DELETE", &format!("/group/{identifier}"), None)
        .await
}

/// Group names spanning what `GroupName` generalises over: length (one character up to
/// the modelled maximum), script (ASCII, accented Latin, non-Latin) and the spaces and
/// punctuation a name with no `@pattern` admits. None is the name any recorded request
/// used. The flag says whether a URI path segment can carry that name as written, which
/// decides which identifier forms the row can address the group by.
fn group_names() -> Vec<(&'static str, String, bool)> {
    vec![
        ("single-character", "d".to_string(), true),
        ("ascii-hyphenated", "doomed-checks".to_string(), true),
        ("digits-only", "20261022".to_string(), true),
        ("accented-latin", "contrôles-naïfs".to_string(), false),
        ("non-latin-script", "グループ".to_string(), false),
        ("spaces-and-punctuation", "my group (7)".to_string(), false),
        ("max-length", "d".repeat(GROUP_NAME_MAX_LEN), true),
    ]
}

/// Assert the 404/`ResourceNotFoundException` envelope and the request-derived message.
fn assert_group_not_found(case: &str, identifier: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected 404, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case}: expected ResourceNotFoundException, got {:?}",
        answer.error_code
    );
    let message = answer
        .at("Message")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a Message, got {:?}", answer.body))
        .to_string();
    assert_eq!(
        message,
        group_not_found_message(identifier),
        "case {case}: expected the missing-group message built from the requested \
         identifier {identifier:?}, got {message:?}"
    );
}

/// A delete removes the stored record: every identifier form addresses it, every form
/// then misses, the name is free again, and a second delete is a miss rather than a
/// success.
#[tokio::test]
async fn deleting_a_group_removes_it_whichever_identifier_form_addresses_it() {
    let aws = Synthetics::new("us-east-2");
    let mut deleted_ids: BTreeSet<String> = BTreeSet::new();

    for (index, (case, name, name_is_path_safe)) in group_names().into_iter().enumerate() {
        let created = create_group(&aws, json!({ "Name": name })).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateGroup, got {} ({:?})",
            created.status, created.body
        );
        let id = created.string_at("Group.Id");
        let arn = created.string_at("Group.Arn");
        // The id and the ARN always go in a path as written; the name does so only when a
        // path segment can carry it.
        let mut forms = vec![("id", id.clone()), ("arn", arn.clone())];
        if name_is_path_safe {
            forms.push(("name", name.clone()));
        }

        // Delete by a different identifier form on each row, so every form is exercised
        // as the delete key.
        let (form, identifier) = &forms[index % forms.len()];
        let deleted = delete_group(&aws, identifier).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}/delete-by-{form}: expected 200 from DeleteGroup, got {} ({:?})",
            deleted.status, deleted.body
        );
        assert!(
            deleted_ids.insert(id.clone()),
            "case {case}: expected a distinct group per row, got the repeated id {id:?}"
        );

        // Gone from the store, whichever form asks, and a second delete misses too.
        for (form, identifier) in &forms {
            let read = get_group(&aws, identifier).await;
            assert_eq!(
                read.status, 404,
                "case {case}/get-after-delete-by-{form}: expected 404, got {} ({:?})",
                read.status, read.body
            );
            assert_group_not_found(
                &format!("{case}/second-delete-by-{form}"),
                identifier,
                &delete_group(&aws, identifier).await,
            );
        }

        // The name the deleted group held is free again, and the new group is a new
        // record rather than the old one resurfacing.
        let recreated = create_group(&aws, json!({ "Name": name })).await;
        assert_eq!(
            recreated.status, 200,
            "case {case}/recreate: expected 200 once the name is free, got {} ({:?})",
            recreated.status, recreated.body
        );
        assert_ne!(
            recreated.string_at("Group.Id"),
            id,
            "case {case}/recreate: expected a freshly minted id, not the deleted {id:?}"
        );
        // Leave the store empty for the next row.
        assert_eq!(
            delete_group(&aws, &recreated.string_at("Group.Id")).await.status,
            200,
            "case {case}/cleanup: expected 200 from DeleteGroup"
        );
    }
}

/// A `GroupIdentifier` the store does not hold is refused with the message built from
/// that identifier — over a table of identifier forms, including near misses of a group
/// that does exist.
#[tokio::test]
async fn a_delete_of_a_missing_group_names_the_requested_identifier() {
    let aws = Synthetics::new("ap-southeast-2");
    let live = create_group(&aws, json!({ "Name": "live-group" })).await;
    assert_eq!(
        live.status, 200,
        "case create-live: expected 200, got {} ({:?})",
        live.status, live.body
    );
    let live_id = live.string_at("Group.Id");
    let account = emulator_server::emulator_account_id();

    // Every row is a legal `GroupIdentifier` (`length 1..128`) no store holds.
    let identifiers = [
        ("short-name", "x".to_string()),
        ("name-form", "no-such-group".to_string()),
        ("suffix-of-live-name", "live-group-2".to_string()),
        ("id-form", "0123456789ab".to_string()),
        (
            "arn-form",
            format!("arn:aws:synthetics:ap-southeast-2:{account}:group:0123456789ab"),
        ),
        (
            // The live group's id inside another account's ARN: an ARN addresses a group
            // only when it is that group's whole ARN.
            "arn-of-another-account",
            format!("arn:aws:synthetics:ap-southeast-2:123456789012:group:{live_id}"),
        ),
        ("max-length-identifier", "z".repeat(128)),
    ];
    let mut messages: BTreeSet<String> = BTreeSet::new();
    let rows = identifiers.len();
    for (case, identifier) in identifiers {
        let answer = delete_group(&aws, &identifier).await;
        assert_group_not_found(case, &identifier, &answer);
        messages.insert(answer.string_at("Message"));
    }
    // The message is request-derived, so it differs per identifier: one fixed sentence
    // across the table would be the canary rule, not this one.
    assert_eq!(
        messages.len(),
        rows,
        "case request-derived: expected one message per requested identifier, got {messages:?}"
    );

    // None of the misses resolved to the live group.
    let read = get_group(&aws, &live_id).await;
    assert_eq!(
        read.status, 200,
        "case live-group-survives: expected the live group to still exist, got {} ({:?})",
        read.status, read.body
    );
}

/// A group is readable from any Region and deletable only from its home Region, and a
/// refused delete leaves the group in place.
#[tokio::test]
async fn a_group_is_global_to_read_and_home_region_only_to_delete() {
    let home = "eu-central-1";
    let aws = Synthetics::new(home);
    let account = emulator_server::emulator_account_id();
    let name = "home-region-group";

    let created = create_group(&aws, json!({ "Name": name })).await;
    assert_eq!(
        created.status, 200,
        "case create: expected 200, got {} ({:?})",
        created.status, created.body
    );
    let id = created.string_at("Group.Id");
    let arn = created.string_at("Group.Arn");
    assert_eq!(
        arn,
        format!("arn:aws:synthetics:{home}:{account}:group:{id}"),
        "case create: expected the ARN to name the home Region, got {arn:?}"
    );

    // Regions spanning the partition's geographies, none of them the home Region.
    for away in ["us-east-1", "ap-southeast-2", "sa-east-1", "eu-west-3"] {
        let read = get_group_from(&aws, away, name).await;
        assert_eq!(
            read.status, 200,
            "case read-from-{away}: expected 200 (a group appears in all Regions), got {} ({:?})",
            read.status, read.body
        );
        assert_eq!(
            read.string_at("Group.Arn"),
            arn,
            "case read-from-{away}: expected the ARN to keep naming the home Region, got {:?}",
            read.at("Group.Arn")
        );

        let refused = delete_group_from(&aws, away, name).await;
        assert_eq!(
            refused.status, 400,
            "case delete-from-{away}: expected 400, got {} ({:?})",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case delete-from-{away}: expected ValidationException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.string_at("Message"),
            NOT_HOME_REGION,
            "case delete-from-{away}: expected the home-Region sentence, got {:?}",
            refused.at("Message")
        );

        let survived = get_group(&aws, name).await;
        assert_eq!(
            survived.string_at("Group.Id"),
            id,
            "case survives-delete-from-{away}: expected the same group to still exist, got {:?}",
            survived.at("Group")
        );
    }

    let deleted = delete_group(&aws, name).await;
    assert_eq!(
        deleted.status, 200,
        "case delete-from-home: expected 200, got {} ({:?})",
        deleted.status, deleted.body
    );
    assert_eq!(
        get_group(&aws, name).await.status,
        404,
        "case delete-from-home: expected the group to be gone"
    );
}

/// Deleting a group deletes nothing else: the canaries stay, even one whose name is the
/// group's name — canaries and groups are separate namespaces.
#[tokio::test]
async fn deleting_a_group_leaves_the_canaries_in_place() {
    let aws = Synthetics::new("us-west-2");
    // A string that is a legal `CanaryName` (`^[0-9a-z_\-]+$`) as well as a group name.
    let shared = "shared-name";

    let canary = aws.create(create_body(shared)).await;
    assert_eq!(
        canary.status, 200,
        "case create-canary: expected 200, got {} ({:?})",
        canary.status, canary.body
    );
    let group = create_group(&aws, json!({ "Name": shared })).await;
    assert_eq!(
        group.status, 200,
        "case create-group: expected 200, got {} ({:?})",
        group.status, group.body
    );

    assert_eq!(
        delete_group(&aws, shared).await.status,
        200,
        "case delete-group: expected 200 from DeleteGroup"
    );
    assert_eq!(
        get_group(&aws, shared).await.status,
        404,
        "case delete-group: expected the group to be gone"
    );

    let read = aws.get(shared).await;
    assert_eq!(
        read.status, 200,
        "case canary-survives: expected the canary to survive the group delete, got {} ({:?})",
        read.status, read.body
    );
    assert_eq!(
        read.string_at("Canary.Name"),
        shared,
        "case canary-survives: expected the stored canary, got {:?}",
        read.at("Canary")
    );
}

/// Every captured `emulator::audit` record this suite's `DeleteGroup` calls wrote.
///
/// Sibling tests in this binary run in parallel, so the filter is the operation plus the
/// Region a call was signed for; the capture is read without draining it, because the
/// create-side audit suite drains the same global capture.
fn delete_group_records(captured: &[String], region: &str) -> Vec<String> {
    captured
        .iter()
        .filter(|record| {
            record.starts_with(&format!("{AUDIT_TARGET} "))
                && record.contains("operation=DeleteGroup")
                && record.contains(&format!("region=\"{region}\""))
        })
        .cloned()
        .collect()
}

/// RULE: every `DeleteGroup` call leaves exactly ONE structured record on the audit
/// target, naming the operation, the identity the request presented, the Region it
/// resolved to and the outcome — and nothing the caller supplied. A table over the
/// outcomes the operation can report.
#[tokio::test]
async fn every_delete_group_call_emits_one_structured_audit_record() {
    // The same global capture the create-side audit suite installs: a test binary can
    // hold only one global `tracing` subscriber, so it is shared rather than re-installed,
    // and read as a delta rather than drained.
    let captured = audit_capture();

    let aws = Synthetics::new(AUDIT_REGION);
    let name = "audited-doomed-group";
    let created = create_group(&aws, json!({ "Name": name })).await;
    assert_eq!(
        created.status, 200,
        "case create: expected 200, got {} ({:?})",
        created.status, created.body
    );

    // (case, the Region the delete is signed for, the outcome it must record, status)
    let cases = [
        (
            "not-home-region",
            AUDIT_AWAY_REGION,
            "rejected_not_home_region",
            400,
        ),
        ("deleted", AUDIT_REGION, "deleted", 200),
        ("not-found", AUDIT_REGION, "rejected_not_found", 404),
    ];
    for (case, region, outcome, status) in cases {
        let before = delete_group_records(&captured.all(), region).len();
        let answer = delete_group_from(&aws, region, name).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected {status}, got {} ({:?})",
            answer.status, answer.body
        );
        let records = delete_group_records(&captured.all(), region);
        assert_eq!(
            records.len(),
            before + 1,
            "case {case}: expected exactly one new audit record for the call, got \
             {records:?}"
        );
        let record = records
            .last()
            .expect("the call just appended an audit record");
        for expected in [
            "operation=DeleteGroup".to_string(),
            // The identity the harness signs with, as the credential scope presents it.
            "caller=\"AKIDEXAMPLE\"".to_string(),
            format!("region=\"{region}\""),
            format!("outcome={outcome}"),
        ] {
            assert!(
                record.contains(&expected),
                "case {case}: expected the audit record to carry {expected:?}, got {record:?}"
            );
        }
        assert!(
            !record.contains(name),
            "case {case}: expected no caller-supplied value in the audit record, found \
             {name:?} in {record:?}"
        );
    }
}
