---
name: review
description: Review local changes or a GitHub PR for actionable defects in Grove.
---

Invoke `/review [local|pr|PR-URL]` or `$review [local|pr|PR-URL]` from Dashboard or
Grove's composer, or choose the Review action in Checkout. Grove creates or
reuses a dedicated retained conversation and opens validated snapshot-bound
findings, captured diffs and explicit coverage. See
[feature behavior](../../features.md#review-changes).

The originating draft and conversation stay intact. View review reopens the
saved result after restart; Conversation returns to that same retained session.
The initial prompt requests read-only analysis within normal session permissions.
No GitHub review is published and no fixes are applied automatically. Read-only
side conversations do not receive this skill. Mentioning it in an assistant
response does not invoke it.
