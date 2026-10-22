Issue-Severity: MEDIUM
Issue-Gap-Class: wrong

# Personalize Events accepts a metadata put against a dataset of another type

_Operation: service-com-amazonaws-personalizeevents-amazonpersonalizeevents | Class: wrong_

## Description

`PutItems`, `PutUsers` and `PutActions` each name a `datasetArn`, and the ARN's resource
segment names the dataset's TYPE: `dataset/<datasetGroup>/<TYPE>`, where `<TYPE>` is one
of `INTERACTIONS`, `ITEMS`, `USERS`, `ACTIONS`, `ACTION_INTERACTIONS` (the Amazon
Personalize `CreateDataset` `datasetType`). The real service refuses a put whose records
do not match the dataset's type — `PutUsers` into an Items dataset is not a users put.

The emulator currently accepts it: a registered `Dataset` in
`services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/src/state.rs`
carries the three collections (`items`, `users`, `actions`) and NO type, so
`put_metadata_records` stores a record in whichever collection the operation names,
whatever the ARN says, and answers 200. The limitation is documented on the `Dataset`
type, and it is deliberate for the bootstrap slice: that issue's state model gives a
dataset the three collections and no type, and its reproduction steps address all three
kinds through a single `.../ITEMS` ARN, so refusing a mismatch would have refused the
steps it asked for.

## What to implement

1. Carry the dataset type on the registry seam — `register_dataset(arn, dataset_type)`,
   or derive it from the ARN's last path segment, keeping the derived value on `Dataset`
   so one place decides it.
2. Refuse a mismatch with the operation's modelled `InvalidInputException` (HTTP 400):
   `PutItems` serves only an `ITEMS` dataset, `PutUsers` only `USERS`, `PutActions` only
   `ACTIONS`. The message should name the dataset's type and the records' type, both of
   which come from the request.
3. A dataset whose type is not one of the five published values is itself an invalid
   input, not a silent accept.
4. Test it as a table: the three operations × a matching dataset (200) × each
   non-matching published type (400), and the ARN-derived type on an entry the bootstrap
   issue did not cite.

## Evidence

`services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/src/state.rs`
— the `Dataset` doc comment states the limitation, and `put_metadata_records` writes
`dataset.records_mut(kind)` without consulting any type. The crate's tests already
address each kind through a dataset of its own type
(`tests/all/ingestion.rs`'s `METADATA_CASES`), so they need no change when the refusal
lands; what is missing is a row per MISMATCH (a users put against an `.../ITEMS` ARN,
and so on), which today answers 200.

## Steps to Reproduce

1. Register the dataset `arn:aws:personalize:us-east-1:123456789012:dataset/demo/ITEMS`.
2. Call `PutUsers` with that `datasetArn` and `users: [{userId: "user-1"}]` -> the
   emulator answers 200 and stores the user; the real service refuses the call with
   `InvalidInputException` (400) because the dataset is an Items dataset.
