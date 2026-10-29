// SPDX-License-Identifier: CC0-1.0

//! What an explicit `Deny` in an origin endpoint's resource policy does to an anonymous
//! egress fetch when the statement is written in a form the emulator's evaluator cannot
//! fully judge.
//!
//! The rule: a `Deny` statement applies to a request unless a member the evaluator CAN
//! read rules the request out of it — an `Action` list that does not name the action the
//! method needs, a `Principal` naming someone other than everyone, or a `Resource` list no
//! pattern of which covers the addressed resource. A dimension stated only in its inverted
//! form (`NotAction`, `NotPrincipal`, `NotResource`), narrowed by a `Condition`, left
//! unstated, or written in a shape the evaluator cannot read is judged by nothing and so
//! rules nothing out: the `Deny` still refuses the fetch `403`. That is the same
//! fail-closed reading the `Allow` side already has, where an unjudgeable member withholds
//! the grant instead of widening it.
//!
//! `rejection_rule_and_negative_paths.rs` (rule 6) pins the `Allow` side of that reading
//! and the `Deny` side in its PLAIN form only. This suite is the `Deny` side along the axis
//! that row fixes: which dimension is inverted, how many of them are, whether a judgeable
//! member sits beside the inverted one, and the method the fetch uses (with the action it
//! is authorized against). Every row attaches the same sibling grant to everyone, so a
//! `403` is attributable to the `Deny` alone.
//!
//! The client, the URL splitter and the fixtures are the ones
//! `rejection_rule_and_negative_paths` already owns, reached through `pub(crate)`.

use serde_json::{json, Value};

use crate::rejection_rule_and_negative_paths::{split_url, Emulator};

/// The resources every row shares. Nothing in the rule depends on their names.
const GROUP: &str = "deny-group";
const CHANNEL: &str = "deny-channel";
const ENDPOINT: &str = "deny-ep";
/// A second origin endpoint, created only so a row can name a REAL ARN of a resource the
/// fetch does not address.
const OTHER_ENDPOINT: &str = "deny-other-ep";

/// One named principal, for the rows that aim a `Deny` at somebody in particular or
/// exclude somebody in particular with `NotPrincipal`.
const CDN_ROLE: &str = "arn:aws:iam::123456789012:role/cdn";

/// The methods the egress data plane serves, each with the action it is authorized
/// against. Source: the egress seam reads the action from the request's method
/// (`crate::service::egress_action`) — `mediapackagev2:GetObject` for a `GET` and
/// `mediapackagev2:GetHeadObject` for a `HEAD`.
const METHODS: &[(&str, &str)] = &[
    ("GET", "mediapackagev2:GetObject"),
    ("HEAD", "mediapackagev2:GetHeadObject"),
];

/// The envelope a data-plane refusal with no grant behind it carries: the `code` and the
/// one `message` sentence `crate::service` answers with, which carries nothing from the
/// request.
const ACCESS_DENIED_CODE: &str = "AccessDenied";
const ACCESS_DENIED_MESSAGE: &str = "Access denied";

/// What a row's `Deny` statement is written against: the action this request's method
/// needs, another action of the same service, the ARN of the endpoint the fetched URL
/// names, and the ARN of an endpoint it does not.
struct Subject<'a> {
    action: &'a str,
    other_action: &'a str,
    arn: &'a str,
    other_arn: &'a str,
}

