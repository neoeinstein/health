//! A library to assist in reporting on the health of a system.
//!
//! ## Usage
//!
//! In order to integrate with this library, a module will need to implement
//! the [`Checkable`][] trait.
//!
//! Consumers can implement the [`Checkable`][] trait directly or provide a
//! function that can perform the health check. Such a function can be either
//! synchronous or asynchronous.
//!
//! ### Synchronous checker
//!
//! Synchronous checker functions have the form
//! `Fn() -> Result<(), Error>` and can be created with
//! [`check_fn()`](fn.check_fn.html).
//!
//! ```
//! # use std::fmt::Error;
//! use health::Checkable;
//!
//! fn all_is_well() -> Result<(), Error> { Ok(()) }
//! fn everything_is_fire() -> Result<(), Error> { Err(Error {}) }
//!
//! let always_ok = health::check_fn("ok", all_is_well);
//! let always_bad = health::check_fn("bad", everything_is_fire);
//! ```
//!
//! ### Asynchronous checker
//!
//! Asynchronous checker functions have the form
//! `async Fn() -> Result<(), Error>` and can be created with
//! [`check_future()`](fn.check_future.html).
//!
//! ```
//! # use std::fmt::Error;
//! use health::Checkable;
//!
//! async fn all_is_well() -> Result<(), Error> { Ok(()) }
//! async fn everything_is_fire() -> Result<(), Error> { Err(Error {}) }
//!
//! let always_ok = health::check_future("ok", all_is_well);
//! let always_bad = health::check_future("bad", everything_is_fire);
//! ```
//!
//! ### Periodic background health checks
//!
//! Once a [`Checkable`][] is created, that can be passed to a
//! [`PeriodicChecker<C>`][], which implements the [`Reporter`][] trait. The
//! periodic checker can be configured to define the parameters for reporting
//! a health status.
//!
//! Health checks are performed periodically in the background and not inline
//! to requests for the current health status. This ensures that information
//! about the current health status is always available without delay, which
//! can be particularly important for health checking endpoings on web servers
//! (such as the oft-seen `/healthz` and `/health` endpoints).
//!
//! ## Example
//!
//! ```
//! # use std::{fmt::Error, future::Future};
//! use std::time::Duration;
//! use health::Reporter;
//!
//! async fn all_is_well() -> Result<(), Error> { Ok(()) }
//! let always_ok = health::check_future("ok", all_is_well);
//!
//! let reporter = health::PeriodicChecker::new(always_ok, health::Config {
//!     check_interval: Duration::from_secs(10),
//!     leeway: Duration::from_secs(30),
//!     min_successes: 2,
//!     min_failures: 6,
//! });
//!
//! // Spawn the reporter on your executor to perform
//! // periodic checks in the background
//! # fn spawn<T>(t: T)
//! # where
//! #     T: Future + Send + 'static,
//! #     T::Output: Send + 'static,
//! # {}
//! spawn(reporter.run());
//!
//! assert_eq!(health::Status::Healthy, reporter.raw_status());
//! assert_eq!(Some(health::Status::Healthy), reporter.status());
//! assert_eq!(health::Check::Pass, reporter.last_check());
//! ```
//!
//! ## Tracing
//!
//! This library makes use of the `tracing` library to report on the health
//! status of resources using the `health` target. The [`PeriodicChecker<C>`][]
//! uses the following event levels when reporting the health status after
//! each check is complete:
//!
//! * `ERROR` when the health status is unhealthy, and the most recent check
//!    failed.
//! * `WARN` when the health status is healthy, but the most recent check
//!    failed.
//! * `INFO` when the health status is unhealthy, but the most recent check
//!    passed.
//! * `INFO` for the report when an unhealthy resource becomes healthy.
//! * `DEBUG` when the health status is healthy, and the most recent check
//!    passed.
//!
//! ## Features
//!
//! * `tokio_0_3` (default): Uses `tokio` v0.3 to space out periodic health
//!    checks
//! * `tokio_0_2`: Uses `tokio` v0.2 to space out periodic health checks
//! * `tracing` (default): Uses the `tracing` library to report the results
//!    of periodic health checks
//!
//!   [`Checkable`]: trait.Checkable.html
//!   [`Reporter`]: trait.Reporter.html
//!   [`check_fn()`]: fn.check_fn.html
//!   [`check_future()`]: fn.check_future.html
//!   [`PeriodicChecker<C>`]: struct.PeriodicChecker.html

