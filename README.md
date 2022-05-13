A library to assist in reporting on the health of a system.

## Usage

In order to integrate with this library, a module will need to implement
the `Checkable` trait.

Consumers can implement the `Checkable` trait directly or provide a
function that can perform the health check. Such a function can be either
synchronous or asynchronous.

### Synchronous checker

Synchronous checker functions have the form
`Fn() -> Result<(), Error>` and can be created with
`check_fn()`.

```
# use std::fmt::Error;
use health::Checkable;

fn all_is_well() -> Result<(), Error> { Ok(()) }
fn everything_is_fire() -> Result<(), Error> { Err(Error {}) }

let always_ok = health::check_fn("ok", all_is_well);
let always_bad = health::check_fn("bad", everything_is_fire);
```

### Asynchronous checker

Asynchronous checker functions have the form
`async Fn() -> Result<(), Error>` and can be created with
`check_future()`.

```
# use std::fmt::Error;
use health::Checkable;

async fn all_is_well() -> Result<(), Error> { Ok(()) }
async fn everything_is_fire() -> Result<(), Error> { Err(Error {}) }

let always_ok = health::check_future("ok", all_is_well);
let always_bad = health::check_future("bad", everything_is_fire);
```

### Periodic background health checks

Once a `Checkable` is created, that can be passed to a
`PeriodicChecker<C>`, which implements the `Reporter` trait. The
periodic checker can be configured to define the parameters for reporting
a health status.

Health checks are performed periodically in the background and not inline
to requests for the current health status. This ensures that information
about the current health status is always available without delay, which
can be particularly important for health checking endpoings on web servers
(such as the oft-seen `/healthz` and `/health` endpoints).

## Example

```
# use std::{fmt::Error, future::Future};
use std::time::Duration;
use health::Reporter;

async fn all_is_well() -> Result<(), Error> { Ok(()) }
let always_ok = health::check_future("ok", all_is_well);

let reporter = health::PeriodicChecker::new(always_ok, health::Config {
    check_interval: Duration::from_secs(10),
    leeway: Duration::from_secs(30),
    min_successes: 2,
    min_failures: 6,
});

// Spawn the reporter on your executor to perform
// periodic checks in the background
# fn spawn<T>(t: T)
# where
#     T: Future + Send + 'static,
#     T::Output: Send + 'static,
# {}
spawn(reporter.clone().run());

assert_eq!(health::Status::Healthy, reporter.raw_status());
assert_eq!(Some(health::Status::Healthy), reporter.status());
assert_eq!(health::Check::Pass, reporter.last_check());
```

## Tracing

This library makes use of the `tracing` library to report on the health
status of resources using the `health` target. The `PeriodicChecker<C>`
uses the following event levels when reporting the health status after
each check is complete:

* `ERROR` when the health status is unhealthy, and the most recent check
   failed.
* `WARN` when the health status is healthy, but the most recent check
   failed.
* `INFO` when the health status is unhealthy, but the most recent check
   passed.
* `INFO` for the report when an unhealthy resource becomes healthy.
* `DEBUG` when the health status is healthy, and the most recent check
   passed.

## Features

* `tracing` (default): Uses the `tracing` library to report the results
   of periodic health checks

