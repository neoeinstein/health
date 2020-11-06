//! A library to assist in reporting on the health of a system.

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

/// A report of a single health check run
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

/// The status of a health check, accounting for allowable variance
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

#[derive(Clone, Copy, Debug, Default)]
struct State {
    status: Status,
    last_update: Duration,
    last_check: Check,
    count: u8,
}

/// A type that exposes a health check
#[async_trait]
pub trait Checkable {
    /// The error reported on a failed health check
    type Error: std::error::Error + Send + Sync + 'static;

    /// The action run to check the current health of the element
    ///
    /// `Ok(())` is interpreted as a passing result. Any `Err(_)`
    /// is interpreted as a failure.
    async fn check(&self) -> Result<(), Self::Error>;

    /// An identifier for the type of the checkable resource
    fn name(&self) -> Cow<str>;
}

/// A background healthcheck for checking the health of the MySQL Pool
#[derive(Clone, Debug)]
pub struct PeriodicChecker<C> {
    inner: Arc<PeriodicCheckerInner<C>>,
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
    /// Creates a new health check for the MySQL pool
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

#[derive(Debug)]
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
            let result = self.checkable.check().await;
            let mut state = self.state.lock();
            state.last_update = self.initialized.elapsed();
            let prior_status = state.status;

            let error = result.err();

            let this_check = if error.is_none() {
                Check::Pass
            } else {
                Check::Failed
            };

            if state.last_check != this_check {
                state.count = 0;
            }
            state.count = state.count.saturating_add(1);

            let new_status = if this_check == Check::Pass
                && state.status == Status::Unhealthy
                && state.count >= self.config.min_successes
            {
                Status::Healthy
            } else if this_check == Check::Failed
                && state.status == Status::Healthy
                && state.count >= self.config.min_failures
            {
                Status::Unhealthy
            } else {
                state.status
            };

            let count = state.count;

            state.last_check = this_check;
            state.status = new_status;

            drop(state);

            let module = &*self.checkable.name();
            match (new_status, &error) {
                // Report errors while unhealthy and still failing health checks
                (Status::Unhealthy, Some(error)) => {
                    tracing::error!(
                        error = error as &dyn StdError,
                        check = ?this_check,
                        status = ?new_status,
                        count,
                        module,
                        "healthcheck"
                    );
                }
                // Report warnings while healthy but reporting failing health checks
                (Status::Healthy, Some(error)) => {
                    tracing::warn!(
                        error = error as &dyn StdError,
                        check = ?this_check,
                        status = ?new_status,
                        count,
                        module,
                        "healthcheck"
                    );
                }
                // Report info while unhealthy but passing health checks
                (Status::Unhealthy, None) => {
                    tracing::info!(
                        check = ?this_check,
                        status = ?new_status,
                        count,
                        module,
                        "healthcheck"
                    );
                }
                // Report info if just becoming healthy
                (Status::Healthy, None) if prior_status == Status::Unhealthy => {
                    tracing::info!(
                        check = ?this_check,
                        status = ?new_status,
                        count,
                        module,
                        "healthcheck"
                    );
                }
                // Report debug if healthy and passing health checks
                (Status::Healthy, None) => {
                    tracing::debug!(
                        check = ?this_check,
                        status = ?new_status,
                        count,
                        module,
                        "healthcheck"
                    );
                }
            }
        }
    }
}