#![warn(
    missing_docs,
    unused_import_braces,
    unused_imports,
    unused_qualifications
)]
#![deny(missing_debug_implementations, trivial_numeric_casts, unused_must_use)]
#![forbid(unsafe_code)]

use async_trait::async_trait;
use parking_lot::Mutex;
use std::{
    borrow::Cow,
    error::Error as StdError,
    fmt,
    future::Future,
    ops,
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(feature = "tokio_0_2")]
use tokio_0_2::time::delay_for as sleep;

#[cfg(all(feature = "tokio_0_3", not(feature = "tokio_0_2")))]
use tokio_0_3::time::sleep;

/// A health status reporter
pub trait Reporter {
    /// The current health status without regard to any reliance criteria
    fn raw_status(&self) -> Status {
        self.last_check().into()
    }

    /// The current health status of the underlying health check or `None` if
    /// the current status should not be relyed upon
    fn status(&self) -> Option<Status> {
        Some(self.raw_status())
    }

    /// The result of the most recent health check
    ///
    /// Because it may take multiple checks to cause the health status to
    /// change, this value may match the current health status.
    fn last_check(&self) -> Check;
}

/// The result of a single health check run on an underlying resource
///
/// Default: `Pass`
///
/// ## Operations
///
/// ```
/// use health::Check;
///
/// assert_eq!(Check::Pass, !Check::Failed);
/// assert_eq!(Check::Failed, !Check::Pass);
///
/// assert_eq!(Check::Pass, Check::Pass & Check::Pass);
/// assert_eq!(Check::Failed, Check::Pass & Check::Failed);
/// assert_eq!(Check::Failed, Check::Failed & Check::Pass);
/// assert_eq!(Check::Failed, Check::Failed & Check::Failed);
///
/// assert_eq!(Check::Pass, Check::Pass | Check::Pass);
/// assert_eq!(Check::Pass, Check::Pass | Check::Failed);
/// assert_eq!(Check::Pass, Check::Failed | Check::Pass);
/// assert_eq!(Check::Failed, Check::Failed | Check::Failed);
///
/// assert_eq!(Check::Failed, Check::Pass ^ Check::Pass);
/// assert_eq!(Check::Pass, Check::Pass ^ Check::Failed);
/// assert_eq!(Check::Pass, Check::Failed ^ Check::Pass);
/// assert_eq!(Check::Failed, Check::Failed ^ Check::Failed);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Check {
    /// The health check passed
    Pass,
    /// The health check failed
    Failed,
}

impl Default for Check {
    #[inline]
    fn default() -> Self {
        Self::Pass
    }
}

impl ops::Not for Check {
    type Output = Self;

    fn not(self) -> Self::Output {
        match self {
            Self::Pass => Self::Failed,
            Self::Failed => Self::Pass,
        }
    }
}

impl ops::BitAnd for Check {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Pass, Self::Pass) => Self::Pass,
            _ => Self::Failed,
        }
    }
}

impl ops::BitOr for Check {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Pass, _) | (_, Self::Pass) => Self::Pass,
            _ => Self::Failed,
        }
    }
}

impl ops::BitXor for Check {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self::Output {
        if self != rhs {
            Self::Pass
        } else {
            Self::Failed
        }
    }
}

