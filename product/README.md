# Product guidance for agents

Grove is a native workspace for developers supervising AI agents and terminals
across repositories and Git worktrees. Preserve session/process identity,
workspace scope and unfinished drafts while users move between tasks. Favor
quiet, compact chrome, clear activity and keyboard access. Read the relevant
current contract before changing behavior; screenshots, old plans and historical
verification records are evidence, not competing specifications.

| Changing | Read |
|---|---|
| Native UI, navigation, controls, themes or interaction | [Design contract](design.md) and the [Grove UI skill](../.agents/skills/grove-ui/SKILL.md) |
| Plans, reports or change categories | [Feature behavior](features.md) |
| GPUI APIs, state, async work or rendering | [Development guidance](development.md) |
| A measured native/HTML component reference | [Reference workflow](ui-reference.md) |
| Native provider contracts, completion, ownership or diagnostics | [Provider guidance](providers.md) |
| Validation, provider smoke checks or known verification gaps | [Verification](verification.md) |

`design.md` is the normative semantic UI contract. Exact values and complete
control recipes remain in the native source owners it names. Update the owning
document with an approved behavior change; update skill routing when the process
changes. Keep one maintained explanation of each rule, and link to it elsewhere.

Repository operation and delivery rules remain in [AGENTS.md](../AGENTS.md).
The root [README](../README.md) covers installation and getting started;
[release notes](../RELEASE_NOTES.md) describe shipped changes. Licenses, vendor
provenance, font provenance and fixture-local instructions stay beside their
owners. Temporary proposals and generated evidence belong in ignored scratch
storage, not this directory. Record unresolved verification work in
`verification.md` with its actual scope and limits.