/// One row: a `Deny` statement, and whether it applies to the fetch.
struct DenyRow {
    label: &'static str,
    deny: fn(&Subject<'_>) -> Value,
    /// `true` when the statement applies, so the fetch is refused `403`; `false` when a
    /// member the evaluator reads rules the request out of it, so the sibling grant stands
    /// and the fetch reaches the (empty) object lookup.
    applies: bool,
}

/// The statement forms a `Deny` is written in, and what each means for an anonymous fetch.
///
/// The first group carries a dimension nothing can judge — inverted, conditional, absent
/// or unreadable — and therefore denies; the second group carries a judgeable member that
/// puts this request outside the statement, and therefore does not.
const DENY_ROWS: &[DenyRow] = &[
    DenyRow {
        label: "NotPrincipal: everyone except one role",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": CDN_ROLE},
                "Action": subject.action,
            })
        },
        applies: true,
    },
    DenyRow {
        label: "NotAction: every action except another one",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": "*",
                "NotAction": subject.other_action,
            })
        },
        applies: true,
    },
    DenyRow {
        label: "NotResource alone, naming another endpoint",
        deny: |subject| json!({"Effect": "Deny", "NotResource": subject.other_arn}),
        applies: true,
    },
    DenyRow {
        label: "a Condition-qualified Deny of this action",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": subject.action,
                "Condition": {"IpAddress": {"aws:SourceIp": "203.0.113.0/24"}},
            })
        },
        applies: true,
    },
    DenyRow {
        label: "every dimension inverted at once",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": CDN_ROLE},
                "NotAction": subject.other_action,
                "NotResource": subject.other_arn,
            })
        },
        applies: true,
    },
    DenyRow {
        label: "NotPrincipal beside a Resource that does cover this endpoint",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": CDN_ROLE},
                "Action": subject.action,
                "Resource": subject.arn,
            })
        },
        applies: true,
    },
    DenyRow {
        label: "NotAction beside an Action list that does name this action",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": [subject.other_action, subject.action],
                "NotAction": subject.other_action,
            })
        },
        applies: true,
    },
    DenyRow {
        label: "a Deny whose Action is not written as a name at all",
        deny: |_subject| {
            json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": CDN_ROLE},
                "Action": 17,
            })
        },
        applies: true,
    },
    DenyRow {
        label: "a Deny whose Action list carries an entry that is not a name",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": CDN_ROLE},
                "Action": [subject.other_action, 17],
            })
        },
        applies: true,
    },
    DenyRow {
        label: "a Deny whose Principal carries a value that is not a principal",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": {"AWS": 17},
                "Action": subject.action,
            })
        },
        applies: true,
    },
    DenyRow {
        label: "a Deny whose Resource list carries an entry that is not a pattern",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": subject.action,
                "Resource": [subject.other_arn, 17],
            })
        },
        applies: true,
    },
    DenyRow {
        label: "a Deny that states no dimension but its effect",
        deny: |_subject| json!({"Effect": "Deny"}),
        applies: true,
    },
    DenyRow {
        label: "a Principal list that includes everyone",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": {"AWS": [CDN_ROLE, "*"]},
                "Action": subject.action,
            })
        },
        applies: true,
    },
    DenyRow {
        label: "a plain Deny of this action to everyone",
        deny: |subject| {
            json!({"Effect": "Deny", "Principal": "*", "Action": subject.action})
        },
        applies: true,
    },
    DenyRow {
        label: "a Deny of another action only",
        deny: |subject| {
            json!({"Effect": "Deny", "Principal": "*", "Action": subject.other_action})
        },
        applies: false,
    },
    DenyRow {
        label: "a Deny aimed at one named principal",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": {"AWS": CDN_ROLE},
                "Action": subject.action,
            })
        },
        applies: false,
    },
    DenyRow {
        label: "a Deny scoped by Resource to another endpoint",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": subject.action,
                "Resource": subject.other_arn,
            })
        },
        applies: false,
    },
    DenyRow {
        label: "NotPrincipal, but an Action naming another action",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": CDN_ROLE},
                "Action": subject.other_action,
            })
        },
        applies: false,
    },
    DenyRow {
        label: "NotAction, but a Resource scoped to another endpoint",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": "*",
                "NotAction": subject.other_action,
                "Resource": subject.other_arn,
            })
        },
        applies: false,
    },
    DenyRow {
        label: "NotResource, but a Principal naming one role",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": {"AWS": CDN_ROLE},
                "NotResource": subject.other_arn,
            })
        },
        applies: false,
    },
    DenyRow {
        label: "an Action list naming only other actions",
        deny: |subject| {
            json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": [subject.other_action, "s3:GetObject"],
            })
        },
        applies: false,
    },
];