/// The health status of resource after accounting for allowable variance in
/// health check results
///
/// Default: `Healthy`
///
/// ## Operations
///
/// ```
/// use health::Status;
///
/// assert_eq!(Status::Healthy, !Status::Unhealthy);
/// assert_eq!(Status::Unhealthy, !Status::Healthy);
///
/// assert_eq!(Status::Healthy, Status::Healthy & Status::Healthy);
/// assert_eq!(Status::Unhealthy, Status::Healthy & Status::Unhealthy);
/// assert_eq!(Status::Unhealthy, Status::Unhealthy & Status::Healthy);
/// assert_eq!(Status::Unhealthy, Status::Unhealthy & Status::Unhealthy);
///
/// assert_eq!(Status::Healthy, Status::Healthy | Status::Healthy);
/// assert_eq!(Status::Healthy, Status::Healthy | Status::Unhealthy);
/// assert_eq!(Status::Healthy, Status::Unhealthy | Status::Healthy);
/// assert_eq!(Status::Unhealthy, Status::Unhealthy | Status::Unhealthy);
///
/// assert_eq!(Status::Unhealthy, Status::Healthy ^ Status::Healthy);
/// assert_eq!(Status::Healthy, Status::Healthy ^ Status::Unhealthy);
/// assert_eq!(Status::Healthy, Status::Unhealthy ^ Status::Healthy);
/// assert_eq!(Status::Unhealthy, Status::Unhealthy ^ Status::Unhealthy);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    /// The health check is reporting as healthy
    Healthy,
    /// The health check is reporting as unhealthy
    Unhealthy,
}

impl Default for Status {
    #[inline]
    fn default() -> Self {
        Self::Healthy
    }
}

impl From<Check> for Status {
    fn from(hc: Check) -> Self {
        match hc {
            Check::Pass => Self::Healthy,
            Check::Failed => Self::Unhealthy,
        }
    }
}

impl ops::Not for Status {
    type Output = Self;

    fn not(self) -> Self::Output {
        match self {
            Self::Healthy => Self::Unhealthy,
            Self::Unhealthy => Self::Healthy,
        }
    }
}

impl ops::BitAnd for Status {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Healthy, Self::Healthy) => Self::Healthy,
            _ => Self::Unhealthy,
        }
    }
}

impl ops::BitOr for Status {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Healthy, _) | (_, Self::Healthy) => Self::Healthy,
            _ => Self::Unhealthy,
        }
    }
}

impl ops::BitXor for Status {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self::Output {
        if self != rhs {
            Self::Healthy
        } else {
            Self::Unhealthy
        }
    }
}

/// Configuration for a periodic health check
///
/// Defaults:
///
/// * Checks every 5 seconds
/// * Becomes unhealthy after 3 consecutive failures
/// * Becomes healthy on first success
/// * Becomes unreliable if no completed checks in 15 seconds
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Interval over which to periodically run the health check
    pub check_interval: Duration,

    /// Minimum number of consecutive failures to flip an healthy
    /// health check to unhealthy
    pub min_failures: u8,

    /// Minimum number of consecutive successes to flip an unhealthy
    /// health check to healthy
    pub min_successes: u8,

    /// Leeway between updated before the health check status is no longer
    /// considered current
    ///
    /// To deal with variation, this should generally not be less than
    /// twice the `check_interval`.
    pub leeway: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(5),
            min_failures: 3,
            min_successes: 1,
            leeway: Duration::from_secs(15),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct State {
    status: Status,
    last_update: Duration,
    last_check: Check,
    count: u8,
}

impl State {
    fn step(&self, since_initialized: Duration, result: Check, config: &Config) -> Self {
        let mut next = *self;
        next.last_update = since_initialized;

        if self.last_check != result {
            next.count = 0;
        }
        next.count = next.count.saturating_add(1);

        next.status = if result == Check::Pass
            && self.status == Status::Unhealthy
            && next.count >= config.min_successes
        {
            Status::Healthy
        } else if result == Check::Failed
            && self.status == Status::Healthy
            && next.count >= config.min_failures
        {
            Status::Unhealthy
        } else {
            self.status
        };

        next.last_check = result;

        next
    }
}

/// A resource whose health can be checked
#[async_trait]
pub trait Checkable: Send + Sync + 'static {
    /// The error reported on a failed health check
    type Error: StdError + Send + Sync + 'static;

    /// The action run to check the current health of the element
    ///
    /// `Ok(())` is interpreted as a passing result. Any `Err(_)`
    /// is interpreted as a failure.
    async fn check(&self) -> Result<(), Self::Error>;

    /// An identifier for the resource
    fn name(&self) -> Cow<str>;
}

/// A wrapper for synchronous functions that can used as a health check
/// data source
#[derive(Debug)]
pub struct FnCheck<F> {
    name: String,
    f: F,
}

