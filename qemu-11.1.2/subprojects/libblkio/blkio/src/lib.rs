// SPDX-License-Identifier: (MIT OR Apache-2.0)

#![deny(unsafe_op_in_unsafe_fn)]

mod drivers;
mod properties;
mod wait;

use crate::properties::Properties;
use crate::wait::TimeoutUpdater;
use bitflags::bitflags;
use libc::c_void;
use rustix::cstr;
use rustix::fs::{ftruncate, memfd_create, MemfdFlags};
use rustix::io::close;
use rustix::mm::{mmap, munmap, MapFlags, ProtFlags};
use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::ffi::CStr;
use std::fmt;
use std::io;
use std::os::unix::io::IntoRawFd;
use std::os::unix::io::RawFd;
use std::result;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{error, ptr};

// Reexport `Errno` and `iovec` since they appear in public APIs.
pub use libc::{c_char, iovec, sigset_t};
pub use rustix::io::Errno;

// Must be kept in sync with include/blkio.h. Also, when adding a flag, make sure to add a
// corresponding arm in the match expression in validate_req_flags().
bitflags! {
    #[repr(transparent)]
    pub struct ReqFlags: u32 {
        const FUA = 1 << 0;
        const NO_UNMAP = 1 << 1;
        const NO_FALLBACK = 1 << 2;
    }
}

/// Returns a [`Completion`] if the request's flags are invalid.
fn validate_req_flags(req: &Request, allowed: ReqFlags) -> Option<Completion> {
    if allowed.contains(req.flags) {
        None
    } else if !ReqFlags::all().contains(req.flags) {
        Some(Completion::for_failed_req(
            req,
            Errno::INVAL,
            cstr!("unsupported bits in request flags"),
        ))
    } else {
        let first_disallowed_flag = 1 << (req.flags & !allowed).bits().trailing_zeros();
        let first_disallowed_flag = ReqFlags::from_bits(first_disallowed_flag).unwrap();

        let error_msg = match first_disallowed_flag {
            ReqFlags::FUA => cstr!("BLKIO_REQ_FUA is invalid for this request type"),
            ReqFlags::NO_UNMAP => {
                cstr!("BLKIO_REQ_NO_UNMAP is invalid for this request type")
            }
            ReqFlags::NO_FALLBACK => {
                cstr!("BLKIO_REQ_NO_FALLBACK is invalid for this request type")
            }
            _ => panic!(),
        };

        Some(Completion::for_failed_req(req, Errno::INVAL, error_msg))
    }
}

#[derive(Debug)]
pub struct Error {
    errno: Errno,
    message: Cow<'static, str>,
}

impl Error {
    pub fn new<M>(errno: Errno, message: M) -> Self
    where
        Cow<'static, str>: From<M>,
    {
        Self {
            errno,
            message: message.into(),
        }
    }

    pub fn from_io_error(io_error: io::Error, default_errno: Errno) -> Self {
        Self {
            errno: io_error
                .raw_os_error()
                .map(Errno::from_raw_os_error)
                .unwrap_or(default_errno),
            message: io_error.to_string().into(),
        }
    }

    pub fn from_last_os_error() -> Self {
        let io_error = io::Error::last_os_error();
        Self {
            errno: Errno::from_raw_os_error(io_error.raw_os_error().unwrap()),
            message: io_error.to_string().into(),
        }
    }

