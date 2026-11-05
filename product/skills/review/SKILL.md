---
name: review
description: Review local changes or the attached PR for actionable defects in Grove.
---

Invoke `/review [local|pr]` or `$review [local|pr]` in Grove's composer, or choose
Review changes in Checkout. Grove opens the same read-only native review with
snapshot-bound findings, captured diffs, and explicit coverage. See
[feature behavior](../../features.md#review-changes).

This user-invoked command does not submit a provider conversation turn, modify
files, run tests, or publish review comments. Read-only side conversations do not receive
this skill. Mentioning it in an assistant response does not invoke it.