impl<F, E> FnCheck<F>
where
    F: Fn() -> Result<(), E> + Send + Sync + 'static,
    E: StdError + Send + Sync + 'static,
{
    /// Constructs a new checkable from a synchronous function
    fn new(name: String, f: F) -> Self {
        Self { name, f }
    }
}

#[async_trait]
impl<F, E> Checkable for FnCheck<F>
where
    F: Fn() -> Result<(), E> + Send + Sync + 'static,
    E: StdError + Send + Sync + 'static,
{
    type Error = E;

    async fn check(&self) -> Result<(), E> {
        (self.f)()
    }

    fn name(&self) -> Cow<str> {
        Cow::Borrowed(&self.name)
    }
}

/// Wraps a synchronous function as a checkable source for health checks
///
/// ## Example
///
/// ```
/// # use std::fmt::Error;
/// # use futures::executor::block_on;
/// use health::Checkable;
///
/// fn all_is_well() -> Result<(), Error> { Ok(()) }
/// fn everything_is_fire() -> Result<(), Error> { Err(Error {}) }
///
/// let always_ok = health::check_fn("ok", all_is_well);
/// assert_eq!(Ok(()), block_on(always_ok.check()));
///
/// let always_err = health::check_fn("err", everything_is_fire);
/// assert_eq!(Err(Error {}), block_on(always_err.check()));
/// ```
pub fn check_fn<F, E>(name: impl Into<String>, f: F) -> FnCheck<F>
where
    F: Fn() -> Result<(), E> + Send + Sync + 'static,
    E: StdError + Send + Sync + 'static,
{
    FnCheck::new(name.into(), f)
}

/// A wrapper for asynchronous functions that can used as a health check
/// data source
#[derive(Debug)]
pub struct FutureCheck<F> {
    name: String,
    f: F,
}

impl<F, X, E> FutureCheck<F>
where
    F: Fn() -> X + Send + Sync + 'static,
    X: Future<Output = Result<(), E>> + Send + Sync + 'static,
    E: StdError + Send + Sync + 'static,
{
    /// Constructs a new checkable from an asynchronous function
    fn new(name: String, f: F) -> Self {
        Self { name, f }
    }
}

#[async_trait]
impl<F, X, E> Checkable for FutureCheck<F>
where
    F: Fn() -> X + Send + Sync + 'static,
    X: Future<Output = Result<(), E>> + Send + Sync + 'static,
    E: StdError + Send + Sync + 'static,
{
    type Error = E;

    async fn check(&self) -> Result<(), E> {
        (self.f)().await
    }

    fn name(&self) -> Cow<str> {
        Cow::Borrowed(&self.name)
    }
}

/// Wraps an asynchronous function as a checkable source for health checks
///
/// ## Example
///
/// ```
/// # use std::fmt::Error;
/// # use futures::executor::block_on;
/// use health::Checkable;
///
/// async fn all_is_well() -> Result<(), Error> { Ok(()) }
/// async fn everything_is_fire() -> Result<(), Error> { Err(Error {}) }
///
/// let always_ok = health::check_future("ok", all_is_well);
/// assert_eq!(Ok(()), block_on(always_ok.check()));
///
/// let always_err = health::check_future("err", everything_is_fire);
/// assert_eq!(Err(Error {}), block_on(always_err.check()));
/// ```
pub fn check_future<F, X, E>(name: impl Into<String>, f: F) -> FutureCheck<F>
where
    F: Fn() -> X + Send + Sync + 'static,
    X: Future<Output = Result<(), E>> + Send + Sync + 'static,
    E: StdError + Send + Sync + 'static,
{
    FutureCheck::new(name.into(), f)
}

/// A health reporter that periodically updates its status based on the result
/// an underlying [`Checkable`][] resource
///
///   [`Checkable`]: trait.Checkable.html
pub struct PeriodicChecker<C> {
    inner: Arc<PeriodicCheckerInner<C>>,
}

