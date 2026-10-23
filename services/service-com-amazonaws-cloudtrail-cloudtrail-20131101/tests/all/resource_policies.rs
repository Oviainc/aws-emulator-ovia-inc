// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for CloudTrail resource-based policies (`PutResourcePolicy` /
//! `GetResourcePolicy`).
//!
//! Every case goes over the wire: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: CloudTrail_20131101.<Operation>` through the same
//! `MultiServiceRouter` the production router serves, so what is asserted here is
//! what a client sees — including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * a resource-based policy is persisted against the resource its `ResourceArn`
//!   names, and the put response and every later read report that stored document
//!   verbatim, under that resource's canonical ARN;
//! * one resource carries one policy: a second put REPLACES it (`@idempotent`);
//! * a `ResourceArn` that names no stored resource is refused by the case it is:
//!   not a CloudTrail resource ARN at all (`ResourceARNNotValidException`), a
//!   resource type that carries no resource-based policy
//!   (`ResourceTypeNotSupportedException`), or a supported type naming nothing
//!   stored (`ResourceNotFoundException`);
//! * a resource with no policy attached answers `GetResourcePolicy` with
//!   `ResourcePolicyNotFoundException` (HTTP 404), the one 404 this surface models;
//! * a policy document with syntax errors, or without a usable principal, is refused
//!   with `ResourcePolicyNotValidException`;
//! * a write to a resource CloudTrail is still loading is refused with
//!   `ConflictException` until the clock settles it;
//! * a `ResourceArn` outside the modelled `@length`/`@pattern` constraints is refused
//!   as a validation error before any policy is read or written.
//!
//! `CreateEventDataStore` and `CreateChannel` are still scaffold stubs, so no
//! implemented operation can mint either resource. The event data store and channel
//! cases therefore seed the SAME registries the handlers read, through the shared
//! engine (the way `lake_queries` does), and then go over the wire for every
//! assertion.

use serde_json::{json, Value};
use std::thread::sleep;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    self, channels, EventDataStoreRecord, EventDataStoreState,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases that read the Region out of the ARN rather than out
/// of the request.
const OTHER_REGION: &str = "us-west-2";

/// A real-looking account id a client may quote in an ARN, which is NOT the account
/// the emulator mints ARNs for (`engine::emulator_account_id()`).
const OTHER_ACCOUNT: &str = "123456789012";

/// A margin on top of [`state::DASHBOARD_SETTLE_AFTER`], so a case that needs a
/// settled resource does not race the instant the handler reads.
fn past_settle() -> Duration {
    state::DASHBOARD_SETTLE_AFTER + Duration::from_millis(250)
}

/// The ARN of the CloudTrail resource of type `resource_type` called `id` in
/// `region`, in the account the emulator mints ARNs for.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!(
        "arn:aws:cloudtrail:{region}:{}:{resource_type}/{id}",
        engine::emulator_account_id()
    )
}

/// Create a dashboard called `name` in `region` and hand back its ARN, waiting out
/// the load the clock owes so it is ready to carry a policy.
async fn settled_dashboard(emulator: &Emulator, name: &str, region: &str) -> String {
    let created = emulator
        .call_in("CreateDashboard", region, &json!({"Name": name}))
        .await;
    let resource_arn = created
        .expect_ok("create a dashboard to attach a policy to")
        .get("DashboardArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateDashboard reported no ARN: {created:?}"))
        .to_string();
    sleep(past_settle());
    resource_arn
}

/// Seed one event data store, the way `CreateEventDataStore` will once it is
/// implemented, and hand back its ARN.
fn seed_store(
    emulator: &Emulator,
    id: &str,
    region: &str,
    status: EventDataStoreState,
) -> String {
    emulator.state().lock().event_data_stores.insert(
        id.to_string(),
        EventDataStoreRecord {
            id: id.to_string(),
            name: format!("store-{id}"),
            region: region.to_string(),
            status,
            created_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            // Every other member is one a resource-policy operation never reads.
            ..EventDataStoreRecord::default()
        },
    );
    state::event_data_store_arn(region, id)
}

