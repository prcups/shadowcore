// SPDX-License-Identifier: (MIT OR Apache-2.0)

use crate::{Error, Result};
use libc::{pollfd, ppoll, sigset_t, timespec, POLLIN};
use rustix::fd::BorrowedFd;
use rustix::io::{read, Errno};
use std::convert::TryInto;
use std::os::unix::io::RawFd;
use std::time::{Duration, Instant};

/// Updates a timeout that is consumed across several blocking operations. Call `next()` to get the
/// next timeout value.
pub(crate) struct TimeoutUpdater {
    timeout: Duration,
    last: Option<Instant>,
}

impl TimeoutUpdater {
    pub(crate) fn new(initial_timeout: Duration) -> Self {
        Self {
            timeout: initial_timeout,
            last: None,
        }
    }

    pub fn next(&mut self) -> Duration {
        let now = Instant::now();

        if let Some(last) = self.last {
            self.timeout = match self.timeout.checked_sub(now.duration_since(last)) {
                Some(t) => t,
                None => Duration::new(0, 0),
            };
        }

        self.last = Some(now);
        self.timeout
    }
}

/// Waits for the next completion_fd notification. Drivers can use this to wait for completions
/// in `Queue::do_io()` if they have no other completion waiting mechanism.
pub(crate) fn wait_for_completion_fd(
    fd: RawFd,
    timeout: Option<Duration>,
    sig: Option<&sigset_t>,
) -> Result<()> {
    let mut pfd = pollfd {
        fd,
        events: POLLIN,
        revents: 0,
    };

    let ts = timeout.map(|t| timespec {
        tv_sec: t.as_secs().try_into().unwrap(),
        tv_nsec: t.subsec_nanos().try_into().unwrap(),
    });
    let ts_ptr = ts.as_ref().map_or(std::ptr::null(), |ts| ts);

    let sig_ptr = sig.map_or(std::ptr::null(), |s| s);

    let ret = unsafe { ppoll(&mut pfd, 1, ts_ptr, sig_ptr) };
    if ret < 0 {
        Err(Error::from_last_os_error())
    } else if ret == 0 {
        Err(Error::new(Errno::TIME, "Timed out"))
    } else if ret == 1 && (pfd.revents & POLLIN) == POLLIN {
        // Read eventfd to clear it and ignore failure
        let mut val = [0u8; 8];
        match read(unsafe { BorrowedFd::borrow_raw(fd) }, &mut val) {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::from(e)),
        }
    } else {
        unreachable!()
    }
}

pub(crate) fn loop_until(
    mut predicate: impl FnMut() -> bool,
    timeout_updater: &mut Option<&mut TimeoutUpdater>,
) -> Result<()> {
    while !predicate() {
        if let Some(timeout_updater) = timeout_updater {
            if timeout_updater.next() <= Duration::new(0, 0) {
                return Err(Error::new(Errno::TIME, "Timed out"));
            }
        }

        // `std::hint::spin_loop()` appeared in Rust 1.48, so we can't use it yet.
        #[allow(deprecated)]
        std::sync::atomic::spin_loop_hint(); // TODO: Are we sure we want to call this?
    }

    Ok(())
}