impl<C> Clone for PeriodicChecker<C> {
    fn clone(&self) -> Self {
        PeriodicChecker {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<C> fmt::Debug for PeriodicChecker<C> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PeriodicChecker")
            .field("config", &self.inner.config)
            .field("initialized", &self.inner.initialized)
            .field("state", &self.inner.state)
            .finish()
    }
}

impl<C: Checkable> Reporter for PeriodicChecker<C> {
    /// The current health status without consideration for when
    /// the health status was last updated
    #[inline]
    fn raw_status(&self) -> Status {
        self.inner.raw_status()
    }

    /// The current health status or `None` if the health check hasn't
    /// updated its internal state within the leeway time
    #[inline]
    fn status(&self) -> Option<Status> {
        self.inner.status()
    }

    /// The result of the most recent health check
    #[inline]
    fn last_check(&self) -> Check {
        self.inner.last_check()
    }
}

impl<C: Checkable> PeriodicChecker<C> {
    /// Creates a new health check for the [`Checkable`][] resource
    ///
    ///   [`Checkable`]: trait.Checkable.html
    pub fn new(checkable: C, config: Config) -> Self {
        Self {
            inner: Arc::new(PeriodicCheckerInner {
                checkable,
                initialized: Instant::now(),
                state: Mutex::new(State::default()),
                config,
            }),
        }
    }

    /// Begins the health check loop and never returns
    pub async fn run(self) -> ! {
        self.inner.run().await
    }
}

struct PeriodicCheckerInner<C> {
    checkable: C,
    initialized: Instant,
    state: Mutex<State>,
    config: Config,
}

impl<C: Checkable> Reporter for PeriodicCheckerInner<C> {
    fn raw_status(&self) -> Status {
        self.state.lock().status
    }

    fn status(&self) -> Option<Status> {
        let state = self.state.lock();
        let now = self.initialized.elapsed();
        if now - state.last_update > self.config.leeway {
            None
        } else {
            Some(state.status)
        }
    }

