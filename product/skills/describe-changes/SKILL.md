---
name: describe-changes
description: Describe the selected checkout's local changes or attached pull request in Grove, with a concise summary, grouped file walkthrough, optional diagram, and explicit coverage.
---

# Describe changes in Grove

This is a Grove-hosted skill shared by Codex and Claude sessions. The user invokes
`/describe-changes` (or `$describe-changes`) in Grove's composer, or chooses
**Describe changes** in Checkout. Grove runs the same native description workflow
for both entry points; invoking the skill does not submit a conversational prompt
or alter the main agent's conversation.

Use `/describe-changes local` for staged, unstaged, and nonignored untracked
changes. Use `/describe-changes pr` for the attached pull request's base-to-head
diff. Use `/describe-changes https://github.com/owner/repo/pull/17` to describe
an explicit GitHub.com PR without changing the checkout or attaching it.
Commands also work from the Dashboard before provider startup. Without an
argument, use the scope selected in Checkout, defaulting to local changes when no
scope has been selected. If the checkout
has multiple possible scopes, keep that choice visible instead of combining them.

## Result

Generate a concise behavior-focused title and one to four summary bullets. Group
the file walkthrough by purpose, with each file's path, short change title, and
brief supporting details. Include a diagram only when it explains a meaningful
flow. Derive all claims from the captured diff, and identify omitted or truncated
files. Describing changes does not establish that tests passed.

The native reader retains the exact snapshot identity. When the checkout or PR
changes, preserve the prior description, mark it out of date, and offer
regeneration. Cancellation and failure preserve the previous result and checkout.

Generation stays inside Grove. Copy Markdown is explicit. Updating an attached
PR is a separate user action with an editable preview; keep its title and existing
context, recheck the snapshot and current description, and reject a stale update.
A local description has no PR publishing action. Never commit, push, merge, or
publish merely because the user requested a description.

## Agent boundary

Grove handles these user-entered invocations before provider submission. Mentioning
this command in an assistant response does not execute it. Do not claim that a
description was generated or published unless Grove reports that outcome. For a
conversational request, explain the changes using available evidence or direct the
user to the native action; this skill does not grant an additional tool or expand
provider permissions. Read-only side conversations do not receive this capability.
