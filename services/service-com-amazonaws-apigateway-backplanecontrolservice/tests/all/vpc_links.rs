// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an account's VPC links: `CreateVpcLink` and `GetVpcLink`, and
//! the tag collection a link carries.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator and the request builder are this
//! crate's existing harness, reused from `rest_api_lifecycle`.
//!
//! The RULES under test:
//!
//! * a created VPC link is PERSISTED under an id the service mints, and `GetVpcLink`
//!   reports that record member for member — the members the request set as it set them,
//!   and nothing for the members it did not;
//! * a link's `status` is the one the CLOCK owes: it is `PENDING` while the link is being
//!   provisioned and `AVAILABLE` once the provisioning span has passed, for every reader
//!   rather than for the first one, since creating a link is "an asynchronous operation"
//!   (`CreateVpcLink`'s own documentation) and `VpcLinkStatus` declares both values;
//! * a `vpcLinkId` no stored link answers to is the modelled `NotFoundException` (404),
//!   and a request this service will not store is the modelled `BadRequestException`
//!   (400) — neither is a 501; a create past the account's published link quota is the
//!   modelled `LimitExceededException` (429).
//!
//! The tables span what the rules generalise over rather than one recorded call: how many
//! target ARNs a link names and of which vendor, which optional members the request
//! carries, how many tags it sets, the forms an unknown id takes, and the ways a request
//! can fall outside what the service stores.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_VPC_LINKS, VPC_LINK_AVAILABLE, VPC_LINK_PENDING, VPC_LINK_PROVISION_SPAN,
};

use crate::rest_api_lifecycle::{assert_failure, string_member, Answer, Gateway};

/// Slack added to a stored span before asserting the state it is due in, so a loaded
/// machine does not read the record one tick early.
const MARGIN: Duration = Duration::from_millis(100);

/// A network-load-balancer ARN of the shape `targetArns` documents ("The ARN of the
/// network load balancer of the VPC targeted by the VPC link"), numbered so a row can
/// name several distinct targets.
///
/// `pub(crate)` so a sibling suite names a link's targets the one way rather than keeping
/// its own copy (the `update_vpc_link` suite does).
pub(crate) fn target_arn(index: usize) -> String {
    format!(
        "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/nlb-{index}/\
         abc123def456789{index}"
    )
}

/// `CreateVpcLink` with `request`, answering the whole response.
pub(crate) async fn create_vpc_link(gateway: &Gateway, request: Value) -> Answer {
    gateway.call("POST", "/vpclinks", Some(request)).await
}

/// A `VpcLink` body without its `status`: the members a create and a read must agree on
/// member for member, where `status` is the one the clock moves on its own.
fn without_status(body: &Value) -> Value {
    let mut without = body.clone();
    if let Some(members) = without.as_object_mut() {
        members.remove("status");
    }
    without
}

/// `GetVpcLink` for `vpc_link_id`.
pub(crate) async fn get_vpc_link(gateway: &Gateway, vpc_link_id: &str) -> Answer {
    gateway
        .call("GET", &format!("/vpclinks/{vpc_link_id}"), None)
        .await
}