    fn last_check(&self) -> Check {
        self.state.lock().last_check
    }
}

impl<C: Checkable> PeriodicCheckerInner<C> {
    async fn run(self: Arc<Self>) -> ! {
        let mut delay = sleep(Duration::from_secs(0));
        loop {
            delay.await;
            delay = sleep(self.config.check_interval);

            #[cfg(feature = "tracing")]
            let check_start = Instant::now();
            let result = self.checkable.check().await;
            #[cfg(feature = "tracing")]
            let check_duration = check_start.elapsed();

            // Note that we are _not_ holding the mutex across an await point.
            // Doing so _could_ result in a deadlock.
            let mut state = self.state.lock();

            let this_check = if result.is_ok() {
                Check::Pass
            } else {
                Check::Failed
            };

            #[cfg(feature = "tracing")]
            let last_state = *state;
            let next_state = state.step(self.initialized.elapsed(), this_check, &self.config);

            *state = next_state;

            drop(state);

            #[cfg(feature = "tracing")]
            {
                use std::convert::TryFrom;

                let error = result.err();
                let module = &*self.checkable.name();

                macro_rules! tracing_event {
                    ($lvl:expr, $err:expr) => {
                        tracing::event!(
                            $lvl,
                            error = $err as &dyn StdError,
                            passing = next_state.last_check == Check::Pass,
                            healthy = next_state.status == Status::Healthy,
                            count = next_state.count,
                            duration =
                                u64::try_from(check_duration.as_millis()).unwrap_or(u64::MAX),
                            module,
                            "healthcheck"
                        );
                    };
                    ($lvl:expr) => {
                        tracing::event!(
                            $lvl,
                            passing = next_state.last_check == Check::Pass,
                            healthy = next_state.status == Status::Healthy,
                            count = next_state.count,
                            duration =
                                u64::try_from(check_duration.as_millis()).unwrap_or(u64::MAX),
                            module,
                            "healthcheck"
                        );
                    };
                }

                match (next_state.status, &error) {
                    // Report errors while unhealthy and still failing health checks
                    (Status::Unhealthy, Some(e)) => {
                        tracing_event!(tracing::Level::ERROR, e);
                    }
                    // Report warnings while healthy but reporting failing health checks
                    (Status::Healthy, Some(e)) => {
                        tracing_event!(tracing::Level::WARN, e);
                    }
                    // Report info while unhealthy but passing health checks
                    (Status::Unhealthy, None) => {
                        tracing_event!(tracing::Level::INFO);
                    }
                    // Report info if just becoming healthy
                    (Status::Healthy, None) if last_state.status == Status::Unhealthy => {
                        tracing_event!(tracing::Level::INFO);
                    }
                    // Report debug if healthy and passing health checks
                    (Status::Healthy, None) => {
                        tracing_event!(tracing::Level::DEBUG);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_check_success() {
        let initial = State { ..State::default() };
        let config = Config {
            min_successes: 2,
            ..Config::default()
        };

        run_test(initial, &config, Status::Healthy, 1, vec![Check::Pass]);
    }

    #[test]
    fn first_check_failure() {
        let initial = State { ..State::default() };
        let config = Config {
            min_successes: 2,
            ..Config::default()
        };

        run_test(initial, &config, Status::Healthy, 1, vec![Check::Failed]);
    }

    #[test]
    fn first_two_checks_failure() {
        let initial = State { ..State::default() };
        let config = Config {
            min_successes: 2,
            ..Config::default()
        };

        run_test(
            initial,
            &config,
            Status::Healthy,
            2,
            vec![Check::Failed, Check::Failed],
        );
    }

    #[test]
    fn first_three_checks_failure() {
        let initial = State { ..State::default() };
        let config = Config {
            min_successes: 2,
            ..Config::default()
        };

        run_test(
            initial,
            &config,
            Status::Unhealthy,
            3,
            vec![Check::Failed, Check::Failed, Check::Failed],
        );
    }

    #[test]
    fn first_three_checks_failure_then_one_success() {
        let initial = State { ..State::default() };
        let config = Config {
            min_successes: 2,
            ..Config::default()
        };

        run_test(
            initial,
            &config,
            Status::Unhealthy,
            1,
            vec![Check::Failed, Check::Failed, Check::Failed, Check::Pass],
        );
    }

    #[test]
    fn first_three_checks_failure_then_one_success_then_fail() {
        let initial = State { ..State::default() };
        let config = Config {
            min_successes: 2,
            ..Config::default()
        };

        run_test(
            initial,
            &config,
            Status::Unhealthy,
            1,
            vec![
                Check::Failed,
                Check::Failed,
                Check::Failed,
                Check::Pass,
                Check::Failed,
            ],
        );
    }

    #[test]
    fn first_three_checks_failure_then_one_success_then_fail_then_pass() {
        let initial = State { ..State::default() };
        let config = Config {
            min_successes: 2,
            ..Config::default()
        };

        run_test(
            initial,
            &config,
            Status::Unhealthy,
            1,
            vec![
                Check::Failed,
                Check::Failed,
                Check::Failed,
                Check::Pass,
                Check::Failed,
                Check::Pass,
            ],
        );
    }

    #[test]
    fn first_three_checks_failure_then_two_success() {
        let initial = State { ..State::default() };
        let config = Config {
            min_successes: 2,
            ..Config::default()
        };

        run_test(
            initial,
            &config,
            Status::Healthy,
            2,
            vec![
                Check::Failed,
                Check::Failed,
                Check::Failed,
                Check::Pass,
                Check::Pass,
            ],
        );
    }

    #[test]
    fn first_three_checks_failure_then_two_success_then_fail() {
        let initial = State { ..State::default() };
        let config = Config {
            min_successes: 2,
            ..Config::default()
        };

        run_test(
            initial,
            &config,
            Status::Healthy,
            1,
            vec![
                Check::Failed,
                Check::Failed,
                Check::Failed,
                Check::Pass,
                Check::Pass,
                Check::Failed,
            ],
        );
    }

    fn run_test(
        initial: State,
        config: &Config,
        expected_status: Status,
        expected_count: u8,
        steps: impl IntoIterator<Item = Check>,
    ) {
        let mut state = initial;
        let mut count = 0;
        let mut last_check = Check::default();
        for check in steps {
            count += 1;
            last_check = check;
            state = state.step(Duration::from_secs(count), check, config);
        }
        let actual = state;

        let expected = State {
            status: expected_status,
            count: expected_count,
            last_update: Duration::from_secs(count),
            last_check,
        };

        assert_eq!(expected, actual);
    }
}