/// Seed one channel, the way `CreateChannel` will once it is implemented, and hand
/// back its ARN.
fn seed_channel(emulator: &Emulator, channel_id: &str, region: &str) -> String {
    let channel_arn = channels::channel_arn(region, channel_id);
    emulator.state().lock().channels.insert(
        (channel_id.to_string(), region.to_string()),
        channels::ChannelRecord {
            channel_id: channel_id.to_string(),
            name: format!("channel-{channel_id}"),
            channel_arn: channel_arn.clone(),
            home_region: region.to_string(),
            // Every other member is one a resource-policy operation never reads.
            ..channels::ChannelRecord::default()
        },
    );
    channel_arn
}

/// Attach `document` to `resource_arn`.
///
/// A free function rather than an inherent method on the shared [`Emulator`]: the
/// dashboard suite already extends that type with a `get`, and two inherent methods
/// of one name on one type cannot both be reached.
async fn put(emulator: &Emulator, resource_arn: &str, document: &str) -> Reply {
    emulator
        .call(
            "PutResourcePolicy",
            &json!({"ResourceArn": resource_arn, "ResourcePolicy": document}),
        )
        .await
}

/// Read the policy attached to `resource_arn`.
async fn get(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .call("GetResourcePolicy", &json!({"ResourceArn": resource_arn}))
        .await
}

/// A policy document that is valid, so a case about something OTHER than the
/// document has one to send.
fn valid_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowOneAccount",
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")},
            "Action": "cloudtrail:GetDashboard",
        }],
    })
    .to_string()
}

/// Assert `reply` reports `document` attached to `resource_arn`, and nothing else.
fn expect_policy(case: &str, reply: &Reply, resource_arn: &str, document: &str) {
    let body = reply.expect_ok(case);
    assert_eq!(
        body.get("ResourceArn").and_then(Value::as_str),
        Some(resource_arn),
        "case {case:?}: expected the resource ARN {resource_arn}, got {body:?}"
    );
    assert_eq!(
        body.get("ResourcePolicy").and_then(Value::as_str),
        Some(document),
        "case {case:?}: expected the stored policy document, got {body:?}"
    );
    // "The default resource-based policy that is automatically generated for the
    // delegated administrator of an Organizations organization": no delegated
    // administrator has been registered, so there is no such policy to report.
    assert!(
        body.get("DelegatedAdminResourcePolicy").is_none(),
        "case {case:?}: no delegated administrator is registered, so no default \
         delegated-administrator policy exists, got {body:?}"
    );
}

