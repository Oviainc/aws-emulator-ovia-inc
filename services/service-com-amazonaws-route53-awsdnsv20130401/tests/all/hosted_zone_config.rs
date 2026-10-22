// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the `HostedZoneConfig` a hosted-zone read reports,
//! and of the `UpdateHostedZoneComment` write a later read has to reflect.
//!
//! Two rules are under test:
//!
//! 1. Every response that carries a `HostedZone` carries its `Config`
//!    container, whose `PrivateZone` is the zone's private flag (`false` for a
//!    public zone, since `HostedZoneConfig$PrivateZone` is a boolean with
//!    `@default false`) and whose `Comment` appears only when the zone holds
//!    one. So the table below differs along the private flag, along whether a
//!    comment was given and along its length — and every row is asserted on
//!    `CreateHostedZone`, on `GetHostedZone` AND on `ListHostedZones`, because
//!    the rule is about the projection, not about one operation.
//! 2. A zone's comment is state: `UpdateHostedZoneComment` writes it (and an
//!    absent `Comment` clears it, per the operation's own documentation), and
//!    the next `GetHostedZone` reports what the last accepted update left.
//! 3. `GetHostedZone` reports the zone's deletion protection alongside it, read
//!    off the zone — `false` for every zone here, because this API version
//!    carries no operation that turns it on.
//!
//! Every test drives the service through the real router, using the SAME
//! harness `hosted_zone_records` uses, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, escape_xml, router, Reply};
use crate::resource_tags::element_text;

/// `ResourceDescription`, which `HostedZoneConfig$Comment` and
/// `UpdateHostedZoneCommentRequest$Comment` both target, is `len 0..256`
/// (smithy_query(query="shape",shape="ResourceDescription")).
const COMMENT_MAX_LEN: usize = 256;

/// A `CreateHostedZone` body with a caller-chosen `HostedZoneConfig` block.
fn create_zone_body(name: &str, caller_reference: &str, extra: &str) -> String {
    format!(
        "<CreateHostedZoneRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         <Name>{name}</Name><CallerReference>{caller_reference}</CallerReference>{extra}\
         </CreateHostedZoneRequest>"
    )
}

/// Create a zone and return the reply.
async fn create_zone(
    router: &MultiServiceRouter,
    name: &str,
    caller_reference: &str,
    extra: &str,
) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/hostedzone",
        &create_zone_body(name, caller_reference, extra),
    )
    .await
}

/// Read a zone back.
async fn get_zone(router: &MultiServiceRouter, zone_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}"),
        "",
    )
    .await
}

/// Update a zone's comment. `None` sends NO `Comment` element at all.
async fn update_comment(
    router: &MultiServiceRouter,
    zone_id: &str,
    comment: Option<&str>,
) -> Reply {
    let comment = comment.map_or_else(String::new, |text| {
        format!("<Comment>{}</Comment>", escape_xml(text))
    });
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}"),
        &format!(
            "<UpdateHostedZoneCommentRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">{comment}\
             </UpdateHostedZoneCommentRequest>"
        ),
    )
    .await
}

/// The bare (`Z…`) id from a reply carrying a `HostedZone`.
fn bare_zone_id(reply: &Reply) -> String {
    let id = reply
        .first("HostedZone/Id")
        .unwrap_or_else(|| panic!("the reply must carry HostedZone/Id; body {}", reply.body));
    id.trim_start_matches("/hostedzone/").to_string()
}

/// The `<Config>` block of the first `HostedZone` in a reply, or `None` when the
/// reply carries no `Config` element at all.
///
/// Read out of the raw body rather than out of the flattened document, because
/// the question this file asks is whether the CONTAINER is there — and a
/// container whose only child is an empty `Comment` carries no text to flatten.
fn config_block(reply: &Reply) -> Option<&str> {
    if reply.body.contains("<Config/>") {
        return Some("");
    }
    let start = reply.body.find("<Config>")? + "<Config>".len();
    let end = reply.body[start..].find("</Config>")? + start;
    Some(&reply.body[start..end])
}

