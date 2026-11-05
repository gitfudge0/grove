---
name: ask
description: Answer a question about selected local changes or the attached PR in Grove.
---

The user invokes `/ask [local|pr] [question]` or `$ask [local|pr] [question]`
in Grove's composer, or chooses **Ask about changes** in Checkout. Omitting the
question opens an editable question field. Grove handles this command before
provider submission; it preserves the composer and uses a separate tool-free
model call on an immutable diff. Replies cite captured lines and disclose omitted
patches. No repository writes or external publication occur. Read-only side
conversations do not receive this skill. See `product/features.md` for behavior.
