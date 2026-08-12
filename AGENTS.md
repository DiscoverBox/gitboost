# Project instructions

## Engineering principles

Prefer the smallest implementation that fully satisfies the current requirement.

### Avoid overengineering

- Do not introduce abstractions for hypothetical future requirements.
- Do not create interfaces, factories, registries, plugin systems, event buses,
  generic repositories, or framework-style layers unless the current task
  requires them.
- Do not refactor unrelated code while implementing a feature.
- Do not create new modules when an existing module is an appropriate home.
- Do not add dependencies if the task can reasonably be implemented with
  existing dependencies or standard library functionality.
- Do not add configuration options unless the requirement explicitly needs
  configurability.
- Do not create reusable helpers for logic that is only used once unless doing
  so materially improves readability.
- Prefer explicit code over generic infrastructure.

### Change budget

For each task:

1. Identify the smallest set of files that must change.
2. Modify those files only unless another change is required for correctness.
3. Preserve the existing architecture and conventions.
4. Implement only the requested behavior.
5. Add tests for the requested behavior.
6. Stop when the acceptance criteria are satisfied.

If a larger refactor appears beneficial but is not required:
- do not perform it;
- mention it separately as an optional follow-up.

When choosing between:
A. a simple implementation that satisfies today's requirements
B. a more extensible implementation for possible future requirements

choose A.
