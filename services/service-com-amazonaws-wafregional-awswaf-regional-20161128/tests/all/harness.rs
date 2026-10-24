// SPDX-License-Identifier: CC0-1.0

//! The `WebACL` test harness: the fixtures a web-ACL suite needs on top of the wire
//! harness in `wire.rs` (which owns the router, the awsJson1_1 call and the
//! change-token helpers every suite in this crate shares).
//!
//! AWS WAF Classic is past its end of support, so `CreateWebACL` refuses every request
//! (`web_acl_end_of_support.rs` pins that rule) and no call mints a web ACL any more.
//! The web ACLs that exist are the ones that already existed, so a suite needing one to
//! read, tag or reference puts the record in the store with [`store_web_acl`].

use std::collections::BTreeMap;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    service_state, web_acl_arn, WebACLEntry,
};

/// Store a web ACL that EXISTS: one of the web ACLs created before AWS WAF Classic
/// reached end of support, which is the only kind there is now.
///
/// The record goes in through the service's OWN state types — the id
/// `ComAmazonawsWafregionalAwswafRegional20161128State::mint_id` hands out, the ARN
/// `state::web_acl_arn` mints for `region` and the `WebACLEntry` every reader projects —
/// so what a suite then reads over the wire is a record indistinguishable from one this
/// service minted itself. Answers the stored `WebACLId`.
pub fn store_web_acl(
    engine: &engine::InMemoryEngine,
    region: &str,
    name: &str,
    metric_name: &str,
    default_action: &str,
    tags: &[(&str, &str)],
) -> String {
    let state = service_state(engine).expect("the service state is reachable from the engine");
    let mut state = state.lock();
    let web_acl_id = state.mint_id();
    let tags: BTreeMap<String, String> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    let entry = WebACLEntry::new(
        web_acl_id.clone(),
        name.to_string(),
        metric_name.to_string(),
        default_action.to_string(),
        web_acl_arn(region, &web_acl_id),
        tags,
    );
    state.web_acls.insert(web_acl_id.clone(), entry);
    web_acl_id
}

/// The ARN AWS WAF Classic's regional endpoints give a web ACL:
/// `arn:aws:waf-regional:<region>:<account>:webacl/<WebACLId>`. The account segment is
/// the emulator's canonical one, taken from the engine rather than written out.
pub fn expected_arn(region: &str, web_acl_id: &str) -> String {
    format!(
        "arn:aws:waf-regional:{region}:{}:webacl/{web_acl_id}",
        engine::emulator_account_id()
    )
}
