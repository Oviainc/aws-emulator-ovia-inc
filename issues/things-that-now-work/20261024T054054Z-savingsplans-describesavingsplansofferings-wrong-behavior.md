Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# Seeded offering catalog covers only 8 of the 14 modeled product types, so productType searches for the database products answer empty

_Filed: 2026-10-24T05:40:54Z_
_Operation: DescribeSavingsPlansOfferings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeSavingsPlansOfferings` searches a catalog that is seeded in `crate::state::seed_offerings`. The catalog advertises these `productTypes` only: Compute offerings -> ["Fargate","EC2","Lambda"], EC2Instance -> ["EC2"], SageMaker -> ["SageMaker"], Database -> ["RDS","DocDB","Neptune","ElastiCache"].

The model's `SavingsPlanProductType` enum declares 14 values: EC2, Fargate, Lambda, SageMaker, RDS, DSQL, DynamoDB, ElastiCache, DocDB, Neptune, Timestream, Keyspaces, DMS, OpenSearch. Six of them (DSQL, DynamoDB, Timestream, Keyspaces, DMS, OpenSearch) appear on no offering in the catalog.

Expected: a search narrowed to a product type the service sells Savings Plans for (e.g. `productType: "DynamoDB"` or `"Timestream"`) answers with the offerings that cover it, and `CreateSavingsPlan` can buy one of them.
Actual: the emulator answers with an empty `searchResults` page (plus the first-page cursor), and no offering exists that a client scoped to those products can purchase. The same hole makes `SavingsPlan.productTypes` unable ever to report those values.

Fix is seed-data breadth: extend the Database (and any new plan-type) rows so every product type the model enumerates is covered by at least one offering, keeping the existing per-term/payment-option/region shape of the catalog.

## Evidence

Spec: `SavingsPlanProductType` enum in the Smithy model (`smithy_query shape=SavingsPlanProductType`) lists 14 product types, and `DescribeSavingsPlansOfferingsInput.productType` / `SavingsPlanOffering.productTypes` are typed by it. Handler: `src/ops/describe_savings_plans_offerings.rs` filters on `offering.product_types` (`admits`, lines ~252-256) against the catalog built in `src/state.rs::seed_offerings` (lines 738-836), whose only product-type lists are `["Fargate","EC2","Lambda"]`, `["EC2"]`, `["SageMaker"]` and `["RDS","DocDB","Neptune","ElastiCache"]`. Tests: `tests/all/savings_plans_lifecycle.rs::offering_search_applies_every_filter` exercises `productType` only for "Lambda" and "EC2"; no test asserts the behavior for the six uncovered product types, so this is not already covered.

## Steps to Reproduce

1. POST /DescribeSavingsPlansOfferings with body {"productType": "DynamoDB", "maxResults": 1000}
2. Observe `searchResults` is empty, where the real service answers with the offerings that cover DynamoDB usage
3. Repeat for "Timestream", "Keyspaces", "DMS", "OpenSearch" and "DSQL" — all answer empty
4. Contrast with {"productType": "RDS"}, which returns the seeded Database rows