/// `(PrivateZone, Comment)` as the first `HostedZone`'s `Config` reports them.
/// Panics when the `Config` container is missing, which is itself the defect
/// this file pins.
fn config_of(reply: &Reply, case: &str) -> (String, Option<String>) {
    let block = config_block(reply).unwrap_or_else(|| {
        panic!(
            "case {case:?}: every HostedZone must carry a Config container; body {}",
            reply.body
        )
    });
    let private_zone = element_text(block, "PrivateZone").unwrap_or_else(|| {
        panic!(
            "case {case:?}: Config must carry PrivateZone; body {}",
            reply.body
        )
    });
    (private_zone, element_text(block, "Comment"))
}

// ---------------------------------------------------------------------------
// HostedZone.Config on every read
// ---------------------------------------------------------------------------

/// One row of the config table: how the zone is created, and the `Config` every
/// read of it must report.
struct ConfigRow {
    case: &'static str,
    name: &'static str,
    /// The `HostedZoneConfig` (and, for a private zone, `VPC`) block sent.
    extra: String,
    expect_private: &'static str,
    expect_comment: Option<String>,
}

#[tokio::test]
async fn every_hosted_zone_read_reports_config_with_the_private_flag() {
    let long_comment = "c".repeat(COMMENT_MAX_LEN);
    let rows = [
        ConfigRow {
            case: "a public zone created with no HostedZoneConfig at all",
            name: "no-config.example.com",
            extra: String::new(),
            expect_private: "false",
            expect_comment: None,
        },
        ConfigRow {
            case: "a public zone created with an empty HostedZoneConfig",
            name: "empty-config.example.com",
            extra: "<HostedZoneConfig></HostedZoneConfig>".to_string(),
            expect_private: "false",
            expect_comment: None,
        },
        ConfigRow {
            case: "a public zone created with a comment",
            name: "commented.example.com",
            extra: "<HostedZoneConfig><Comment>initial comment</Comment></HostedZoneConfig>"
                .to_string(),
            expect_private: "false",
            expect_comment: Some("initial comment".to_string()),
        },
        ConfigRow {
            case: "a comment at the 256-character maximum",
            name: "long-comment.example.net",
            extra: format!("<HostedZoneConfig><Comment>{long_comment}</Comment></HostedZoneConfig>"),
            expect_private: "false",
            expect_comment: Some(long_comment.clone()),
        },
        ConfigRow {
            case: "a comment carrying XML special characters",
            name: "escaped.example.org",
            extra: format!(
                "<HostedZoneConfig><Comment>{}</Comment></HostedZoneConfig>",
                escape_xml("a&b<c>d\"e'f")
            ),
            expect_private: "false",
            expect_comment: Some("a&b<c>d\"e'f".to_string()),
        },
        ConfigRow {
            case: "PrivateZone sent explicitly false alongside a comment",
            name: "explicit-public.example.com",
            extra: "<HostedZoneConfig><Comment>explicitly public</Comment>\
                    <PrivateZone>false</PrivateZone></HostedZoneConfig>"
                .to_string(),
            expect_private: "false",
            expect_comment: Some("explicitly public".to_string()),
        },
        ConfigRow {
            case: "a private zone with no comment",
            name: "internal.example.co.uk",
            extra: "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>\
                    <VPC><VPCRegion>us-east-1</VPCRegion><VPCId>vpc-1a2b3c4d</VPCId></VPC>"
                .to_string(),
            expect_private: "true",
            expect_comment: None,
        },
        ConfigRow {
            case: "a private zone with a comment",
            name: "internal-commented.example.com",
            extra: "<HostedZoneConfig><Comment>private and commented</Comment>\
                    <PrivateZone>true</PrivateZone></HostedZoneConfig>\
                    <VPC><VPCRegion>eu-west-1</VPCRegion><VPCId>vpc-9f8e7d6c</VPCId></VPC>"
                .to_string(),
            expect_private: "true",
            expect_comment: Some("private and commented".to_string()),
        },
    ];

    let router = router();
    // Every row's expected PrivateZone, in the order ListHostedZones reports
    // them — which is zone-id order, so the multiset is what can be compared.
    let mut expected_private: Vec<&str> = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let created = create_zone(&router, row.name, &format!("config-ref-{index}"), &row.extra)
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHostedZone, got {} with body {}",
            row.case, created.status, created.body
        );
        let created_config = config_of(&created, row.case);
        assert_eq!(
            created_config,
            (row.expect_private.to_string(), row.expect_comment.clone()),
            "case {:?}: CreateHostedZone must report (PrivateZone, Comment) as \
             ({:?}, {:?}), got {:?}; body {}",
            row.case,
            row.expect_private,
            row.expect_comment,
            created_config,
            created.body
        );

        let zone_id = bare_zone_id(&created);
        let read = get_zone(&router, &zone_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetHostedZone, got {} with body {}",
            row.case, read.status, read.body
        );
        let read_config = config_of(&read, row.case);
        assert_eq!(
            read_config, created_config,
            "case {:?}: GetHostedZone must report the same Config the create did, \
             got {:?} after {:?}; body {}",
            row.case, read_config, created_config, read.body
        );
        // The read also reports the zone's deletion protection, which no
        // operation of this API version turns on, so every zone reads `false`.
        assert_eq!(
            read.first("HostedZoneDeletionProtectionEnabled"),
            Some("false"),
            "case {:?}: GetHostedZone must report HostedZoneDeletionProtectionEnabled, \
             got {:?}; body {}",
            row.case,
            read.first("HostedZoneDeletionProtectionEnabled"),
            read.body
        );

        expected_private.push(row.expect_private);
    }

    // The same projection on the LIST path: one Config per listed zone.
    let listed = call(&router, "GET", "/2013-04-01/hostedzone", "").await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListHostedZones, got {} with body {}",
        listed.status, listed.body
    );
    let mut listed_private: Vec<String> = listed
        .all("HostedZone/Config/PrivateZone")
        .into_iter()
        .map(str::to_string)
        .collect();
    listed_private.sort();
    let mut expected_private: Vec<String> =
        expected_private.iter().map(|flag| (*flag).to_string()).collect();
    expected_private.sort();
    assert_eq!(
        listed_private, expected_private,
        "ListHostedZones must report one Config/PrivateZone per zone; body {}",
        listed.body
    );
}

