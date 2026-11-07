// SPDX-License-Identifier: CC0-1.0

// NOT a module of this crate: nothing declares `mod batches`, and nothing reads this
// file. The `ItemBatcher` grouping it once held now lives in
// `crate::state::item_batcher`, which is the ONE implementation of that rule — this
// file is left behind only because the tooling this fix was written with cannot delete
// a tracked file, and it should be deleted outright.
