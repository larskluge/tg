# Build and test

The everyday commands and the owl tier are in [AGENTS.md](../AGENTS.md); these are the notes behind
the tests that talk to a real TDLib.

**Testing against real TDLib:** static requests (`parseTextEntities`, `getMarkdownText`,
`setLogVerbosityLevel`) answer on a client that has never called `setTdlibParameters`, so a
test needs `tdlib_rs::create_client()` and a receive thread and nothing else — no credentials,
no database files, no network. tdlib-rs 1.3.0 exposes no synchronous `td_execute`, so the
receive loop is mandatory even for a static request. Three rules, all learned by hitting
them, and all three abort the process rather than failing a test:

- **Exactly one receive loop per process.** TDLib aborts ("Receive must not be called
  simultaneously from two different threads") as soon as `cargo test` runs two loop-owning
  tests in parallel. `client::tdlib_parse_tests` shares one refcounted loop behind a mutex.
- **Join the loop; never leak it.** Hold the lock across the join, or a replacement loop
  starts while the old one is still parked inside `td_receive`.
- **Close the client, and wait for `authorizationStateClosed`.** This is the one that bites
  silently. TDLib frees a client's resources only after that update — "All resources will be
  freed only after authorizationStateClosed has been received" — and a process that exits
  with a client still open aborts inside TDLib's teardown, *after* the harness has already
  printed `test result: ok`. The symptom is a bare `SIGABRT` with no failing test and, because
  the harness sets log verbosity to 0, no TDLib message either. It is intermittent, so it
  passes locally and on the PR run and then fails on `main` — where it blocks the image push.
  Measured by looping the real test binary on 4 cores against libtdjson 1.8.61: 15 of 100 runs
  aborted without the close, 0 of 100 with it.

So teardown is: send `close`, let the loop keep pumping until it sees `Closed`, then join. The
loop's exit condition is that update, not a shutdown flag, and both the close acknowledgement
and the `Closed` confirmation are asserted — losing either means the process is once again
exiting with a live client, and that must fail where it is legible rather than at exit.

Set log verbosity with the synchronous `set_tdlib_log_verbosity` **before** `create_client()`.
The async `setLogVerbosityLevel` only takes effect once TDLib is already up, so it cannot
suppress the startup banner, which otherwise floods CI logs.