/// RULE: a resource-based policy is persisted against the resource its `ResourceArn`
/// names, and the put response and every later read report that stored document
/// verbatim under that resource's canonical ARN.
///
/// The rows differ in the document's shape (a lone statement object versus an array,
/// one statement versus several), in its size, in its formatting (compact versus
/// pretty-printed, which is what "verbatim" is about), in which policy elements it
/// states (`Version`, `Sid`, `Action` versus `NotAction`, `Principal` versus
/// `NotPrincipal`, a wildcard principal versus one or many identifiers), and in the
/// resource's name length and Region.
#[tokio::test]
async fn a_stored_policy_is_reported_back_for_the_resource_it_was_attached_to() {
    let emulator = Emulator::new();

    // A document large enough to exercise a multi-statement policy, still inside the
    // modelled `ResourcePolicy` maximum of 8192 characters.
    let many_statements: Vec<Value> = (0..20)
        .map(|index| {
            json!({
                "Sid": format!("Statement{index}"),
                "Effect": if index % 2 == 0 { "Allow" } else { "Deny" },
                "Principal": {"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:role/reader-{index}")},
                "Action": ["cloudtrail:GetDashboard", "cloudtrail:StartQuery"],
                "Resource": "*",
            })
        })
        .collect();

    // (case, dashboard name, Region, the policy document as it is sent)
    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "a lone statement object, wildcard principal",
            "p1a".to_string(),
            HOME_REGION,
            json!({
                "Statement": {"Effect": "Allow", "Principal": "*", "Action": "cloudtrail:*"},
            })
            .to_string(),
        ),
        (
            "one statement in an array, with Version and Sid",
            "Policy_Dash-9".to_string(),
            HOME_REGION,
            valid_policy(),
        ),
        (
            "a principal naming several identifiers, and NotAction",
            "deny-list".to_string(),
            HOME_REGION,
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": {
                        "AWS": [
                            format!("arn:aws:iam::{OTHER_ACCOUNT}:root"),
                            format!("arn:aws:iam::{OTHER_ACCOUNT}:user/reader"),
                        ],
                        "Service": "cloudtrail.amazonaws.com",
                    },
                    "NotAction": "cloudtrail:GetResourcePolicy",
                }],
            })
            .to_string(),
        ),
        (
            "NotPrincipal instead of Principal",
            "not-principal".to_string(),
            HOME_REGION,
            json!({
                "Statement": [{
                    "Effect": "Deny",
                    "NotPrincipal": {"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")},
                    "Action": "cloudtrail:*",
                }],
            })
            .to_string(),
        ),
        (
            "a pretty-printed document is stored byte for byte",
            "pretty-printed".to_string(),
            HOME_REGION,
            serde_json::to_string_pretty(&json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "cloudtrail.amazonaws.com"},
                    "Action": "cloudtrail:GetChannel",
                }],
            }))
            .expect("render a pretty-printed policy"),
        ),
        (
            "twenty statements",
            "many-statements".to_string(),
            HOME_REGION,
            json!({"Version": "2012-10-17", "Statement": many_statements}).to_string(),
        ),
        (
            "the longest dashboard name the model admits",
            "d".repeat(state::DASHBOARD_NAME_MAX_LEN),
            HOME_REGION,
            valid_policy(),
        ),
        (
            "a resource in another Region, addressed by its own ARN",
            "cross-region".to_string(),
            OTHER_REGION,
            valid_policy(),
        ),
    ];

    for (case, name, region, document) in &rows {
        let resource_arn = settled_dashboard(&emulator, name, region).await;
        assert_eq!(
            resource_arn,
            arn(region, "dashboard", name),
            "case {case:?}: the dashboard's own ARN is what addresses its policy"
        );

        // The put reports what it stored...
        let answer = put(&emulator, &resource_arn, document).await;
        expect_policy(case, &answer, &resource_arn, document);

        // ...and so does every later read, twice over: reading a policy changes
        // nothing a later reader sees.
        for read in 0..2 {
            let answer = get(&emulator, &resource_arn).await;
            expect_policy(
                &format!("{case} (read {read})"),
                &answer,
                &resource_arn,
                document,
            );
        }

        // The account segment is not part of the resource's identity — the emulator
        // mints every ARN for one account — so a client quoting another account id
        // addresses the same resource's policy.
        let quoted = resource_arn.replace(engine::emulator_account_id(), OTHER_ACCOUNT);
        let answer = get(&emulator, &quoted).await;
        expect_policy(
            &format!("{case} (addressed as {quoted})"),
            &answer,
            &resource_arn,
            document,
        );
    }
}

/// RULE: one resource carries one resource-based policy, so a second put REPLACES the
/// stored document rather than adding to it or conflicting with it
/// (`PutResourcePolicy` is `@idempotent`).
///
/// The rows walk a sequence of documents differing in statement count, effect and
/// principal form, re-reading after each put.
#[tokio::test]
async fn a_second_put_replaces_the_stored_policy() {
    let emulator = Emulator::new();
    let resource_arn = settled_dashboard(&emulator, "replaced", HOME_REGION).await;

    let documents: Vec<(&str, String)> = vec![
        (
            "the first document",
            json!({"Statement": {"Effect": "Allow", "Principal": "*", "Action": "cloudtrail:*"}})
                .to_string(),
        ),
        ("a different document", valid_policy()),
        ("the same document again", valid_policy()),
        (
            "a third document denying two principals",
            json!({
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": {"AWS": [
                        format!("arn:aws:iam::{OTHER_ACCOUNT}:root"),
                        format!("arn:aws:iam::{OTHER_ACCOUNT}:role/auditor"),
                    ]},
                    "Action": "cloudtrail:GetResourcePolicy",
                }],
            })
            .to_string(),
        ),
    ];

    for (case, document) in &documents {
        let answer = put(&emulator, &resource_arn, document).await;
        expect_policy(case, &answer, &resource_arn, document);
        let answer = get(&emulator, &resource_arn).await;
        expect_policy(&format!("{case} read back"), &answer, &resource_arn, document);
    }
}