// ---------------------------------------------------------------------------
// UpdateHostedZoneComment
// ---------------------------------------------------------------------------

/// One row of the comment-update table: the `Comment` sent, the status it must
/// answer with, and the comment a later read must report.
struct CommentRow {
    case: &'static str,
    /// `None` sends no `Comment` element at all.
    sent: Option<String>,
    status: u16,
    /// What `GetHostedZone` must report for `Config/Comment` afterwards.
    expect_comment: Option<String>,
}

#[tokio::test]
async fn a_zone_comment_is_written_by_the_update_and_read_back() {
    let at_bound = "u".repeat(COMMENT_MAX_LEN);
    let past_bound = "u".repeat(COMMENT_MAX_LEN + 1);
    let rows = [
        CommentRow {
            case: "a new comment replaces the one the zone was created with",
            sent: Some("updated comment".to_string()),
            status: 200,
            expect_comment: Some("updated comment".to_string()),
        },
        CommentRow {
            case: "a comment carrying XML special characters",
            sent: Some("a&b<c>d\"e'f".to_string()),
            status: 200,
            expect_comment: Some("a&b<c>d\"e'f".to_string()),
        },
        CommentRow {
            case: "a comment at the 256-character maximum",
            sent: Some(at_bound.clone()),
            status: 200,
            expect_comment: Some(at_bound.clone()),
        },
        CommentRow {
            // `ResourceDescription` is `len 0..256`, so one character past the
            // maximum is refused and the stored comment is left as it was.
            case: "a comment one character past the maximum",
            sent: Some(past_bound),
            status: 400,
            expect_comment: Some(at_bound.clone()),
        },
        CommentRow {
            // "If you don't specify a value for Comment, Amazon Route 53
            // deletes the existing value of the Comment element, if any."
            case: "an update with no Comment clears the comment",
            sent: None,
            status: 200,
            expect_comment: None,
        },
        CommentRow {
            case: "a comment can be set again after being cleared",
            sent: Some("set again".to_string()),
            status: 200,
            expect_comment: Some("set again".to_string()),
        },
    ];

    let router = router();
    let created = create_zone(
        &router,
        "comment.example.com",
        "comment-updates",
        "<HostedZoneConfig><Comment>initial comment</Comment></HostedZoneConfig>",
    )
    .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateHostedZone, got {} with body {}",
        created.status, created.body
    );
    let zone_id = bare_zone_id(&created);

    for row in &rows {
        let updated = update_comment(&router, &zone_id, row.sent.as_deref()).await;
        assert_eq!(
            updated.status, row.status,
            "case {:?}: expected HTTP {} from UpdateHostedZoneComment, got {} with body {}",
            row.case, row.status, updated.status, updated.body
        );
        if row.status == 200 {
            // The update answers with the zone it just wrote.
            let answered = config_of(&updated, row.case);
            assert_eq!(
                answered,
                ("false".to_string(), row.expect_comment.clone()),
                "case {:?}: UpdateHostedZoneComment must answer with the comment it \
                 wrote, got {:?}; body {}",
                row.case,
                answered,
                updated.body
            );
        }

        let read = get_zone(&router, &zone_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetHostedZone, got {} with body {}",
            row.case, read.status, read.body
        );
        let (private_zone, comment) = config_of(&read, row.case);
        assert_eq!(
            private_zone, "false",
            "case {:?}: an update must not change the private flag; body {}",
            row.case, read.body
        );
        assert_eq!(
            comment, row.expect_comment,
            "case {:?}: expected Config/Comment {:?} after the update, got {:?}; body {}",
            row.case, row.expect_comment, comment, read.body
        );
    }
}

