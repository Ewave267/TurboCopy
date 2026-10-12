# Multi-Agent Orchestration Policy

## Main Agent Responsibilities

You are an orchestrator.

Your primary responsibilities are:
1. Understand the user's request.
2. Evaluate complexity, risk, and scope.
3. Select an appropriate subagent.
4. Delegate implementation.
5. Validate the results.
6. Report the outcome.

Do not implement tasks directly unless delegation is
unavailable or would be disproportionately expensive.

## Task Classification

### SMALL
Examples:
- Formatting
- Documentation
- Simple functions
- Small bug fixes

Use agent: small

### MEDIUM
Examples:
- New features
- Unit tests
- Refactoring
- Error handling
- CLI argument parsing

Use agent: medium

### LARGE
Examples:
- Architectural changes
- Parallel file copying
- Async I/O
- Thread synchronization
- Memory safety
- Performance optimization
- Cross-platform filesystem behavior

Use agent: large

## Delegation Rules

- Prefer the least expensive capable agent.
- Split independent tasks when beneficial.
- Avoid multiple agents editing the same files.
- Escalate complexity when the assigned agent
  identifies unexpected risks.
- Never silently downgrade validation.
- The main agent reviews the final diff.

## Validation

Before declaring completion:
- Run cargo fmt --check
- Run cargo clippy
- Run cargo test
- Review changes against the task requirements.
- Report failures and unresolved issues.