/// RULE (the same one, over another resource KIND): the policy is persisted against
/// the resource the ARN names whatever kind of resource that is — an event data store
/// here — and a resource the service is still changing refuses the WRITE with
/// `ConflictException` ("the specified resource is not ready for an operation …
/// because another operation is modifying the resource") while the read reports that
/// no policy is attached.
///
/// The rows walk every `EventDataStoreStatus` the model enumerates, which is what
/// decides whether the store can take a policy, plus a store addressed from the wrong
/// Region. The stores are seeded because `CreateEventDataStore` is still a stub.
#[tokio::test]
async fn a_policy_attaches_to_an_event_data_store_by_its_status() {
    let emulator = Emulator::new();

    // (case, status, whether a policy can be attached)
    let rows: [(&str, EventDataStoreState, bool); 6] = [
        ("a store that was just created", EventDataStoreState::Created, true),
        ("an enabled store", EventDataStoreState::Enabled, true),
        (
            "a store starting ingestion",
            EventDataStoreState::StartingIngestion,
            true,
        ),
        (
            "a store stopping ingestion",
            EventDataStoreState::StoppingIngestion,
            true,
        ),
        (
            "a store that has stopped ingesting",
            EventDataStoreState::StoppedIngestion,
            true,
        ),
        (
            "a store awaiting deletion: another operation is modifying it",
            EventDataStoreState::PendingDeletion,
            false,
        ),
    ];

    for (index, (case, status, attachable)) in rows.into_iter().enumerate() {
        let id = format!("{index:08x}-1111-2222-3333-444444444444");
        let resource_arn = seed_store(&emulator, &id, HOME_REGION, status);

        // Nothing is attached yet, whatever the status.
        get(&emulator, &resource_arn).await.expect_error(
            &format!("{case}: before any policy"),
            404,
            "ResourcePolicyNotFoundException",
        );

        let answer = put(&emulator, &resource_arn, &valid_policy()).await;
        if attachable {
            expect_policy(case, &answer, &resource_arn, &valid_policy());
            expect_policy(
                &format!("{case} read back"),
                &get(&emulator, &resource_arn).await,
                &resource_arn,
                &valid_policy(),
            );
        } else {
            answer.expect_error(case, 409, "ConflictException");
            get(&emulator, &resource_arn).await.expect_error(
                &format!("{case}: the refused put stored nothing"),
                404,
                "ResourcePolicyNotFoundException",
            );
        }

        // A store id is unique across Regions and the store is reachable only from
        // its own Region, so the same id in another Region names no store.
        let elsewhere = state::event_data_store_arn(OTHER_REGION, &id);
        get(&emulator, &elsewhere).await.expect_error(
            &format!("{case}: addressed from another Region"),
            400,
            "ResourceNotFoundException",
        );
    }
}