#[tokio::test]
async fn updating_the_comment_of_an_unknown_zone_is_refused() {
    // The rule: the operation resolves its zone first, so an id no zone carries
    // is the modelled NoSuchHostedZone — never a silent success.
    let router = router();
    let reply = update_comment(&router, "ZNOSUCHZONE0001", Some("nobody's comment")).await;
    assert_eq!(
        reply.status, 404,
        "expected 404 for an unknown zone, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "NoSuchHostedZone",
        "expected NoSuchHostedZone, got body {}",
        reply.body
    );
}

#[tokio::test]
async fn a_comment_update_touches_only_the_zone_it_names() {
    // The rule: the comment is held BY the zone, so updating one zone's comment
    // leaves every other zone's alone.
    let router = router();
    let mut zones = Vec::new();
    for (index, name) in ["first.example.com", "second.example.com"]
        .into_iter()
        .enumerate()
    {
        let created = create_zone(
            &router,
            name,
            &format!("comment-owner-{index}"),
            &format!("<HostedZoneConfig><Comment>comment {index}</Comment></HostedZoneConfig>"),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "creating {name}: expected 201, got {} with body {}",
            created.status, created.body
        );
        zones.push(bare_zone_id(&created));
    }

    let updated = update_comment(&router, &zones[0], Some("only the first")).await;
    assert_eq!(
        updated.status, 200,
        "expected 200 from UpdateHostedZoneComment, got {} with body {}",
        updated.status, updated.body
    );

    for (zone, expected) in [(&zones[0], "only the first"), (&zones[1], "comment 1")] {
        let read = get_zone(&router, zone).await;
        let (_, comment) = config_of(&read, zone);
        assert_eq!(
            comment.as_deref(),
            Some(expected),
            "zone {zone}: expected Config/Comment {expected:?}, got {comment:?}; body {}",
            read.body
        );
    }
}
