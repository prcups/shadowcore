// SPDX-License-Identifier: (MIT OR Apache-2.0)

#![deny(unsafe_op_in_unsafe_fn)]

use libc::{size_t, timespec};
use std::convert::TryInto;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::time::Duration;

use blkio::{
    iovec, sigset_t, Blkio, Blkioq, Completion, Errno, Error, MemoryRegion, ReqFlags, Result, State,
};

/// If `f()` evaluates to `Ok`, this returns `0`. Otherwise, this sets the thread-local error
/// message from the `Err` and returns its associated _negated_ errno value.
fn handling_result<F>(f: F) -> c_int
where
    F: FnOnce() -> Result<()>,
{
    handling_result_with_value(|| {
        f()?;
        Ok(0)
    })
}

/// If `f()` evaluates to `Ok`, this returns its value. Otherwise, this sets the thread-local error
/// message from the `Err` and returns its associated _negated_ errno value.
fn handling_result_with_value<F>(f: F) -> c_int
where
    F: FnOnce() -> Result<c_int>,
{
    match f() {
        Ok(value) => value,
        Err(e) => {
            let msg = e.message();
            unsafe { blkio_set_error_msg_(msg.as_ptr() as *const c_char, msg.len()) };
            -(e.errno().raw_os_error())
        }
    }
}

extern "C" {
    fn blkio_get_error_msg_() -> *const c_char;
    fn blkio_set_error_msg_(msg: *const c_char, msg_len: size_t);
}

// Can't reexport directly from error-msg.c, so we create a wrapper here. See:
//     https://github.com/rust-lang/rfcs/issues/2771
#[no_mangle]
pub extern "C" fn blkio_get_error_msg() -> *const c_char {
    unsafe { blkio_get_error_msg_() }
}

/// Holds a [`Blkio`] instance and the corresponding [`Blkioq`]s. Corresponds to `struct blkio` in
/// the C API.
pub struct BlkioWrapper {
    blkio: Blkio,
    queues: Vec<Option<Box<Blkioq>>>,
    poll_queues: Vec<Option<Box<Blkioq>>>,
}