/// RULE (the same one, over the third kind the `ResourceArn` doc names): a policy
/// attaches to a stored CHANNEL, is read back from it, and is addressed in the Region
/// the channel's ARN carries.
///
/// The rows differ in the channel's Region and in the document attached. Channels are
/// seeded because `CreateChannel` is still a stub.
#[tokio::test]
async fn a_policy_attaches_to_a_channel() {
    let emulator = Emulator::new();

    // (case, channel id, Region, the document attached)
    let rows: [(&str, &str, &str, String); 2] = [
        (
            "a channel in the caller's own Region",
            "8ada6b3b-e1c3-43c8-8a3a-3281d09f458b",
            HOME_REGION,
            valid_policy(),
        ),
        (
            "a channel in another Region, addressed by its own ARN",
            "01234567-89ab-cdef-0123-456789abcdef",
            OTHER_REGION,
            json!({
                "Statement": {"Effect": "Allow", "Principal": "*", "Action": "cloudtrail:*"},
            })
            .to_string(),
        ),
    ];

    for (case, channel_id, region, document) in rows {
        let resource_arn = seed_channel(&emulator, channel_id, region);

        // Nothing is attached to a freshly stored channel.
        get(&emulator, &resource_arn).await.expect_error(
            &format!("{case}: before any policy"),
            404,
            "ResourcePolicyNotFoundException",
        );

        expect_policy(
            case,
            &put(&emulator, &resource_arn, &document).await,
            &resource_arn,
            &document,
        );
        expect_policy(
            &format!("{case} read back"),
            &get(&emulator, &resource_arn).await,
            &resource_arn,
            &document,
        );

        // A channel is reachable only from the Region its ARN carries, so the same id
        // in another Region names no channel.
        let other_region = if region == HOME_REGION {
            OTHER_REGION
        } else {
            HOME_REGION
        };
        get(&emulator, &channels::channel_arn(other_region, channel_id))
            .await
            .expect_error(
                &format!("{case}: addressed from another Region"),
                400,
                "ResourceNotFoundException",
            );
    }
}

/// RULE: a `ResourceArn` that names no stored resource is refused by the case it is —
/// not a CloudTrail resource ARN at all, a resource type that carries no
/// resource-based policy, or a supported type naming nothing stored — and the same
/// ARN is refused the same way whichever resource-policy operation reads it.
///
/// The rows span the ways an ARN can fail: a bare name, another service's ARN, a
/// missing or extra field, an unknown resource type, an empty resource id, the one
/// CloudTrail resource type that carries no policy, and each supported type with
/// nothing stored under it. The resource types come from the model doc for
/// `ResourceArn` ("the CloudTrail event data store, dashboard, or channel").
#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_refused_by_its_case() {
    let emulator = Emulator::new();
    // A dashboard that DOES exist, so the "no such resource" rows are about the
    // resource being absent rather than about the registry being empty.
    let existing = settled_dashboard(&emulator, "existing-dash", HOME_REGION).await;

    // (case, ResourceArn, expected HTTP status, expected error code)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a bare resource name",
            "existing-dash".to_string(),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{HOME_REGION}:{OTHER_ACCOUNT}:bucket/logs"),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN with no resource field",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{OTHER_ACCOUNT}"),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN with an empty Region",
            format!("arn:aws:cloudtrail::{OTHER_ACCOUNT}:dashboard/existing-dash"),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN with an empty account",
            format!("arn:aws:cloudtrail:{HOME_REGION}::dashboard/existing-dash"),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN naming no CloudTrail resource type",
            arn(HOME_REGION, "query", "0d7b0b5a-2e2a-4f57-9f7f-1f5a6f8f0c11"),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "an ARN whose resource id is empty",
            arn(HOME_REGION, "dashboard", ""),
            400,
            "ResourceARNNotValidException",
        ),
        (
            "a trail ARN: a CloudTrail resource that carries no resource-based policy",
            arn(HOME_REGION, "trail", "org-trail"),
            400,
            "ResourceTypeNotSupportedException",
        ),
        (
            "an event data store that was never created",
            arn(
                HOME_REGION,
                "eventdatastore",
                "EXAMPLE-f852-4e8f-8bd1-bcf6cEXAMPLE",
            ),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a channel that was never created",
            arn(HOME_REGION, "channel", "01234567890"),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a dashboard that was never created",
            arn(HOME_REGION, "dashboard", "no-such-dash"),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a dashboard that exists in another Region",
            arn(OTHER_REGION, "dashboard", "existing-dash"),
            400,
            "ResourceNotFoundException",
        ),
    ];

    for (case, resource_arn, status, code) in &rows {
        let got = get(&emulator, resource_arn).await;
        got.expect_error(&format!("GetResourcePolicy: {case}"), *status, code);
        let wrote = put(&emulator, resource_arn, &valid_policy()).await;
        wrote.expect_error(&format!("PutResourcePolicy: {case}"), *status, code);

        // A resource CloudTrail could not find is named by its kind, the way the
        // resource-policy surface words that kind: a dashboard's sentence names no
        // identifier, every other kind's names the ARN the request supplied. The
        // sentences are written out HERE rather than read back from the handler's own
        // helper, so a change to either side is a failure.
        let expected_message = match *code {
            "ResourceNotFoundException" => {
                let kind = state::ResourceKind::ALL
                    .into_iter()
                    .find(|kind| resource_arn.contains(&format!(":{}", kind.resource_prefix())))
                    .unwrap_or_else(|| panic!("case {case:?}: {resource_arn} names no type"));
                Some(match kind {
                    state::ResourceKind::Dashboard => "The dashboard was not found.".to_string(),
                    state::ResourceKind::EventDataStore => {
                        format!("The event data store {resource_arn} was not found.")
                    }
                    state::ResourceKind::Channel => {
                        format!("The channel {resource_arn} was not found.")
                    }
                    state::ResourceKind::Trail => panic!(
                        "case {case:?}: a trail ARN is refused as an unsupported resource \
                         type, never as a resource that was not found"
                    ),
                })
            }
            // A value that is no CloudTrail resource ARN at all is named back, so a
            // caller sees which value was refused.
            "ResourceARNNotValidException" => Some(format!(
                "The format of the resource ARN {resource_arn} is not valid."
            )),
            _ => None,
        };
        if let Some(expected) = expected_message {
            for (operation, answer) in [("GetResourcePolicy", &got), ("PutResourcePolicy", &wrote)]
            {
                assert_eq!(
                    answer.message(),
                    expected,
                    "case {operation}: {case:?}: expected {expected:?}, got {answer:?}"
                );
            }
        }
    }

    // The resource the rows above are measured against is reachable, so the refusals
    // are about the ARN and not about the service.
    expect_policy(
        "the dashboard that does exist",
        &put(&emulator, &existing, &valid_policy()).await,
        &existing,
        &valid_policy(),
    );
}

