---
name: ask
description: Answer a question about local changes or a GitHub PR in Grove.
---

Invoke `/ask [local|pr|GitHub-PR-URL] [question]` or the equivalent `$ask` in
Dashboard or either provider's composer. Omitting the question opens a native
field. A submitted question creates a retained conversation with the selected
provider/model preferences; native answers are validated and saved per source.
Conversation opens that same session. Ordinary follow-ups do not replace the
saved answer. The initial request is read-only; normal provider permissions apply.
Read-only side conversations do not receive this skill. See `product/features.md`.
