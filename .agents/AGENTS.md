# Operation Procedures & Testing Guidelines

## Pre-Commit Testing Rule
Every time before executing a `git commit` for any code changes, the agent **MUST** perform a comprehensive unit test run.

- **Unit Tests**: Mandatory. Ensure all unit tests pass completely before proceeding with a commit.
- **Integration Tests**: Optional by default. Only run integration tests if there is an explicit instruction or requirement from the user to do so.
