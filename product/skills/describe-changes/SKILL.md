---
name: describe-changes
description: Start a Grove description session for local changes or a GitHub PR, with a saved summary, file walkthrough and explicit coverage.
---

# Describe changes in Grove

The user invokes `/describe-changes` (or `$describe-changes`) in Grove's composer,
or chooses **Describe changes** from the overflow menu beside Checkout's
**Local changes** or **Pull request** heading. Grove intercepts the command and
starts a dedicated, retained Codex or Claude conversation. The originating draft
is preserved. An invocation while the same source is already generating opens
that active session instead of starting duplicate work.

Use `/describe-changes local` for staged, unstaged and nonignored untracked edits,
`/describe-changes pr` for the attached PR, or
`/describe-changes https://github.com/owner/repo/pull/17` for an explicit GitHub.com
PR. With no argument, use local changes. The section menu supplies its own
source explicitly.
Scopes are never combined. An explicit PR URL does not switch or attach the checkout.

Grove captures the immutable diff, saves the request for recovery, and sends it
through the new session's normal provider turn. Generate from the supplied capture
only: do not inspect additional files, run tools, edit repository files, run tests,
commit, push or publish. Return the requested `grove-description` JSON block so
Grove can validate and save it. This skill adds no tools or permissions.

The result has a behavior-focused title, one to four concise summary bullets,
a file walkthrough grouped by purpose, optional Mermaid flow and explicit omitted
coverage. Describing a diff does not establish that tests passed.

There is one canonical saved result per PR, or per local checkout, in Grove's
private configuration storage. Sessions link to that result; regeneration replaces
it at the same path. Interrupted requests remain recoverable. Acknowledged failure
or cancellation preserves the last saved result and retires that request; an older
or canceled request cannot overwrite a newer result or be resurrected on restart.

The native page opens after generation and remains available through the
**Open description** entry in the corresponding Checkout section menu, including
when the checkout has no remaining local edits or attached PR. **Conversation**
returns to the retained conversation; the menu entry reopens the page. Saved
results survive restart. Changed or unverifiable snapshots retain their content,
show staleness and offer regeneration.

**Copy Markdown** is explicit. **Update PR description** is a separate action
with an editable preview, current-result and snapshot guards, preserved human
context and an unchanged PR title. A local description has no publishing action.

Mentioning a command in an assistant reply does not invoke it. Do not claim
creation, generation, saving or publication without an observed Grove outcome.
Read-only side conversations do not receive this capability.