/// RULE: a resource that exists but carries no resource-based policy answers
/// `GetResourcePolicy` with `ResourcePolicyNotFoundException` — the one HTTP 404 this
/// surface models — and keeps answering it after a policy is attached and taken away
/// by being replaced on another resource.
#[tokio::test]
async fn a_resource_with_no_policy_attached_is_a_404() {
    let emulator = Emulator::new();

    // Two resources, so attaching a policy to one cannot answer for the other.
    let with_policy = settled_dashboard(&emulator, "with-policy", HOME_REGION).await;
    let without_policy = settled_dashboard(&emulator, "without-policy", HOME_REGION).await;

    for (case, resource_arn) in [
        ("before any policy is attached", &with_policy),
        ("a resource that never carried one", &without_policy),
    ] {
        get(&emulator, resource_arn)
            .await
            .expect_error(case, 404, "ResourcePolicyNotFoundException");
    }

    put(&emulator, &with_policy, &valid_policy())
        .await
        .expect_ok("attach a policy to one of the two");

    expect_policy(
        "the resource the policy was attached to",
        &get(&emulator, &with_policy).await,
        &with_policy,
        &valid_policy(),
    );
    get(&emulator, &without_policy).await.expect_error(
        "the other resource still carries none",
        404,
        "ResourcePolicyNotFoundException",
    );
}

