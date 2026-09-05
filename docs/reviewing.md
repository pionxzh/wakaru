# Reviewing and Handing Off Work

Review against the agreed behavior and the actual version under review. Read
[rewrite-assumptions.md](rewrite-assumptions.md) for the semantic contract and
[learnings/](learnings/) before proposing a broader design.

## Establish the target

Use a short handoff in the task or PR; a separate document is not required.
Include the worktree, base and HEAD, whether uncommitted changes are included,
the intended behavior, known open items, and completed verification. Preserve
the user's scope and previously stated limits on commit, push, and integration.

Confirm the checkout matches that handoff before reviewing. If implementation
is still in progress, identify the snapshot you inspected and label findings
against it as provisional. Do not describe an intermediate failure as a defect
in a later completed revision. When hashes changed after rebase, compare the
patches and base changes rather than relying on old hashes alone.

Start the report with the review verdict, the concrete problem, and intended
before/after behavior. Explain unfamiliar case labels and where their
inputs came from before asking the user to decide what to do about them.

## Classify findings before proposing fixes

For each actionable finding, identify:

- The input, mode or rewrite level, and violated contract.
- A reproducer or precise code path showing the consequence.
- Whether it was introduced by this change, already existed on the base, or
  was newly exposed by a harness or validator change. Say when attribution is
  still unknown.
- The evidence source: real bundle, reproduced toolchain output, or handwritten
  counterexample. For generated-code recovery, identify the producer/version
  when available.
- The smallest correction, its scope, and why it should or should not block
  this change.

A handwritten counterexample can prove a correctness bug within the supported
contract; corpus occurrence is not required. Conversely, a scenario outside an
accepted environment assumption is not automatically a merge blocker. Cite
the relevant assumption and separate a proposed policy change from a defect
under today's policy. Never use an assumption to dismiss an observed violation
of a hard rule, such as removing a temporary that is used elsewhere.

Keep baseline defects, unsupported inputs, deliberate fallback, and unexplained
findings distinct from candidate regressions. Fallback may satisfy the safety
contract without counting as successful recovery. Fewer validator findings do
not by themselves prove better output; check for lost modules, reduced coverage,
or changed validation. When both producer and validator changed, compare them
separately on fixed inputs before attributing the difference.

## Keep the fix proportionate

Before a non-obvious implementation, state the target shape, the proof already
available, the assumptions it needs, and the remaining boundary. Inspect shared
analysis and neighboring callers before adding another collector or visitor.
Confirm the AST shape at the actual pipeline position using
[debugging.md](debugging.md).

If a narrow fix starts requiring a new scope model, alias graph, or control-flow
analysis, reassess the design before extending it. Compare reuse of existing
analysis, a narrower matcher, preserving the original form, and an explicit
level/assumption change. Explain the recovery and correctness tradeoff; none
of these alternatives is an automatic substitute for satisfying the contract.

Report unrelated discoveries separately with a disposition and next step. Do
not silently expand a targeted fix into a subsystem audit. The author should
recommend a scope decision, with evidence and cost, rather than leave the user
to infer it from test labels.

## Re-review and completion

For a follow-up review, first check the previous findings and the effects of
their fixes. Expand inspection when a new change, failure, or unresolved concern
justifies it. Use the evidence rules in
[testing.md](testing.md#sharing-verification-results) instead of repeating the
full suite solely because the reviewer changed.

State which revision is approved or still blocked. Distinguish readiness of the
core fix from readiness of the whole branch, including any new harness contract.
List remaining blockers, non-blocking follow-ups, and checks still pending.
Review approval describes the inspected code; it does not itself authorize a
push or merge.