/// # Safety
///
/// The C caller must ensure that the pointers are valid
#[no_mangle]
pub unsafe extern "C" fn blkio_create(cdriver: *const c_char, bp: *mut *mut BlkioWrapper) -> c_int {
    handling_result(|| {
        if bp.is_null() {
            return Err(Error::new(Errno::INVAL, "bp must be non-NULL"));
        }

        unsafe { *bp = ptr::null_mut() };

        let driver_name = unsafe { CStr::from_ptr(cdriver) }
            .to_str()
            .map_err(|_| Error::new(Errno::INVAL, "Invalid driver name"))?;

        let b = Box::new(BlkioWrapper {
            blkio: Blkio::new(driver_name)?,
            queues: Vec::new(),
            poll_queues: Vec::new(),
        });

        unsafe { *bp = Box::into_raw(b) };

        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn blkio_connect(b: &mut BlkioWrapper) -> c_int {
    handling_result(|| {
        if b.blkio.state() != State::Connected {
            b.blkio.connect()?;
        }
        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn blkio_start(b: &mut BlkioWrapper) -> c_int {
    handling_result(|| {
        if b.blkio.state() != State::Started {
            let outcome = b.blkio.start()?;

            let into_some_box = |q| Some(Box::new(q));
            b.queues = outcome.queues.into_iter().map(into_some_box).collect();
            b.poll_queues = outcome.poll_queues.into_iter().map(into_some_box).collect();
        }
        Ok(())
    })
}

/// # Safety
///
/// The C caller must ensure that the pointers are valid
///
#[no_mangle]
pub unsafe extern "C" fn blkio_destroy(bp: *mut *mut BlkioWrapper) {
    if !bp.is_null() {
        unsafe {
            drop(Box::from_raw(*bp));
            *bp = ptr::null_mut();
        }
    }
}

fn get_property<T, G>(cname: *const c_char, value: *mut T, default: T, getter: G) -> c_int
where
    G: FnOnce(&str) -> Result<T>,
{
    handling_result(|| {
        let name = unsafe { CStr::from_ptr(cname) }
            .to_str()
            .map_err(|_| Error::new(Errno::INVAL, "Invalid property name"))?;

        match getter(name) {
            Ok(v) => {
                unsafe { *value = v };
                Ok(())
            }
            Err(e) => {
                unsafe { *value = default };
                Err(e)
            }
        }
    })
}

fn set_property<S>(cname: *const c_char, setter: S) -> c_int
where
    S: FnOnce(&str) -> Result<()>,
{
    handling_result(|| {
        let name = unsafe { CStr::from_ptr(cname) }
            .to_str()
            .map_err(|_| Error::new(Errno::INVAL, "Invalid property name"))?;

        setter(name)
    })
}

/// # Safety
///
/// The C caller must ensure that the pointers are valid
///
#[no_mangle]
pub unsafe extern "C" fn blkio_get_bool(
    b: &BlkioWrapper,
    cname: *const c_char,
    value: *mut bool,
) -> c_int {
    get_property(cname, value, false, |name| b.blkio.get_bool(name))
}

/// # Safety
///
/// The C caller must ensure that the pointers are valid
///
#[no_mangle]
pub unsafe extern "C" fn blkio_get_int(
    b: &BlkioWrapper,
    cname: *const c_char,
    value: *mut c_int,
) -> c_int {
    get_property(cname, value, 0, |name| b.blkio.get_i32(name))
}

/// # Safety
///
/// The C caller must ensure that the pointers are valid
///
#[no_mangle]
pub unsafe extern "C" fn blkio_get_str(
    b: &BlkioWrapper,
    cname: *const c_char,
    value: *mut *mut c_char,
) -> c_int {
    get_property(cname, value, ptr::null_mut(), |name| {
        let value_str = b.blkio.get_str(name)?;
        let value_copy =
            unsafe { libc::strndup(value_str.as_ptr() as *const c_char, value_str.len()) };

        if value_copy.is_null() {
            Err(Error::new(Errno::NOMEM, "Out of memory"))
        } else {
            Ok(value_copy)
        }
    })
}

/// # Safety
///
/// The C caller must ensure that the pointers are valid
///
#[no_mangle]
pub unsafe extern "C" fn blkio_get_uint64(
    b: &BlkioWrapper,
    cname: *const c_char,
    value: *mut u64,
) -> c_int {
    get_property(cname, value, 0, |name| b.blkio.get_u64(name))
}

/// # Safety
///
/// The C caller must ensure that the pointers are valid
///
#[no_mangle]
pub unsafe extern "C" fn blkio_set_bool(
    b: &mut BlkioWrapper,
    cname: *const c_char,
    value: bool,
) -> c_int {
    set_property(cname, |name| b.blkio.set_bool(name, value))
}

/// # Safety
///
/// The C caller must ensure that the pointers are valid
///
#[no_mangle]
pub unsafe extern "C" fn blkio_set_int(
    b: &mut BlkioWrapper,
    cname: *const c_char,
    value: c_int,
) -> c_int {
    set_property(cname, |name| b.blkio.set_i32(name, value))
}

/// # Safety
///
/// The C caller must ensure that the pointers are valid
///
#[no_mangle]
pub unsafe extern "C" fn blkio_set_str(
    b: &mut BlkioWrapper,
    cname: *const c_char,
    cvalue: *const c_char,
) -> c_int {
    set_property(cname, |name| {
        let value = unsafe { CStr::from_ptr(cvalue) }
            .to_str()
            .map_err(|_| Error::new(Errno::INVAL, "Invalid value string"))?;

        b.blkio.set_str(name, value)
    })
}

/// # Safety
///
/// The C caller must ensure that the pointers are valid
///
#[no_mangle]
pub unsafe extern "C" fn blkio_set_uint64(
    b: &mut BlkioWrapper,
    cname: *const c_char,
    value: u64,
) -> c_int {
    set_property(cname, |name| b.blkio.set_u64(name, value))
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct MemRegion {
    addr: *mut c_void,
    len: size_t,
    iova: u64,
    fd_offset: i64,
    fd: c_int,
    flags: u32,
}

impl From<MemRegion> for MemoryRegion {
    fn from(region: MemRegion) -> MemoryRegion {
        MemoryRegion {
            addr: region.addr as usize,
            iova: region.iova,
            len: region.len,
            fd: region.fd,
            fd_offset: region.fd_offset,
            flags: region.flags,
        }
    }
}

impl From<MemoryRegion> for MemRegion {
    fn from(region: MemoryRegion) -> MemRegion {
        MemRegion {
            addr: region.addr as *mut c_void,
            iova: region.iova,
            len: region.len,
            fd_offset: region.fd_offset,
            fd: region.fd,
            flags: region.flags,
        }
    }
}

#[no_mangle]
pub extern "C" fn blkio_alloc_mem_region(
    b: &mut BlkioWrapper,
    region: &mut MemRegion,
    len: size_t,
) -> c_int {
    handling_result(|| {
        let mem_region = b.blkio.alloc_mem_region(len)?;
        *region = mem_region.into();
        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn blkio_free_mem_region(b: &mut BlkioWrapper, region: &MemRegion) {
    b.blkio.free_mem_region(&(*region).into());
}

#[no_mangle]
pub extern "C" fn blkio_map_mem_region(b: &mut BlkioWrapper, region: &MemRegion) -> c_int {
    handling_result(|| b.blkio.map_mem_region(&(*region).into()))
}

#[no_mangle]
pub extern "C" fn blkio_unmap_mem_region(b: &mut BlkioWrapper, region: &MemRegion) {
    b.blkio.unmap_mem_region(&(*region).into());
}

#[no_mangle]
pub extern "C" fn blkio_get_queue(b: &mut BlkioWrapper, index: c_int) -> Option<&mut Blkioq> {
    let index: usize = index.try_into().ok()?;
    b.queues.get_mut(index)?.as_deref_mut()
}

#[no_mangle]
pub extern "C" fn blkio_get_poll_queue(b: &mut BlkioWrapper, index: c_int) -> Option<&mut Blkioq> {
    let index: usize = index.try_into().ok()?;
    b.poll_queues.get_mut(index)?.as_deref_mut()
}

/// Returns index of added queue.
fn add_queue(queues: &mut Vec<Option<Box<Blkioq>>>, q: Blkioq) -> c_int {
    let q = Some(Box::new(q));

    let index = if let Some(i) = queues.iter().position(Option::is_none) {
        queues[i] = q;
        i
    } else {
        queues.push(q);
        queues.len() - 1
    };

    index.try_into().unwrap()
}

#[no_mangle]
pub extern "C" fn blkio_add_queue(b: &mut BlkioWrapper) -> c_int {
    handling_result_with_value(|| {
        let queue = b.blkio.add_queue(false)?;
        Ok(add_queue(&mut b.queues, queue))
    })
}

#[no_mangle]
pub extern "C" fn blkio_add_poll_queue(b: &mut BlkioWrapper) -> c_int {
    handling_result_with_value(|| {
        let poll_queue = b.blkio.add_queue(true)?;
        Ok(add_queue(&mut b.poll_queues, poll_queue))
    })
}

fn remove_queue(queues: &mut [Option<Box<Blkioq>>], index: c_int) -> Result<()> {
    let error = || Error::new(Errno::NOENT, format!("no queue with index {}", index));

    let index: usize = index.try_into().map_err(|_| error())?;
    let q = queues.get_mut(index).ok_or_else(error)?;

    // simply drop the Blkioq
    match q.take() {
        Some(_) => Ok(()),
        None => Err(error()),
    }
}

#[no_mangle]
pub extern "C" fn blkio_remove_queue(b: &mut BlkioWrapper, index: c_int) -> c_int {
    handling_result(|| remove_queue(&mut b.queues, index))
}

#[no_mangle]
pub extern "C" fn blkio_remove_poll_queue(b: &mut BlkioWrapper, index: c_int) -> c_int {
    handling_result(|| remove_queue(&mut b.poll_queues, index))
}

#[no_mangle]
pub extern "C" fn blkioq_read(
    q: &mut Blkioq,
    start: u64,
    buf: *mut c_void,
    len: size_t,
    user_data: *mut c_void,
    flags: ReqFlags,
) {
    q.read(start, buf as *mut u8, len, user_data as usize, flags)
}

#[no_mangle]
pub extern "C" fn blkioq_write(
    q: &mut Blkioq,
    start: u64,
    buf: *const c_void,
    len: size_t,
    user_data: *mut c_void,
    flags: ReqFlags,
) {
    q.write(start, buf as *const u8, len, user_data as usize, flags)
}

#[no_mangle]
pub extern "C" fn blkioq_readv(
    q: &mut Blkioq,
    start: u64,
    iovec: *const iovec,
    iovcnt: c_int,
    user_data: *mut c_void,
    flags: ReqFlags,
) {
    q.readv(start, iovec, iovcnt as u32, user_data as usize, flags)
}

#[no_mangle]
pub extern "C" fn blkioq_writev(
    q: &mut Blkioq,
    start: u64,
    iovec: *const iovec,
    iovcnt: c_int,
    user_data: *mut c_void,
    flags: ReqFlags,
) {
    q.writev(start, iovec, iovcnt as u32, user_data as usize, flags)
}

#[no_mangle]
pub extern "C" fn blkioq_write_zeroes(
    q: &mut Blkioq,
    start: u64,
    len: u64,
    user_data: *mut c_void,
    flags: ReqFlags,
) {
    q.write_zeroes(start, len, user_data as usize, flags)
}

#[no_mangle]
pub extern "C" fn blkioq_discard(
    q: &mut Blkioq,
    start: u64,
    len: u64,
    user_data: *mut c_void,
    flags: ReqFlags,
) {
    q.discard(start, len, user_data as usize, flags)
}

#[no_mangle]
pub extern "C" fn blkioq_flush(q: &mut Blkioq, user_data: *mut c_void, flags: ReqFlags) {
    q.flush(user_data as usize, flags)
}

fn duration_from_timespec(t: &timespec) -> Result<Duration> {
    // The Linux kernel checks these timespec fields in the same way. Check now since the same
    // preconditions are necessary for converting to Duration.
    if t.tv_sec < 0 {
        return Err(Error::new(Errno::INVAL, "tv_sec cannot be negative"));
    }
    if t.tv_nsec as u64 >= 1000000000 {
        return Err(Error::new(
            Errno::INVAL,
            "tv_nsec must be less than one second",
        ));
    }

    Ok(Duration::new(t.tv_sec as u64, t.tv_nsec as u32))
}

fn timespec_from_duration(d: Duration) -> timespec {
    timespec {
        tv_sec: d.as_secs() as _,
        tv_nsec: d.subsec_nanos() as _,
    }
}

fn completions_to_slice<'a>(
    completions: *mut std::mem::MaybeUninit<Completion>,
    min_completions: c_int,
    max_completions: c_int,
) -> Result<&'a mut [std::mem::MaybeUninit<Completion>]> {
    if min_completions < 0 {
        return Err(Error::new(
            Errno::INVAL,
            "min_completions cannot be negative",
        ));
    }
    if max_completions < 0 {
        return Err(Error::new(
            Errno::INVAL,
            "max_completions cannot be negative",
        ));
    }
    if min_completions > max_completions {
        return Err(Error::new(
            Errno::INVAL,
            "min_completions must be less than or equal to max_completions",
        ));
    }
    if completions.is_null() {
        if max_completions > 0 {
            return Err(Error::new(
                Errno::INVAL,
                "max_completions must be 0 when completions is NULL",
            ));
        }

        return Ok(&mut []);
    }
    Ok(unsafe { std::slice::from_raw_parts_mut(completions, max_completions as usize) })
}

#[no_mangle]
pub extern "C" fn blkioq_do_io(
    q: &mut Blkioq,
    completions: *mut std::mem::MaybeUninit<Completion>,
    min_completions: c_int,
    max_completions: c_int,
    timeout: Option<&mut timespec>,
) -> c_int {
    handling_result_with_value(|| {
        let mut duration = timeout.as_deref().map(duration_from_timespec).transpose()?;

        let completions_slice =
            completions_to_slice(completions, min_completions, max_completions)?;

        let result = loop {
            match q
                .do_io(
                    completions_slice,
                    min_completions as usize,
                    duration.as_mut(),
                    None,
                )
                .map(|n| n.try_into().unwrap())
            {
                Err(err) if err.errno() == Errno::INTR => {}
                result => break result,
            }
        };

        if let Some(t) = timeout {
            *t = timespec_from_duration(duration.unwrap());
        }
        result
    })
}

#[no_mangle]
pub extern "C" fn blkioq_do_io_interruptible(
    q: &mut Blkioq,
    completions: *mut std::mem::MaybeUninit<Completion>,
    min_completions: c_int,
    max_completions: c_int,
    timeout: Option<&mut timespec>,
    sig: &sigset_t,
) -> c_int {
    handling_result_with_value(|| {
        let mut duration = timeout.as_deref().map(duration_from_timespec).transpose()?;

        let completions_slice =
            completions_to_slice(completions, min_completions, max_completions)?;

        let result = q
            .do_io(
                completions_slice,
                min_completions as usize,
                duration.as_mut(),
                Some(sig),
            )
            .map(|n| n.try_into().unwrap());

        if let Some(t) = timeout {
            *t = timespec_from_duration(duration.unwrap());
        }
        result
    })
}

#[no_mangle]
pub extern "C" fn blkioq_get_completion_fd(q: &mut Blkioq) -> c_int {
    q.get_completion_fd().unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn blkioq_set_completion_fd_enabled(q: &mut Blkioq, enable: bool) {
    q.set_completion_fd_enabled(enable);
}
