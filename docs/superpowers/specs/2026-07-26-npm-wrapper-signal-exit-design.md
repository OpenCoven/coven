# npm Wrapper Signal Exit Design

## Problem

The npm launcher installs SIGINT and SIGTERM forwarding handlers. When the
native child exits because of one of those signals, the launcher re-sends the
signal to itself without removing its handler. The handler consumes the signal,
and the launcher falls off the event loop with exit code 0.

## Termination contract

- Continue forwarding SIGINT and SIGTERM from the wrapper to the native child.
- On POSIX, remove the wrapper's handler for the child's terminating signal
  before re-raising it. Shells and supervisors then observe the wrapper as
  terminated by the same signal.
- On Windows, where POSIX re-raising is not portable, exit with
  `128 + os.constants.signals[signal]`. Use exit code 1 only if Node reports an
  unknown signal name.
- Preserve ordinary child exit codes and launch-error behavior unchanged.

## Test design

`scripts/publish-npm-test.mjs` will construct a temporary wrapper installation
whose platform-native binary is a symlink to the current Node executable. The
fake child prints a readiness marker and waits. The test sends SIGINT and
SIGTERM to the wrapper only after readiness, then asserts that POSIX reports
the same terminating signal rather than exit code 0. A source-level Windows
guard asserts that the numeric fallback remains packaged.

The fixture is isolated under the OS temporary directory and removed in a
`finally` block.

## Acceptance evidence

- The new behavioral test fails against the current wrapper with exit code 0.
- SIGINT and SIGTERM both terminate the fixed POSIX wrapper by the same signal.
- Windows fallback code maps known signals to `128 + signum`.
- Existing npm publish/onboarding smoke tests and all repository CI gates pass.
