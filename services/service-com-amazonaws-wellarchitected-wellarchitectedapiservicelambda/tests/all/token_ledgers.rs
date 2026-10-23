// SPDX-License-Identifier: CC0-1.0

//! The idempotency ledgers a CALLER keys stay bounded.
//!
//! A `ClientRequestToken` is caller-chosen, so a map keyed by one is growth this service
//! does not control: a client issuing creates with a fresh token each time would otherwise
//! make the emulator hold a key and a record per request for the life of the process. The
//! rule every such ledger follows is therefore the same one — hold at most
//! `MAX_REMEMBERED_REQUEST_TOKENS` tokens and forget the one remembered longest ago — and
//! this suite asserts it over EVERY ledger the state struct holds, one row each, rather
//! than over the single one that happened to be noticed first. A token the ledger has
//! forgotten is not a replay: the next request carrying it acts afresh, which
//! `custom_lens_import` pins over HTTP for the import ledger.
//!
//! The ledgers are driven through the state the handlers share, because reaching a ceiling
//! of ten thousand over HTTP would take ten thousand signed requests; the eviction rule
//! being asserted is the state's own.

use super::workload_harness::Client;

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// How far past the ceiling each row fills its ledger.
const OVERFLOW: usize = 25;

/// The `ClientRequestToken` of the `index`th filler request. Zero-padded so the newest and
/// the oldest are nameable afterwards.
fn token_of(index: usize) -> String {
    format!("bulk-token-{index:08}")
}

/// Fill `ledger` past its ceiling and assert what it did: held exactly
/// `MAX_REMEMBERED_REQUEST_TOKENS` tokens, kept the newest and evicted the oldest.
fn assert_bounded<Token, Mint: Fn(usize) -> Token>(
    case: &str,
    ledger: &mut state::TokenLedger<Token>,
    mint: Mint,
) {
    let total = state::MAX_REMEMBERED_REQUEST_TOKENS + OVERFLOW;
    for index in 0..total {
        ledger.remember(token_of(index), mint(index));
    }
    assert_eq!(
        ledger.len(),
        state::MAX_REMEMBERED_REQUEST_TOKENS,
        "case {case:?}: the ledger must hold exactly {} tokens after {total} inserts, got {}",
        state::MAX_REMEMBERED_REQUEST_TOKENS,
        ledger.len()
    );
    assert!(
        ledger.contains_key(&token_of(total - 1)),
        "case {case:?}: the newest token must be remembered, got a ledger of {}",
        ledger.len()
    );
    assert!(
        !ledger.contains_key(&token_of(0)),
        "case {case:?}: the oldest token must have been evicted, got a ledger of {}",
        ledger.len()
    );
    // Re-remembering a token already held names the same request: it replaces the entry
    // and evicts nothing.
    ledger.remember(token_of(total - 1), mint(total - 1));
    assert_eq!(
        ledger.len(),
        state::MAX_REMEMBERED_REQUEST_TOKENS,
        "case {case:?}: re-recording a remembered token must evict nothing, got {}",
        ledger.len()
    );
    // And forgetting one is what a token resolving to a resource that is gone does.
    ledger.forget(&token_of(total - 1));
    assert!(
        !ledger.contains_key(&token_of(total - 1)),
        "case {case:?}: a forgotten token must be gone, got a ledger of {}",
        ledger.len()
    );
    assert_eq!(
        ledger.len(),
        state::MAX_REMEMBERED_REQUEST_TOKENS - 1,
        "case {case:?}: forgetting one token must drop exactly one, got {}",
        ledger.len()
    );
}

/// Every caller-keyed ledger on the state struct: one row per operation that takes a
/// `ClientRequestToken`, so a ledger added without a bound fails here.
#[tokio::test]
async fn every_caller_keyed_token_ledger_stays_within_its_ceiling() {
    let client = Client::new();
    let state_handle = client.store();
    let mut store = state_handle.lock();

    assert_bounded(
        "create-workload",
        &mut store.create_workload_tokens,
        |index| state::CreateWorkloadToken {
            workload_id: format!("{index:032x}"),
            fingerprint: format!("fingerprint-{index}"),
        },
    );
    assert_bounded(
        "create-milestone",
        &mut store.create_milestone_tokens,
        |index| state::CreateMilestoneToken {
            workload_id: format!("{index:032x}"),
            milestone_number: 1,
            fingerprint: format!("fingerprint-{index}"),
        },
    );
    assert_bounded(
        "create-workload-share",
        &mut store.create_workload_share_tokens,
        |index| state::CreateWorkloadShareToken {
            workload_id: format!("{index:032x}"),
            share_id: format!("{index:032x}"),
            fingerprint: format!("fingerprint-{index}"),
        },
    );
    assert_bounded(
        "create-review-template",
        &mut store.create_review_template_tokens,
        |index| state::CreateReviewTemplateToken {
            template_arn: format!(
                "arn:aws:wellarchitected:us-west-2:000000000000:review-template/{index:032x}"
            ),
            fingerprint: format!("fingerprint-{index}"),
        },
    );
    assert_bounded("import-lens", &mut store.import_lens_tokens, |index| {
        state::ImportLensToken {
            lens_id: format!("{index:032x}"),
            fingerprint: format!("fingerprint-{index}"),
        }
    });
    assert_bounded(
        "create-lens-version",
        &mut store.create_lens_version_tokens,
        |index| state::CreateLensVersionToken {
            lens_id: format!("{index:032x}"),
            lens_version: format!("1.{index}"),
            fingerprint: format!("fingerprint-{index}"),
        },
    );
}