    pub fn errno(&self) -> Errno {
        self.errno
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl error::Error for Error {}

impl From<Errno> for Error {
    fn from(errno: Errno) -> Self {
        Self {
            errno,
            message: errno.to_string().into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub type Result<T> = result::Result<T, Error>;

// This is the same as struct blkio_completion
#[repr(C)]
pub struct Completion {
    pub user_data: usize,
    pub error_msg: *const c_char,
    pub ret: i32,
    pub reserved_: [u8; 12],
}

// `Send` and `Sync` are not implemented automatically due to the `error_msg` pointer.
unsafe impl Send for Completion {}
unsafe impl Sync for Completion {}

impl Completion {
    pub(crate) fn for_successful_req(req: &Request) -> Self {
        Self {
            user_data: req.user_data,
            ret: 0,
            error_msg: ptr::null(),
            reserved_: [0; 12],
        }
    }

    /// Build a Completion for a given Request
    ///
    /// Use [`rustix::cstr!`] to construct an error message. `&'static CStr` is used instead of
    /// `&'static str` so that callers don't have to remember to add a trailing `\0`.
    pub(crate) fn for_failed_req(req: &Request, errno: Errno, error_msg: &'static CStr) -> Self {
        Self {
            user_data: req.user_data,
            ret: -errno.raw_os_error(),
            error_msg: error_msg.as_ptr(),
            reserved_: [0; 12],
        }
    }
}

#[derive(Clone)]
pub(crate) enum IoVecArray {
    RawBorrowed { iovec: *const iovec, iovcnt: u32 },
    Owned { iovec: Box<[iovec]> },
}

unsafe fn iovecarray_as_slice(iov: &IoVecArray) -> &[iovec] {
    match iov {
        IoVecArray::RawBorrowed { iovec, iovcnt } => unsafe {
            std::slice::from_raw_parts(*iovec, *iovcnt as usize)
        },
        IoVecArray::Owned { iovec } => iovec.as_ref(),
    }
}

impl IoVecArray {
    /// Forms an iovec array from a pointer and a length
    fn from_raw_parts(iovec: *const iovec, iovcnt: u32) -> Self {
        Self::RawBorrowed { iovec, iovcnt }
    }

    fn from_buffer(buf: *const u8, len: usize) -> Self {
        Self::Owned {
            iovec: Box::new([iovec {
                iov_base: buf as *mut c_void,
                iov_len: len,
            }]),
        }
    }

    fn as_ptr(&self) -> *const iovec {
        match self {
            IoVecArray::RawBorrowed { iovec, .. } => *iovec,
            IoVecArray::Owned { iovec } => iovec.as_ptr(),
        }
    }

    /// Returns the number of iovec elements in the array
    fn len(&self) -> u32 {
        match self {
            IoVecArray::RawBorrowed { iovcnt, .. } => *iovcnt,
            IoVecArray::Owned { iovec } => iovec.len() as u32,
        }
    }

    /// Returns the total number of bytes
    unsafe fn buffer_size(&self) -> usize {
        unsafe { iovecarray_as_slice(self) }
            .iter()
            .map(|iov| iov.iov_len)
            .sum()
    }

    unsafe fn offset(&self, count: usize) -> Self {
        let current_array = unsafe { iovecarray_as_slice(self) };

        // let's find the first iovec element containing the offset
        let mut current_array_idx = current_array.iter().enumerate();
        let mut len: usize = 0;
        let (iov_idx, offset) = loop {
            let (iov_idx, iov) = current_array_idx
                .next()
                .expect("the offset should be less than buffer size");
            len += iov.iov_len;
            if count < len {
                let offset = iov.iov_len - (len - count);
                break (iov_idx, offset);
            }
        };

        // copy the rest of the iovec vector
        let mut new_array = current_array[iov_idx..].to_vec();

        // advance the first iovec base pointer and length, if needed
        let first_buf = unsafe { new_array[0].iov_base.add(offset) };
        let first_len = new_array[0].iov_len - offset;
        new_array[0] = iovec {
            iov_base: first_buf,
            iov_len: first_len,
        };

        Self::Owned {
            iovec: new_array.into_boxed_slice(),
        }
    }

    unsafe fn fill_with_zeroes(&mut self) {
        for iovec in unsafe { iovecarray_as_slice(self) } {
            unsafe {
                ptr::write_bytes(iovec.iov_base.cast::<u8>(), 0, iovec.iov_len);
            }
        }
    }
}

#[derive(Clone)]
pub(crate) enum RequestTypeArgs {
    Read {
        start: u64,
        buf: *mut u8,
        len: usize,
    },
    Write {
        start: u64,
        buf: *const u8,
        len: usize,
    },
    Readv {
        start: u64,
        iovec: IoVecArray,
    },
    Writev {
        start: u64,
        iovec: IoVecArray,
    },
    WriteZeroes {
        start: u64,
        len: u64,
    },
    Discard {
        start: u64,
        len: u64,
    },
    Flush,
}

// `Send` and `Sync` are not implemented automatically due to the pointer fields.
unsafe impl Send for RequestTypeArgs {}
unsafe impl Sync for RequestTypeArgs {}

/// A handy struct for request arguments
#[derive(Clone)]
pub(crate) struct Request {
    pub(crate) args: RequestTypeArgs,
    pub(crate) user_data: usize,
    pub(crate) flags: ReqFlags,
}

pub(crate) trait Queue: Send + Sync {
    fn is_poll_queue(&self) -> bool;

    fn get_completion_fd(&self) -> Option<RawFd>;

    fn set_completion_fd_enabled(&mut self, enabled: bool);

    /// Enqueue a request if there is enough space, returning true on success.
    fn try_enqueue(
        &mut self,
        completion_backlog: &mut CompletionBacklog,
        req: Request,
    ) -> result::Result<(), Request>;

    fn do_io(
        &mut self,
        request_backlog: &mut RequestBacklog,
        completion_backlog: &mut CompletionBacklog,
        completions: &mut [std::mem::MaybeUninit<Completion>],
        min_completions: usize,
        timeout_updater: Option<&mut TimeoutUpdater>,
        sig: Option<&sigset_t>,
    ) -> Result<usize>;
}

/// Requests waiting to be enqueued or submitted. Used when `Queue::try_enqueue()` does not have
/// space for a request.
pub(crate) struct RequestBacklog {
    reqs: VecDeque<Request>,
}

impl RequestBacklog {
    fn new() -> RequestBacklog {
        RequestBacklog {
            reqs: VecDeque::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.reqs.len()
    }

    /// Try to enqueue a request with `try_enqueue()`. If the queue is full, put the request on the
    /// backlog.
    fn enqueue_or_backlog(
        &mut self,
        queue: &mut dyn Queue,
        completion_backlog: &mut CompletionBacklog,
        req: Request,
    ) {
        if self.reqs.is_empty() {
            if let Err(req) = queue.try_enqueue(completion_backlog, req) {
                self.reqs.push_back(req);
            }
        } else {
            self.reqs.push_back(req);
        }
    }

    /// Enqueue as many backlogged requests as possible and return the count
    pub(crate) fn process(
        &mut self,
        queue: &mut dyn Queue,
        completion_backlog: &mut CompletionBacklog,
    ) -> usize {
        let mut count = 0;
        while let Some(req) = self.reqs.pop_front() {
            if let Err(req) = queue.try_enqueue(completion_backlog, req) {
                self.reqs.push_front(req); // reinsert the request into the backlog
                break;
            }
            count += 1;
        }
        count
    }
}

/// Completions waiting to be reaped by the application. Typically used for ENOTSUP and EINVAL
/// cases while enqueuing requests. Also used to hold completions until the next call when
/// Queue.do_io() has reaped some completions but needs to return an error.
pub(crate) struct CompletionBacklog {
    completions: VecDeque<Completion>,
    completion_fd: Option<RawFd>,
}

impl CompletionBacklog {
    fn new(completion_fd: Option<RawFd>) -> Self {
        Self {
            completions: VecDeque::new(),
            completion_fd,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.completions.len()
    }

    /// Notify the application that completions are available
    fn signal_completion_fd(&mut self) {
        if let Some(fd) = self.completion_fd {
            let val: u64 = 1;
            let valp: *const u64 = &val;
            unsafe { libc::write(fd, valp.cast(), std::mem::size_of::<u64>()) };
        }
    }

    /// Add a Completion to the backlog
    pub(crate) fn push(&mut self, completion: Completion) {
        self.completions.push_back(completion);
        self.signal_completion_fd();
    }

    /// Fill completions[] from the backlog, oldest-first
    pub(crate) fn fill_completions(
        &mut self,
        completions: &mut [std::mem::MaybeUninit<Completion>],
    ) -> usize {
        let mut n = 0;
        for c in completions.iter_mut().take(self.completions.len()) {
            let val = self.completions.pop_front().unwrap();
            unsafe { c.as_mut_ptr().write(val) };
            n += 1;
        }
        n
    }

    /// Prepend filled completions[] elements to the backlog
    pub(crate) fn unfill_completions(
        &mut self,
        completions: &mut [std::mem::MaybeUninit<Completion>],
        count: usize,
    ) {
        for c in completions[..count].iter().rev() {
            self.completions.push_front(unsafe { c.as_ptr().read() });
        }
        self.signal_completion_fd();
    }
}

/// Dropping a `Blkioq` will safely remove the queue from the driver. Depending on the driver, this
/// may eagerly free up resources that were dedicated to the queue.
pub struct Blkioq {
    queue: Box<dyn Queue>,
    request_backlog: RequestBacklog,
    completion_backlog: CompletionBacklog,

    // This `Arc` is never accessed, and is here only to ensure that allocated memory regions are
    // freed only once both the `Blkio` and all associated `Blkioq`s are dropped. Keep this as the
    // last field so that allocated memory regions are never freed prior to `queue` being dropped.
    // (Rust guarantees that fields are dropped in the same order as they are declared.)
    #[allow(dead_code)]
    mem_region_reg: Arc<Mutex<MemoryRegionRegistry>>,
}

impl Blkioq {
    pub(crate) fn new(
        mem_region_reg: Arc<Mutex<MemoryRegionRegistry>>,
        queue: Box<dyn Queue>,
    ) -> Self {
        let completion_fd = queue.get_completion_fd();
        Blkioq {
            queue,
            request_backlog: RequestBacklog::new(),
            completion_backlog: CompletionBacklog::new(completion_fd),
            mem_region_reg,
        }
    }

    pub fn get_completion_fd(&self) -> Option<RawFd> {
        self.queue.get_completion_fd()
    }

    pub fn set_completion_fd_enabled(&mut self, enabled: bool) {
        self.queue.set_completion_fd_enabled(enabled);
    }

    fn enqueue(&mut self, allowed_flags: ReqFlags, req: Request) {
        if let Some(completion) = validate_req_flags(&req, allowed_flags) {
            self.completion_backlog.push(completion);
            return;
        }

        self.request_backlog
            .enqueue_or_backlog(&mut *self.queue, &mut self.completion_backlog, req)
    }

    pub fn read(
        &mut self,
        start: u64,
        buf: *mut u8,
        len: usize,
        user_data: usize,
        flags: ReqFlags,
    ) {
        self.enqueue(
            ReqFlags::empty(),
            Request {
                args: RequestTypeArgs::Read { start, buf, len },
                user_data,
                flags,
            },
        )
    }

    pub fn write(
        &mut self,
        start: u64,
        buf: *const u8,
        len: usize,
        user_data: usize,
        flags: ReqFlags,
    ) {
        self.enqueue(
            ReqFlags::FUA,
            Request {
                args: RequestTypeArgs::Write { start, buf, len },
                user_data,
                flags,
            },
        )
    }

    pub fn readv(
        &mut self,
        start: u64,
        iovec: *const iovec,
        iovcnt: u32,
        user_data: usize,
        flags: ReqFlags,
    ) {
        let req = Request {
            args: RequestTypeArgs::Readv {
                start,
                iovec: IoVecArray::from_raw_parts(iovec, iovcnt),
            },
            user_data,
            flags,
        };

        if iovcnt > i32::MAX as u32 {
            self.completion_backlog.push(Completion::for_failed_req(
                &req,
                Errno::INVAL,
                cstr!("iovcnt must be non-negative and fit in a signed 32-bit integer"),
            ));
            return;
        }

        self.enqueue(ReqFlags::empty(), req)
    }

    pub fn writev(
        &mut self,
        start: u64,
        iovec: *const iovec,
        iovcnt: u32,
        user_data: usize,
        flags: ReqFlags,
    ) {
        let req = Request {
            args: RequestTypeArgs::Writev {
                start,
                iovec: IoVecArray::from_raw_parts(iovec, iovcnt),
            },
            user_data,
            flags,
        };

        if iovcnt > i32::MAX as u32 {
            self.completion_backlog.push(Completion::for_failed_req(
                &req,
                Errno::INVAL,
                cstr!("iovcnt must be non-negative and fit in a signed 32-bit integer"),
            ));
            return;
        }

        self.enqueue(ReqFlags::FUA, req)
    }

    pub fn write_zeroes(&mut self, start: u64, len: u64, user_data: usize, flags: ReqFlags) {
        self.enqueue(
            ReqFlags::FUA | ReqFlags::NO_UNMAP | ReqFlags::NO_FALLBACK,
            Request {
                args: RequestTypeArgs::WriteZeroes { start, len },
                user_data,
                flags,
            },
        )
    }

    pub fn discard(&mut self, start: u64, len: u64, user_data: usize, flags: ReqFlags) {
        self.enqueue(
            ReqFlags::empty(),
            Request {
                args: RequestTypeArgs::Discard { start, len },
                user_data,
                flags,
            },
        )
    }

    pub fn flush(&mut self, user_data: usize, flags: ReqFlags) {
        self.enqueue(
            ReqFlags::empty(),
            Request {
                args: RequestTypeArgs::Flush,
                user_data,
                flags,
            },
        );
    }

    pub fn do_io(
        &mut self,
        completions: &mut [std::mem::MaybeUninit<Completion>],
        min_completions: usize,
        timeout: Option<&mut Duration>,
        sig: Option<&sigset_t>,
    ) -> Result<usize> {
        if sig.is_some() && self.queue.is_poll_queue() {
            return Err(Error::new(
                Errno::NOTSUP,
                "blkioq_do_io_interruptible() is not supported on poll queues",
            ));
        }

        let mut timeout_updater = timeout.as_deref().map(|t| TimeoutUpdater::new(*t));

        let result = self.queue.do_io(
            &mut self.request_backlog,
            &mut self.completion_backlog,
            completions,
            min_completions,
            timeout_updater.as_mut(),
            sig,
        );

        if let Some(timeout) = timeout {
            *timeout = timeout_updater.unwrap().next();
        };

        result
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd)]
pub enum State {
    Created,   // after blkio_new()
    Connected, // after blkio_connect()
    Started,   // after blkio_start()
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct MemoryRegion {
    pub addr: usize,
    pub iova: u64,
    pub len: usize,
    pub fd: RawFd,
    pub fd_offset: i64,
    pub flags: u32,
}

/// The successful return value of [`Driver::start`], holding the created queues and poll queues.
struct DriverStartOutcome {
    queues: Vec<Box<dyn Queue>>,
    poll_queues: Vec<Box<dyn Queue>>,
}

/// A callable that deallocates memory regions.
type MemoryRegionDeallocator = Box<dyn Fn(&MemoryRegion) + Send + Sync>;

/// Holds state for keeping track of allocated and mapped memory regions, and frees allocated memory
/// regions when dropped.
struct MemoryRegionRegistry {
    region_deallocator: MemoryRegionDeallocator,
    allocated_regions: HashSet<MemoryRegion>,
    mapped_regions: HashSet<MemoryRegion>, // not necessarily a subset of `allocated_regions`
}

impl Drop for MemoryRegionRegistry {
    fn drop(&mut self) {
        // Mapped memory regions are implicitly unmapped by drivers when the latter are dropped, so
        // we don't need to unmap them here.

        for region in &self.allocated_regions {
            (self.region_deallocator)(region);
        }
    }
}

trait Driver: Properties + Send + Sync {
    fn state(&self) -> State;
    fn connect(&mut self) -> Result<()>;
    fn start(&mut self) -> Result<DriverStartOutcome>;

    fn add_queue(&mut self, _poll_queue: bool) -> Result<Box<dyn Queue>> {
        Err(Error::new(
            Errno::NOTSUP,
            "Driver does not support dynamically adding queues",
        ))
    }

    /// The allocated region is _not_ mapped by this method.
    fn alloc_mem_region(&mut self, len: usize) -> Result<MemoryRegion> {
        if self.state() < State::Connected {
            return Err(properties::error_must_be_connected());
        }

        let align = self.get_u64("mem-region-alignment")? as usize;

        if len % align != 0 {
            return Err(Error::new(
                Errno::INVAL,
                format!("len {} violates mem-region-alignment {}", len, align),
            ));
        }

        let fd = memfd_create("libblkio-buf", MemfdFlags::empty())?;

        ftruncate(&fd, len as u64)?;

        let addr = unsafe {
            mmap(
                ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &fd,
                0,
            )?
        };

        // Give up if the address is unaligned. Don't attempt to align manually because
        // "mem-region-alignment" should not exceed the page size in practice.
        if (addr as usize) % align != 0 {
            unsafe { munmap(addr, len)? };

            return Err(Error::new(
                Errno::OVERFLOW,
                format!(
                    "Address {} violates mem-region-alignment {}",
                    addr as usize, align,
                ),
            ));
        }

        Ok(MemoryRegion {
            addr: addr as usize,
            iova: 0,
            len,
            fd: fd.into_raw_fd(),
            fd_offset: 0,
            flags: 0,
        })
    }

    /// The given region must _not_ be mapped when the returned callable is called.
    fn get_mem_region_deallocator(&self) -> MemoryRegionDeallocator {
        Box::new(|region| {
            let _ = unsafe { munmap(region.addr as *mut c_void, region.len) };
            unsafe { close(region.fd) };
        })
    }

    fn map_mem_region(&mut self, region: &MemoryRegion) -> Result<()>;

    /// Note that drivers must implicitly unmap any mapped regions when dropped.
    fn unmap_mem_region(&mut self, region: &MemoryRegion);
}

/// The successful return value of [`Blkio::start`], holding the created queues and poll queues.
pub struct BlkioStartOutcome {
    pub queues: Vec<Blkioq>,
    pub poll_queues: Vec<Blkioq>,
}

pub struct Blkio {
    driver: Box<dyn Driver>,

    // Keep this last so that allocated regions are never freed prior to `driver` being dropped.
    // (Rust guarantees that fields are dropped in the same order as they are declared.)
    mem_region_reg: Arc<Mutex<MemoryRegionRegistry>>,
}

impl Blkio {
    pub fn new(driver_name: &str) -> Result<Blkio> {
        let driver: Box<dyn Driver> = match driver_name {
            #[cfg(feature = "io_uring")]
            "io_uring" => Box::new(drivers::iouring::IoUring::new()),
            #[cfg(feature = "nvme-io_uring")]
            "nvme-io_uring" => Box::new(drivers::nvme_io_uring::NvmeIoUring::new()),
            #[cfg(feature = "virtio-blk-vfio-pci")]
            drivers::virtio_blk::VFIO_PCI_DRIVER => {
                Box::new(drivers::virtio_blk::VirtioBlk::new(driver_name))
            }
            #[cfg(feature = "virtio-blk-vhost-user")]
            drivers::virtio_blk::VHOST_USER_DRIVER => {
                Box::new(drivers::virtio_blk::VirtioBlk::new(driver_name))
            }
            #[cfg(feature = "virtio-blk-vhost-vdpa")]
            drivers::virtio_blk::VHOST_VDPA_DRIVER => {
                Box::new(drivers::virtio_blk::VirtioBlk::new(driver_name))
            }
            _ => return Err(Error::new(Errno::NOENT, "Unknown driver name")),
        };

        let mem_region_reg = MemoryRegionRegistry {
            region_deallocator: driver.get_mem_region_deallocator(),
            allocated_regions: HashSet::new(),
            mapped_regions: HashSet::new(),
        };

        Ok(Blkio {
            driver,
            mem_region_reg: Arc::new(Mutex::new(mem_region_reg)),
        })
    }

    pub fn state(&self) -> State {
        self.driver.state()
    }

    pub fn connect(&mut self) -> Result<()> {
        self.driver.connect()
    }

    pub fn start(&mut self) -> Result<BlkioStartOutcome> {
        let outcome = self.driver.start()?;

        let into_blkioq = |q| Blkioq::new(Arc::clone(&self.mem_region_reg), q);

        let queues = outcome.queues.into_iter().map(into_blkioq).collect();
        let poll_queues = outcome.poll_queues.into_iter().map(into_blkioq).collect();

        Ok(BlkioStartOutcome {
            queues,
            poll_queues,
        })
    }

    pub fn add_queue(&mut self, poll_queue: bool) -> Result<Blkioq> {
        let q = self.driver.add_queue(poll_queue)?;
        let blkioq = Blkioq::new(Arc::clone(&self.mem_region_reg), q);
        Ok(blkioq)
    }

    pub fn get_bool(&self, name: &str) -> Result<bool> {
        self.driver.get_bool(name)
    }

    pub fn get_i32(&self, name: &str) -> Result<i32> {
        self.driver.get_i32(name)
    }

    pub fn get_str(&self, name: &str) -> Result<String> {
        self.driver.get_str(name)
    }

    pub fn get_u64(&self, name: &str) -> Result<u64> {
        self.driver.get_u64(name)
    }

    pub fn set_bool(&mut self, name: &str, value: bool) -> Result<()> {
        self.driver.set_bool(name, value)
    }

    pub fn set_i32(&mut self, name: &str, value: i32) -> Result<()> {
        self.driver.set_i32(name, value)
    }

    pub fn set_str(&mut self, name: &str, value: &str) -> Result<()> {
        self.driver.set_str(name, value)
    }

    pub fn set_u64(&mut self, name: &str, value: u64) -> Result<()> {
        self.driver.set_u64(name, value)
    }

    /// The allocated region is _not_ mapped by this method.
    ///
    /// If not freed manually, the region will be freed once the `Blkio` and all associated
    /// `Blkioq`s are dropped.
    pub fn alloc_mem_region(&mut self, len: usize) -> Result<MemoryRegion> {
        let region = self.driver.alloc_mem_region(len)?;

        let mut reg = self.mem_region_reg.lock().unwrap();
        assert!(reg.allocated_regions.insert(region));

        Ok(region)
    }

    /// The given region must _not_ be mapped when this method is called.
    pub fn free_mem_region(&mut self, region: &MemoryRegion) {
        let mut reg = self.mem_region_reg.lock().unwrap();
        assert!(!reg.mapped_regions.contains(region));
        assert!(reg.allocated_regions.remove(region));
        (reg.region_deallocator)(region);
    }

    /// An error is returned if the region is already mapped.
    pub fn map_mem_region(&mut self, region: &MemoryRegion) -> Result<()> {
        let align = self.get_u64("mem-region-alignment")? as usize;

        if region.addr % align != 0 {
            return Err(Error::new(
                Errno::INVAL,
                format!(
                    "addr {:#x} violates mem-region-alignment {}",
                    region.addr, align
                ),
            ));
        }

        if region.len % align != 0 {
            return Err(Error::new(
                Errno::INVAL,
                format!(
                    "len {:#x} violates mem-region-alignment {}",
                    region.len, align
                ),
            ));
        }

        let mut reg = self.mem_region_reg.lock().unwrap();

        if reg.mapped_regions.contains(region) {
            return Err(Error::new(Errno::INVAL, "memory region already mapped"));
        }

        self.driver.map_mem_region(region)?;
        reg.mapped_regions.insert(*region);

        Ok(())
    }

    /// This does nothing if the region is not mapped.
    pub fn unmap_mem_region(&mut self, region: &MemoryRegion) {
        if self
            .mem_region_reg
            .lock()
            .unwrap()
            .mapped_regions
            .remove(region)
        {
            self.driver.unmap_mem_region(region);
        }
    }
}