/// A VPC link created for `name` with one target, asserting the modelled 202 and
/// answering its id.
///
/// Shared with the `resource_tagging` suite, which tags a stored link, so both suites
/// create one the same way.
pub(crate) async fn created_vpc_link(gateway: &Gateway, name: &str) -> String {
    let created = create_vpc_link(
        gateway,
        json!({ "name": name, "targetArns": [target_arn(0)] }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case CreateVpcLink({name}): expected 202, got {} with {:?}",
        created.status, created.body
    );
    string_member(&created.body, "id", &format!("CreateVpcLink({name})"))
}

#[tokio::test]
async fn a_created_vpc_link_is_persisted_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: the number of targets, which
    // optional members the request carries, how many tags it sets, and the form of the
    // name.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one target and nothing else",
            json!({ "name": "bare", "targetArns": [target_arn(0)] }),
        ),
        (
            "three targets",
            json!({
                "name": "three-targets",
                "targetArns": [target_arn(0), target_arn(1), target_arn(2)],
            }),
        ),
        (
            "described",
            json!({
                "name": "described",
                "description": "the private link",
                "targetArns": [target_arn(1)],
            }),
        ),
        (
            "tagged twice",
            json!({
                "name": "tagged link 2",
                "targetArns": [target_arn(2)],
                "tags": { "team": "api", "env": "test" },
            }),
        ),
        (
            "a target of another vendor",
            json!({
                "name": "vpce-target",
                "targetArns": ["arn:aws:ec2:us-east-1:000000000000:vpc-endpoint-service/vpce-svc-1"],
            }),
        ),
    ];

    let gateway = Gateway::new();
    let mut minted: Vec<String> = Vec::new();
    for (case, request) in &rows {
        let created = create_vpc_link(&gateway, request.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case}: expected 202 from CreateVpcLink, got {} with {:?}",
            created.status, created.body
        );
        let id = string_member(&created.body, "id", case);
        assert!(
            !minted.contains(&id),
            "case {case}: every link gets its own id, got {id:?} twice in {minted:?}"
        );
        minted.push(id.clone());

        // Every member the request set is reported as it set it, and the members it did
        // not set are absent — `description` and `tags` among them.
        for member in ["name", "description"] {
            assert_eq!(
                created.body[member], request[member],
                "case {case}: expected {member} {:?}, got {:?}",
                request[member], created.body[member]
            );
        }
        assert_eq!(
            created.body["targetArns"], request["targetArns"],
            "case {case}: expected the targets back in order, got {:?}",
            created.body["targetArns"]
        );
        assert_eq!(
            created.body["tags"], request["tags"],
            "case {case}: expected the tags back, got {:?}",
            created.body["tags"]
        );

        // The READ reports the same record: one stored link, not a per-call answer.
        // `status` is left out of the comparison and asserted separately, because it is
        // the one member the CLOCK moves — the provisioning span may pass between these
        // two calls, and which of the two modelled values it reports is the dedicated
        // transition test's rule, not this one's.
        let read = get_vpc_link(&gateway, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetVpcLink, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            without_status(&read.body),
            without_status(&created.body),
            "case {case}: GetVpcLink must report the created link, got {:?} after {:?}",
            read.body,
            created.body
        );
        for (whose, body) in [("CreateVpcLink", &created.body), ("GetVpcLink", &read.body)] {
            let status = body["status"].as_str();
            assert!(
                status == Some(VPC_LINK_PENDING) || status == Some(VPC_LINK_AVAILABLE),
                "case {case}: {whose} must report a modelled provisioning status, \
                 got {status:?}"
            );
        }
    }

    // Every link created above is still readable: a create does not displace the links
    // before it, and a read does not consume the link it reports.
    for id in &minted {
        let read = get_vpc_link(&gateway, id).await;
        assert_eq!(
            read.status, 200,
            "case GetVpcLink({id}) at the end: expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            string_member(&read.body, "id", "GetVpcLink at the end"),
            *id,
            "case GetVpcLink({id}): a link reports its own id, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_new_vpc_link_is_pending_until_the_provisioning_span_has_passed() {
    let gateway = Gateway::new();
    // The clock the assertions below are read against: a status measured while the call
    // demonstrably landed INSIDE the provisioning span must be `PENDING`, and one
    // measured after a machine slow enough to cross the span may already be `AVAILABLE`
    // — so the test holds each answer to what the clock owed when it was measured
    // instead of to how fast the two calls ran.
    let started = std::time::Instant::now();
    let created = create_vpc_link(
        &gateway,
        json!({ "name": "provisioning", "targetArns": [target_arn(0)] }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case CreateVpcLink: expected 202, got {} with {:?}",
        created.status, created.body
    );
    let id = string_member(&created.body, "id", "CreateVpcLink(provisioning)");
    // A caller reading straight after the call still sees the link being provisioned —
    // in the create's own answer and in a read that follows it.
    assert_owed_status(&created, started.elapsed(), "CreateVpcLink");
    let read = get_vpc_link(&gateway, &id).await;
    assert_owed_status(&read, started.elapsed(), "GetVpcLink within the span");

    // The transition is one the clock owes: once the span has passed the link is
    // available — for every reader, not just the first, because nothing about the read
    // moves it.
    tokio::time::sleep(VPC_LINK_PROVISION_SPAN + MARGIN).await;
    for round in 0..2 {
        let read = get_vpc_link(&gateway, &id).await;
        assert_eq!(
            read.body["status"].as_str(),
            Some(VPC_LINK_AVAILABLE),
            "case GetVpcLink(round {round}) after the span: expected \
             {VPC_LINK_AVAILABLE}, got {:?}",
            read.body["status"]
        );
    }
}

/// Assert `answer`'s `status` is the one the clock owed a link created `elapsed` ago:
/// `PENDING` while the provisioning span has demonstrably not passed, and either value
/// once it may have — never a value outside `VpcLinkStatus`'s two provisioning states.
fn assert_owed_status(answer: &Answer, elapsed: Duration, case: &str) {
    let owed: &[&str] = if elapsed < VPC_LINK_PROVISION_SPAN {
        &[VPC_LINK_PENDING]
    } else {
        &[VPC_LINK_PENDING, VPC_LINK_AVAILABLE]
    };
    let status = answer.body["status"].as_str();
    assert!(
        status.is_some_and(|status| owed.contains(&status)),
        "case {case}: expected one of {owed:?} {elapsed:?} after the create \
         (span {VPC_LINK_PROVISION_SPAN:?}), got {:?}",
        answer.body["status"]
    );
}

#[tokio::test]
async fn an_id_no_stored_link_answers_to_is_the_modelled_not_found() {
    let gateway = Gateway::new();
    // One stored link, so the refusals below are about the id and not about an empty
    // store.
    let stored = created_vpc_link(&gateway, "the-stored-one").await;

    // Rows differ along the form of the id: an id of the shape this service mints, a
    // longer and a shorter one, an id differing from the stored one in one character,
    // the link's NAME rather than its id, and an id in another case.
    let mut rows: Vec<(String, String)> = vec![
        ("an unminted id".to_string(), "zzzzz99999".to_string()),
        ("a short id".to_string(), "abc".to_string()),
        (
            "a long id".to_string(),
            "abcdefghij0123456789".to_string(),
        ),
        (
            "the link's name, not its id".to_string(),
            "the-stored-one".to_string(),
        ),
        (
            "the stored id upper-cased".to_string(),
            stored.to_uppercase(),
        ),
    ];
    // The stored id with its last character stepped on: as close to a real id as a wrong
    // id gets.
    let mut near_miss: Vec<char> = stored.chars().collect();
    if let Some(last) = near_miss.pop() {
        near_miss.push(if last == 'a' { 'b' } else { 'a' });
    }
    rows.push((
        "the stored id with one character changed".to_string(),
        near_miss.into_iter().collect(),
    ));

    for (case, id) in &rows {
        assert_failure(
            &get_vpc_link(&gateway, id).await,
            404,
            "NotFoundException",
            case,
        );
    }

    // The stored link is untouched by every refusal above.
    let read = get_vpc_link(&gateway, &stored).await;
    assert_eq!(
        read.status, 200,
        "case GetVpcLink(the stored link): expected 200, got {} with {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn a_request_outside_what_the_service_stores_is_the_modelled_bad_request() {
    // Rows differ along which part of the request falls outside the rule: the member
    // `targetArns` documents as naming load balancers must name at least one, and each
    // entry must be an ARN; a name must not be empty.
    let rows: Vec<(&str, Value)> = vec![
        (
            "no target at all",
            json!({ "name": "no-targets", "targetArns": [] }),
        ),
        (
            "a target that is not an ARN",
            json!({ "name": "bad-target", "targetArns": ["not-an-arn"] }),
        ),
        (
            "a target missing the ARN fields",
            json!({ "name": "short-arn", "targetArns": ["arn:aws:elasticloadbalancing"] }),
        ),
        (
            "one good target and one bad one",
            json!({
                "name": "mixed-targets",
                "targetArns": [target_arn(0), "arn::::"],
            }),
        ),
        (
            "an empty name",
            json!({ "name": "", "targetArns": [target_arn(0)] }),
        ),
        (
            "a tag key in the reserved namespace",
            json!({
                "name": "reserved-tag",
                "targetArns": [target_arn(0)],
                "tags": { "aws:owner": "someone" },
            }),
        ),
    ];

    let gateway = Gateway::new();
    for (case, request) in &rows {
        assert_failure(
            &create_vpc_link(&gateway, request.clone()).await,
            400,
            "BadRequestException",
            case,
        );
    }

    // Nothing above was stored: the first link this account can read is the one the next
    // accepted request creates, and it is readable.
    let id = created_vpc_link(&gateway, "after-the-refusals").await;
    let read = get_vpc_link(&gateway, &id).await;
    assert_eq!(
        read.status, 200,
        "case GetVpcLink after the refusals: expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.body["name"].as_str(),
        Some("after-the-refusals"),
        "case GetVpcLink after the refusals: expected the accepted link, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn a_create_past_the_accounts_link_quota_is_the_modelled_limit_exceeded() {
    // The growth a caller drives is bounded at API Gateway's published quota for VPC
    // links in one Region (`MAX_VPC_LINKS`): every create up to it is accepted, the one
    // past it is the modelled `LimitExceededException` (429), and the links already
    // stored are untouched by the refusal.
    let gateway = Gateway::new();
    let mut stored: Vec<String> = Vec::new();
    for index in 0..MAX_VPC_LINKS {
        let created = create_vpc_link(
            &gateway,
            json!({ "name": format!("link-{index}"), "targetArns": [target_arn(index)] }),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case CreateVpcLink({index}) within the quota: expected 202, got {} with {:?}",
            created.status, created.body
        );
        stored.push(string_member(
            &created.body,
            "id",
            &format!("CreateVpcLink({index})"),
        ));
    }

    assert_failure(
        &create_vpc_link(
            &gateway,
            json!({ "name": "one-too-many", "targetArns": [target_arn(0)] }),
        )
        .await,
        429,
        "LimitExceededException",
        &format!("CreateVpcLink past the quota of {MAX_VPC_LINKS}"),
    );

    for id in &stored {
        let read = get_vpc_link(&gateway, id).await;
        assert_eq!(
            read.status, 200,
            "case GetVpcLink({id}) after the refused create: expected 200, got {} with {:?}",
            read.status, read.body
        );
    }
}
