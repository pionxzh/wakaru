# Proposals

A proposal records a design that needs a decision before code, or planned
work that cannot start now. Each one opens with a `Status:` line; read it
before acting on the rest.

## When to write one

Write a proposal only when at least one of these holds:

- The change needs a choice between real alternatives: a new subsystem, a
  pipeline or cross-rule restructuring, a public API shape.
- The work spans several commits or sessions, and a later agent needs the
  plan to continue it.
- A design is settled or proposed but its implementation is deferred:
  blocked on evidence, an upstream fix, or another line. Record the
  revisit trigger.

Fixing a bug, or pausing a task or research branch (see
[reviewing.md](../reviewing.md#pause-and-resume-research)), does not need
one.

Ask the maintainer before writing a proposal. Proposals are reviewed before
they are committed.

Update the `Status:` line in the same commit that changes the state.