/// The policy a row is driven with: a grant of `action` to everyone, plus the row's
/// `Deny`. The grant alone authorizes the fetch, so a refusal is the `Deny`'s doing.
fn policy_with(deny: Value, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Principal": "*", "Action": action},
            deny,
        ],
    })
    .to_string()
}

/// The published URL of an endpoint's one HLS manifest.
fn manifest_url(created: &Value) -> String {
    created
        .get("HlsManifests")
        .and_then(Value::as_array)
        .and_then(|manifests| manifests.first())
        .and_then(|manifest| manifest.get("Url"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an endpoint must publish a manifest URL, got {created}"))
        .to_string()
}

/// Create the group, the channel and the two endpoints, and report the fetched endpoint's
/// manifest URL and ARN plus the other endpoint's ARN.
async fn fixtures(emulator: &Emulator) -> (String, String, String) {
    let group = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        group.status, 200,
        "the channel group must be created, got {} with body {}",
        group.status, group.body
    );
    let channel = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    assert_eq!(
        channel.status, 200,
        "the channel must be created, got {} with body {}",
        channel.status, channel.body
    );
    let mut arns = Vec::new();
    let mut url = String::new();
    for name in [ENDPOINT, OTHER_ENDPOINT] {
        let created = emulator
            .create_endpoint(
                GROUP,
                CHANNEL,
                name,
                json!({"ContainerType": "TS", "HlsManifests": [{"ManifestName": "index"}]}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "the origin endpoint {name} must be created, got {} with body {}",
            created.status, created.body
        );
        if name == ENDPOINT {
            url = manifest_url(&created.json);
        }
        arns.push(created.string("Arn").to_string());
    }
    (url, arns[0].clone(), arns[1].clone())
}

/// The other action of the pair [`METHODS`] holds — the one this request does NOT need.
fn other_action_for(action: &str) -> &'static str {
    METHODS
        .iter()
        .map(|(_method, candidate)| *candidate)
        .find(|candidate| *candidate != action)
        .expect("the egress methods need more than one action between them")
}

/// A `Deny` whose statement the evaluator cannot fully judge still refuses the fetch; a
/// `Deny` a judgeable member puts this request outside of does not.
#[tokio::test]
async fn an_unjudgeable_deny_still_refuses_an_anonymous_fetch() {
    let emulator = Emulator::new();
    let (url, arn, other_arn) = fixtures(&emulator).await;
    let (host, path) = split_url(&url);
    let policy_path =
        format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/policy");

    for (method, action) in METHODS {
        let other_action = other_action_for(action);
        // The sibling grant on its own authorizes the fetch, so every refusal below is
        // the row's `Deny` and not a missing grant.
        let baseline = json!({
            "Statement": [{"Effect": "Allow", "Principal": "*", "Action": action}],
        })
        .to_string();
        let granted = emulator
            .call("POST", &policy_path, Some(json!({"Policy": baseline})))
            .await;
        assert_eq!(
            granted.status, 200,
            "case {method} baseline: the grant must be stored, got {} with body {}",
            granted.status, granted.body
        );
        let answer = emulator.send(method, &path, None, &host, false, &[]).await;
        assert_eq!(
            answer.status, 404,
            "case {method} baseline: a grant of {action} to everyone must authorize the fetch, \
             got {} with body {}",
            answer.status, answer.body
        );

        for row in DENY_ROWS {
            let label = row.label;
            let subject = Subject {
                action,
                other_action,
                arn: &arn,
                other_arn: &other_arn,
            };
            let policy = policy_with((row.deny)(&subject), action);
            let put = emulator
                .call("POST", &policy_path, Some(json!({"Policy": policy})))
                .await;
            assert_eq!(
                put.status, 200,
                "case {label:?} / {method}: the policy must be stored, got {} with body {}",
                put.status, put.body
            );
            let expected = if row.applies { 403 } else { 404 };
            let answer = emulator.send(method, &path, None, &host, false, &[]).await;
            assert_eq!(
                answer.status, expected,
                "case {label:?} / {method}: expected HTTP {expected} for an anonymous fetch \
                 under policy {policy}, got {} with body {}",
                answer.status, answer.body
            );
            // A `HEAD` answer carries no body to read, so the envelope is pinned on the
            // method that does: the data plane's own refusal, by code and by sentence.
            if row.applies && *method == "GET" {
                assert_eq!(
                    (answer.string("code"), answer.string("message")),
                    (ACCESS_DENIED_CODE, ACCESS_DENIED_MESSAGE),
                    "case {label:?} / {method}: the refusal must be the data plane's \
                     access-denied envelope, got {}",
                    answer.body
                );
            }
        }
    }
}

/// One row of the `Allow` side: a grant statement, and whether it authorizes the fetch.
struct GrantRow {
    label: &'static str,
    grant: fn(&Subject<'_>) -> Value,
    grants: bool,
}

/// The same ignorance read the other way round: a grant has to be stated in members the
/// evaluator reads in FULL, so a member written in a shape it cannot read withholds the
/// grant rather than widening it. The readable list forms are rows here too, so this table
/// is not all refusals.
const GRANT_ROWS: &[GrantRow] = &[
    GrantRow {
        label: "an Action list naming this action among others",
        grant: |subject| {
            json!({
                "Effect": "Allow",
                "Principal": "*",
                "Action": [subject.other_action, subject.action],
            })
        },
        grants: true,
    },
    GrantRow {
        label: "a Resource list covering this endpoint among others",
        grant: |subject| {
            json!({
                "Effect": "Allow",
                "Principal": "*",
                "Action": subject.action,
                "Resource": [subject.other_arn, subject.arn],
            })
        },
        grants: true,
    },
    GrantRow {
        label: "a grant whose Action is not written as a name at all",
        grant: |_subject| json!({"Effect": "Allow", "Principal": "*", "Action": 17}),
        grants: false,
    },
    GrantRow {
        label: "a grant whose Action list carries an entry that is not a name",
        grant: |subject| {
            json!({
                "Effect": "Allow",
                "Principal": "*",
                "Action": [subject.action, 17],
            })
        },
        grants: false,
    },
    GrantRow {
        label: "a grant whose Principal carries a value that is not a principal",
        grant: |subject| {
            json!({
                "Effect": "Allow",
                "Principal": {"AWS": 17},
                "Action": subject.action,
            })
        },
        grants: false,
    },
    GrantRow {
        label: "a grant whose Resource list carries an entry that is not a pattern",
        grant: |subject| {
            json!({
                "Effect": "Allow",
                "Principal": "*",
                "Action": subject.action,
                "Resource": [subject.arn, 17],
            })
        },
        grants: false,
    },
];

/// The `Allow` side of the same reading: a member stated in a shape the evaluator cannot
/// read in full withholds the grant, while the readable list forms still grant.
#[tokio::test]
async fn a_grant_the_evaluator_cannot_read_in_full_does_not_authorize_a_fetch() {
    let emulator = Emulator::new();
    let (url, arn, other_arn) = fixtures(&emulator).await;
    let (host, path) = split_url(&url);
    let policy_path =
        format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/policy");

    for (method, action) in METHODS {
        let other_action = other_action_for(action);
        for row in GRANT_ROWS {
            let label = row.label;
            let subject = Subject {
                action,
                other_action,
                arn: &arn,
                other_arn: &other_arn,
            };
            let policy = json!({"Statement": [(row.grant)(&subject)]}).to_string();
            let put = emulator
                .call("POST", &policy_path, Some(json!({"Policy": policy})))
                .await;
            assert_eq!(
                put.status, 200,
                "case {label:?} / {method}: the policy must be stored, got {} with body {}",
                put.status, put.body
            );
            let expected = if row.grants { 404 } else { 403 };
            let answer = emulator.send(method, &path, None, &host, false, &[]).await;
            assert_eq!(
                answer.status, expected,
                "case {label:?} / {method}: expected HTTP {expected} for an anonymous fetch \
                 under policy {policy}, got {} with body {}",
                answer.status, answer.body
            );
        }
    }
}
