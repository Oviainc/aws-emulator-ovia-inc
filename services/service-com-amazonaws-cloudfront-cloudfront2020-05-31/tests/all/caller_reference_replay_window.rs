// SPDX-License-Identifier: CC0-1.0

//! A `CallerReference`'s idempotent replay is an AS-CREATED privilege, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: "if `CallerReference` is a value that you already sent in a previous
//! request to create a distribution, CloudFront returns a `DistributionAlreadyExists` error"
//! (`smithy_query(query="doc",shape="DistributionConfig",member="CallerReference")`), and
//! what a repeat of THAT create is answered with instead is the distribution it already
//! made. So the replay reaches exactly the create the reference was sent with: while the
//! distribution still holds the configuration that create stored, a repeat of it answers with
//! that distribution's id; once the distribution has been reconfigured, no create under the
//! reference describes it any more and every create under it is refused 409
//! `DistributionAlreadyExists` — including one resending the distribution's own current
//! configuration, which is a configuration no create ever submitted.
//!
//! The rows span what the rule generalises over: what the update between the two creates
//! changed, and which configuration the second create resends (the original one or the
//! distribution's current one). Each row asserts the refused create left the distribution
//! exactly as the update left it.
//!
//! The wire harness, the configuration builder and the distribution helpers are the
//! lifecycle suite's, reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::distribution_already_exists_message;

use crate::distribution_lifecycle::{
    config, create_distribution, emulator, get_distribution, list_distributions,
    update_distribution, Answer, Config,
};

/// The reference every row below creates its distribution under.
const REFERENCE: &str = "replay-window-under-test";

/// How the update between the two creates changes the distribution, as `(case, the
/// configuration it writes)`.
///
/// The rows differ along the members an update can move: the comment, the enabled flag, the
/// price class, the origin set and the web ACL association.
fn updates() -> Vec<(&'static str, Config)> {
    let stored = config("the distribution under replay", REFERENCE);
    vec![
        (
            "a changed comment",
            Config {
                comment: "reconfigured".to_string(),
                ..stored.clone()
            },
        ),
        (
            "a disabled distribution",
            Config {
                enabled: !stored.enabled,
                ..stored.clone()
            },
        ),
        (
            "another price class",
            Config {
                price_class: Some("PriceClass_100"),
                ..stored.clone()
            },
        ),
        (
            "a second origin",
            Config {
                origins: vec![
                    stored.origins[0].clone(),
                    ("secondary".to_string(), "b.example.com".to_string()),
                ],
                ..stored.clone()
            },
        ),
        (
            "a web ACL association",
            Config {
                web_acl_id: Some("a1b2c3d4-5678-90ab-cdef-EXAMPLE11111".to_string()),
                ..stored.clone()
            },
        ),
    ]
}

/// Assert `answer` is the conflict this rule states, naming the distribution the reference is
/// associated with and nothing else of the request.
fn assert_conflict(answer: &Answer, case: &str, existing_id: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case}: expected HTTP 409, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "DistributionAlreadyExists",
        "case {case}: expected DistributionAlreadyExists, got {:?} in {}",
        answer.error_code(),
        answer.raw
    );
    let message = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message,
        distribution_already_exists_message(existing_id),
        "case {case}: expected the conflict to name the distribution already holding the \
         reference, got {message:?}"
    );
    assert!(
        !message.contains(REFERENCE),
        "case {case}: expected the reference itself not to be echoed back, got {message:?}"
    );
}

/// Before any update, a repeat of the create answers with the distribution it already made;
/// after one, every create under that reference is refused.
#[tokio::test]
async fn a_caller_reference_replays_only_the_create_it_was_sent_with() {
    for (case, updated) in updates() {
        let router = emulator();
        let stored = config("the distribution under replay", REFERENCE);

        let created = create_distribution(&router, &stored.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from the create, got {} with {}",
            created.status, created.raw
        );
        let id = created
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let etag = created.header("etag").to_string();

        // The create repeated verbatim is the SAME request again: the distribution it already
        // made, and no second one.
        let replay = create_distribution(&router, &stored.xml()).await;
        assert_eq!(
            replay.status, 201,
            "case {case}: expected the unmodified repeat to be answered, got {} with {}",
            replay.status, replay.raw
        );
        assert_eq!(
            replay.root().text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the repeat to answer with the distribution already created, \
             got {}",
            replay.raw
        );

        // A create under the taken reference carrying another configuration is the conflict
        // even before any update: it is no repeat of the create the reference was sent with.
        let diverged = Config {
            comment: "a different configuration".to_string(),
            ..stored.clone()
        };
        let conflict = create_distribution(&router, &diverged.xml()).await;
        assert_conflict(&conflict, case, &id);

        // Now the distribution is reconfigured, which is what spends the replay.
        let update = update_distribution(&router, &id, &updated.update_xml(), Some(&etag)).await;
        assert_eq!(
            update.status, 200,
            "case {case}: expected HTTP 200 from the update, got {} with {}",
            update.status, update.raw
        );

        for (resent, body) in [
            ("the original create", stored.xml()),
            ("the distribution's own current configuration", updated.update_xml()),
        ] {
            let after = create_distribution(&router, &body).await;
            assert_conflict(&after, &format!("{case}, resending {resent}"), &id);
        }

        // The refused creates changed nothing: the distribution is the one the update left,
        // and it is still the only one in the store.
        let read = get_distribution(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the distribution to still be readable, got {} with {}",
            read.status, read.raw
        );
        updated.assert_matches(
            read.root()
                .at("DistributionConfig")
                .unwrap_or_else(|| panic!("case {case}: the read carries no configuration")),
            "after the refused replays",
        );
        let page = list_distributions(&router, "").await;
        assert_eq!(
            page.root().text_at("Quantity"),
            Some("1"),
            "case {case}: expected the store to hold exactly the one distribution, got {}",
            page.raw
        );
    }
}
