// SPDX-License-Identifier: CC0-1.0

//! The two halves of the retrieval surface AGREE on which `RetrievalToken`
//! values exist: the integrity check accepts exactly the values the store can
//! resolve (the wire harness in [`super::xray_wire`], the rule and its helpers
//! in [`super::retrieval_token_exact_byte_integrity`]).
//!
//! The rule the rows below generalise over: the check a token is judged by is
//! recomputed over the exact characters submitted, and the retrievals it
//! resolves are held under the exact characters the mint emitted, so no value
//! can pass the check and then fail to resolve — a value re-cased anywhere,
//! over however many characters, is refused as a malformed parameter
//! (`InvalidRequestException`, HTTP 400) while the retrieval it was derived from
//! stands untouched, and only the issued characters reach it.
//!
//! Rows differ along what that rule generalises over: WHICH SPAN of an issued
//! token is re-cased — the whole value, its identifying half, its check half —
//! so the damage spans many characters rather than the single one the sibling
//! suite sweeps, and which operation was called and therefore which member its
//! refusal names.

use serde_json::json;

use service_com_amazonaws_xray_awsxray::state::{
    RETRIEVAL_STATUS_CANCELLED, RETRIEVAL_TOKEN_HEX_DIGITS,
};

use super::retrieval_token_exact_byte_integrity::{
    body, genuine_token, undecodable_message, TOKEN_READERS,
};
use super::xray_wire::Xray;

#[tokio::test]
async fn re_casing_a_whole_span_of_an_issued_token_never_resolves_it() {
    let xray = Xray::new();
    let real = genuine_token(&xray, 23).await;
    let half = RETRIEVAL_TOKEN_HEX_DIGITS / 2;
    let (identity, check) = real.split_at(half);

    let spans = [
        ("the whole value", real.to_uppercase()),
        (
            "the identifying half",
            format!("{}{check}", identity.to_uppercase()),
        ),
        (
            "the check half",
            format!("{identity}{}", check.to_uppercase()),
        ),
    ];

    let mut asserted = 0;
    for (span, recased) in &spans {
        // A span of digits alone re-cases to itself, which is the issued value
        // rather than a damaged one: the rule is about values that DIFFER.
        if recased == &real {
            continue;
        }
        asserted += 1;
        let case = format!("{span} of an issued token re-cased");
        assert_eq!(
            recased.len(),
            RETRIEVAL_TOKEN_HEX_DIGITS,
            "case {case:?}: re-casing keeps the minted width, got {recased:?}"
        );
        for (operation, path, member) in TOKEN_READERS {
            let refused = xray.call(path, &body(path, recased)).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} on {operation}: a value the store cannot resolve must be refused \
                 before the lookup, got {refused:?}"
            );
            let expected = undecodable_message(member, recased);
            let said = refused.message().unwrap_or_default();
            assert_eq!(
                said, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {said:?}"
            );
        }
    }
    assert!(
        asserted > 0,
        "at least one span of {real:?} must differ when re-cased"
    );

    // The retrieval behind the issued value was never reached by any of those
    // refusals, and the issued characters still resolve it.
    let listed = xray
        .call(
            "/ListRetrievedTraces",
            &json!({"RetrievalToken": &real, "TraceFormat": "XRAY"}),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "the issued value must still read its retrieval, got {listed:?}"
    );
    assert_ne!(
        listed.get("RetrievalStatus").as_str(),
        Some(RETRIEVAL_STATUS_CANCELLED),
        "no refused call may have cancelled the retrieval, got {listed:?}"
    );
    let cancelled = xray
        .call("/CancelTraceRetrieval", &json!({"RetrievalToken": &real}))
        .await;
    assert_eq!(
        cancelled.status, 200,
        "the issued value must still cancel its retrieval, got {cancelled:?}"
    );
    let after = xray
        .call(
            "/ListRetrievedTraces",
            &json!({"RetrievalToken": &real, "TraceFormat": "XRAY"}),
        )
        .await;
    assert_eq!(
        after.get("RetrievalStatus").as_str(),
        Some(RETRIEVAL_STATUS_CANCELLED),
        "the cancelled retrieval must read back as cancelled, got {after:?}"
    );
}