/// RULE: a policy document with syntax errors, or without a principal the policy can
/// grant to, is refused with `ResourcePolicyNotValidException`, and nothing is
/// stored for the resource.
///
/// The rows span the JSON policy grammar the model's wording points at: the document
/// must parse, must be an object stating at least one statement, and each statement
/// must state an `Effect` of `Allow` or `Deny`, an `Action` (or `NotAction`) and a
/// `Principal` (or `NotPrincipal`) naming at least one identifier.
#[tokio::test]
async fn a_policy_document_that_is_not_a_policy_is_refused() {
    let emulator = Emulator::new();
    let resource_arn = settled_dashboard(&emulator, "bad-policies", HOME_REGION).await;

    // (case, the document as it is sent)
    let rows: Vec<(&str, String)> = vec![
        ("not JSON at all", "this is not a policy".to_string()),
        ("truncated JSON", "{\"Statement\": [".to_string()),
        ("a JSON array, not an object", json!([1, 2]).to_string()),
        ("a JSON string", json!("Allow").to_string()),
        ("an object with no Statement", json!({"Version": "2012-10-17"}).to_string()),
        ("an empty Statement list", json!({"Statement": []}).to_string()),
        ("a Statement that is a string", json!({"Statement": "Allow"}).to_string()),
        (
            "a statement that is not an object",
            json!({"Statement": ["Allow"]}).to_string(),
        ),
        (
            "an Effect that is neither Allow nor Deny",
            json!({"Statement": [{"Effect": "Maybe", "Principal": "*", "Action": "cloudtrail:*"}]})
                .to_string(),
        ),
        (
            "no Effect at all",
            json!({"Statement": [{"Principal": "*", "Action": "cloudtrail:*"}]}).to_string(),
        ),
        (
            "no Action or NotAction",
            json!({"Statement": [{"Effect": "Allow", "Principal": "*"}]}).to_string(),
        ),
        (
            "no Principal: an identity policy, not a resource-based one",
            json!({"Statement": [{"Effect": "Allow", "Action": "cloudtrail:*"}]}).to_string(),
        ),
        (
            "an empty Principal map",
            json!({"Statement": [{"Effect": "Allow", "Principal": {}, "Action": "cloudtrail:*"}]})
                .to_string(),
        ),
        (
            "a Principal type with no identifier",
            json!({
                "Statement": [{"Effect": "Allow", "Principal": {"AWS": []}, "Action": "cloudtrail:*"}],
            })
            .to_string(),
        ),
        (
            "a blank principal identifier",
            json!({
                "Statement": [{"Effect": "Allow", "Principal": {"AWS": "  "}, "Action": "cloudtrail:*"}],
            })
            .to_string(),
        ),
        (
            "a principal that is a number",
            json!({"Statement": [{"Effect": "Allow", "Principal": 7, "Action": "cloudtrail:*"}]})
                .to_string(),
        ),
        (
            "the second statement of two is invalid",
            json!({
                "Statement": [
                    {"Effect": "Allow", "Principal": "*", "Action": "cloudtrail:*"},
                    {"Effect": "Allow", "Action": "cloudtrail:*"},
                ],
            })
            .to_string(),
        ),
    ];

    for (case, document) in &rows {
        put(&emulator, &resource_arn, document)
            .await
            .expect_error(case, 400, "ResourcePolicyNotValidException");
        // A refused put stores nothing, so the resource still carries no policy.
        get(&emulator, &resource_arn).await.expect_error(
            &format!("{case}: nothing was stored"),
            404,
            "ResourcePolicyNotFoundException",
        );
    }

    // The same resource takes a valid document, so the refusals above are about the
    // documents and not about the resource.
    expect_policy(
        "a valid document on the same resource",
        &put(&emulator, &resource_arn, &valid_policy()).await,
        &resource_arn,
        &valid_policy(),
    );
}

/// RULE: a write to a resource CloudTrail is still loading is refused with
/// `ConflictException` ("the specified resource is not ready for an operation … before
/// CloudTrail has time to fully load the resource") until the clock settles it; the
/// read of a resource with no policy is unaffected, because `GetResourcePolicy`
/// models no such conflict.
#[tokio::test]
async fn a_write_to_a_resource_that_is_still_loading_is_refused() {
    let emulator = Emulator::new();

    let created = emulator
        .call("CreateDashboard", &json!({"Name": "still-loading"}))
        .await;
    let resource_arn = created
        .expect_ok("create the dashboard")
        .get("DashboardArn")
        .and_then(Value::as_str)
        .expect("CreateDashboard reported an ARN")
        .to_string();

    put(&emulator, &resource_arn, &valid_policy())
        .await
        .expect_error("a dashboard still loading", 409, "ConflictException");
    // Reading the policy of a resource still loading is still a read: it reports
    // that no policy is attached.
    get(&emulator, &resource_arn).await.expect_error(
        "reading a dashboard still loading",
        404,
        "ResourcePolicyNotFoundException",
    );

    sleep(past_settle());

    expect_policy(
        "once the dashboard has finished loading",
        &put(&emulator, &resource_arn, &valid_policy()).await,
        &resource_arn,
        &valid_policy(),
    );
}

