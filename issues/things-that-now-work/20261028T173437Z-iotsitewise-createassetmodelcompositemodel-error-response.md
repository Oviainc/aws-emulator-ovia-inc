Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# The failed asset-model precondition is worded three different ways across the five handlers that evaluate it, and one echoes the caller's own If-Match value the stated rule forbids

_Filed: 2026-10-28T17:34:37Z_
_Operation: CreateAssetModelCompositeModel | Class: error-response_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Five handlers evaluate the identical `If-Match` / `If-None-Match` / `matchForVersionType` precondition against the same shared target ETag (`crate::state::asset_model_target_etag`) and then answer `PreconditionFailedException` with three different sentences:

* UpdateAssetModel and UpdateAssetModelCompositeModel: "Provided If-Match value does not match the current {version_type} version ETag."
* DeleteAssetModel and DeleteAssetModelCompositeModel: "Precondition failed for provided If-Match value."
* CreateAssetModelCompositeModel: "The If-Match precondition failed: the {version_type} version of asset model {id} does not have entity tag {expected}."

UpdateAssetModel states the rule the others break: the wording is the one the service answers a failed asset-model precondition with, "parameterised by the version the headers targeted; nothing of the request's own values goes into it". CreateAssetModelCompositeModel's message interpolates `expected` — the caller's own `If-Match` header — which is exactly the request value that rule excludes, and the two delete handlers drop both the version type and the sentence altogether. One condition on one resource therefore reports itself three ways depending on which verb was used.

## Evidence

The rule, stated in the handler the others diverge from:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/update_asset_model.rs:277-281
    "// The wording of both refusals is the one IoT SiteWise answers a failed
     // asset-model precondition with — \"Provided If-Match value does not match
     // the current latest version ETag.\" — parameterised by the version the
     // headers targeted; nothing of the request's own values goes into it."
  src/ops/update_asset_model.rs:301-311
    "if let Some(expected) = input.if_match.as_deref() { if target_e_tag.as_deref() != Some(expected) { return Err(precondition_failed(&region, &id, format!(\"Provided If-Match value does not match the current {version_type} version ETag.\")));"
  (same sentence at src/ops/update_asset_model_composite_model.rs:307)

The handler under this issue, echoing the request's own value:
  src/ops/create_asset_model_composite_model.rs:396-406
    "if let Some(expected) = input.if_match.as_deref() {
         if target.as_deref() != Some(expected) {
             return Err(precondition_failed(&region, &asset_model_id, format!(
                 \"The If-Match precondition failed: the {version_type} version of asset \\
                  model {asset_model_id} does not have entity tag {expected}.\")));"

The third wording, for the same comparison on the same target ETag:
  src/ops/delete_asset_model.rs:270-277 — "if let Some(expected) = input.if_match.as_deref() { if target_e_tag.as_deref() != Some(expected) { return Err(precondition_failed(\"Precondition failed for provided If-Match value.\", &id, &arn));"
  src/ops/delete_asset_model_composite_model.rs:277-284 — same literal "Precondition failed for provided If-Match value."

The crate treats refusal wording as a rule with a hand-written oracle for its other refusal families: tests/all/refusal_message_rule.rs:3-24 ("The two refusal-WORDING rules this service follows ... The expected strings below are written out BY HAND from those rules, as an independent oracle: the suite never calls the crate's message helpers, so a handler that drifts back to a template of its own cannot agree with itself."). No test asserts any of the three precondition sentences (grep for "Precondition failed for provided" / "does not match the current" / "If-Match precondition failed" across tests/ finds nothing), so nothing pins the drift.

## Steps to Reproduce

1. CreateAssetModel "M" and read its ETag with DescribeAssetModel.
2. POST /asset-models/{M}/composite-models with If-Match: "not-the-etag" → 412 "The If-Match precondition failed: the latest version of asset model {M} does not have entity tag not-the-etag." (the caller's own header value is echoed back).
3. PUT /asset-models/{M} with If-Match: "not-the-etag" → 412 "Provided If-Match value does not match the current latest version ETag."
4. DELETE /asset-models/{M} with If-Match: "not-the-etag" → 412 "Precondition failed for provided If-Match value."
5. One failed precondition on one asset model reports itself three different ways, and only the second obeys the rule stated at update_asset_model.rs:277-281.