/// RULE: a `ResourceArn` outside the modelled constraints — `@length` 3..256 and
/// `@pattern` `^[a-zA-Z0-9._/\-:]+$` on `ResourceArn`, per
/// `smithy_query(query="operation", operation="GetResourcePolicy")` — is refused as a
/// validation error, before any policy is read or written; and a clause about the
/// pattern names the member and the pattern WITHOUT echoing the value that was
/// rejected.
///
/// The rows walk each published length bound and one value past it, plus characters
/// the pattern does not admit, over both operations that take a `ResourceArn`.
#[tokio::test]
async fn an_arn_outside_the_modelled_constraints_is_a_validation_error() {
    let emulator = Emulator::new();

    /// The clause a violated `@pattern` on `ResourceArn` states.
    fn pattern_clause() -> String {
        "1 validation error detected: Value at 'resourceArn' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: ^[a-zA-Z0-9._/\\-:]+$"
            .to_string()
    }

    // (case, ResourceArn, the message when this service words it itself)
    let rows: Vec<(&str, String, Option<String>)> = vec![
        ("one character below the minimum length", "ab".to_string(), None),
        ("empty: both the minimum length and the pattern", String::new(), None),
        (
            "one character past the maximum length",
            format!(
                "{}{}",
                arn(HOME_REGION, "dashboard", ""),
                "d".repeat(257 - arn(HOME_REGION, "dashboard", "").len())
            ),
            None,
        ),
        (
            "a space, which the pattern does not admit",
            arn(HOME_REGION, "dashboard", "bad name"),
            Some(pattern_clause()),
        ),
        (
            "a character class the pattern does not admit",
            arn(HOME_REGION, "dashboard", "bad*name"),
            Some(pattern_clause()),
        ),
        (
            "a newline, which the pattern does not admit",
            arn(HOME_REGION, "dashboard", "bad\nname"),
            Some(pattern_clause()),
        ),
    ];

    for (case, resource_arn, expected_message) in &rows {
        assert!(
            resource_arn.chars().count() < 3
                || resource_arn.chars().count() > 256
                || resource_arn
                    .chars()
                    .any(|c| !(c.is_ascii_alphanumeric() || ".-_/:".contains(c))),
            "case {case:?}: {resource_arn:?} is inside the modelled constraints, so it is \
             the wrong row for this rule"
        );

        let answers = [
            ("GetResourcePolicy", get(&emulator, resource_arn).await),
            (
                "PutResourcePolicy",
                put(&emulator, resource_arn, &valid_policy()).await,
            ),
        ];
        for (operation, got) in &answers {
            let case = format!("{operation}: {case}");
            assert_eq!(
                (got.status, got.error_code()),
                (400, "ValidationException"),
                "case {case:?}: expected HTTP 400 ValidationException, got {got:?}"
            );
            assert!(
                got.message().contains("validation error"),
                "case {case:?}: expected the AWS validation envelope, got {got:?}"
            );
            if let Some(expected) = expected_message {
                assert_eq!(
                    got.message(),
                    expected,
                    "case {case:?}: expected the value-free pattern clause, got {got:?}"
                );
                assert!(
                    !got.message().contains(resource_arn.as_str()),
                    "case {case:?}: a pattern clause names the member and the pattern, never \
                     the value that was rejected, got {got:?}"
                );
            }
        }
    }

    // The shortest ARN that is INSIDE the modelled bounds is not a validation error:
    // it is refused by this service's own rule instead.
    get(&emulator, "abc").await.expect_error(
        "the shortest value the model admits",
        400,
        "ResourceARNNotValidException",
    );
}
