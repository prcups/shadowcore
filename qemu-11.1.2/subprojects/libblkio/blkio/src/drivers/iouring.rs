// SPDX-License-Identifier: (MIT OR Apache-2.0)

use crate::properties::{properties, PropertiesList, Property};
use crate::wait::{wait_for_completion_fd, TimeoutUpdater};
use crate::{properties, DriverStartOutcome, IoVecArray};
use crate::{
    Completion, CompletionBacklog, Driver, Error, MemoryRegion, Queue, ReqFlags, Request,
    RequestBacklog, RequestTypeArgs, Result, State,
};
use io_uring::cqueue;
use io_uring::opcode::{Fallocate, Fsync, Read, Readv, Write, Writev};
use io_uring::squeue::{self, Entry};
use io_uring::types::{Fd, Fixed, FsyncFlags, SubmitArgs, Timespec};
use libc::{
    c_int, dev_t, iovec, sigset_t, sysconf, ENOTSUP, FALLOC_FL_KEEP_SIZE, FALLOC_FL_PUNCH_HOLE,
    FALLOC_FL_ZERO_RANGE, O_DIRECT, RWF_DSYNC, RWF_HIPRI, _SC_IOV_MAX, _SC_PAGE_SIZE,
};
use rustix::cstr;
use rustix::fd::{BorrowedFd, OwnedFd};
use rustix::fs::{fcntl_getfl, fstatfs, major, minor, seek, OFlags};
use rustix::io::{eventfd, Errno, EventfdFlags, SeekFrom};
use std::convert::TryFrom;
use std::ffi::CStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::num::ParseIntError;
use std::os::linux::fs::MetadataExt;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;
use std::{cmp, iter};
use std::{ptr, result};

// Hardware queue depth 64-128 is common so use that as the default
const NUM_ENTRIES_DEFAULT: i32 = 128;

// The io_uring crate exposes a low-level io_uring_enter(2) interface via IoUring.enter() but the
// flag argument constants are private in io_uring::sys. Redefine the value from <linux/io_uring.h>
// here for now.
const IORING_ENTER_GETEVENTS: u32 = 1;

/// Read a sysfs attribute as a String
fn sysfs_attr_read_string<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let contents = fs::read(path)?;
    Ok(String::from_utf8_lossy(&contents).trim().to_owned())
}

/// Reads a sysfs attribute as an integer
fn sysfs_attr_read<P, T>(path: P) -> io::Result<T>
where
    P: AsRef<Path>,
    T: FromStr<Err = ParseIntError>,
{
    sysfs_attr_read_string(path)?
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Linux block device queue limits
#[derive(Copy, Clone, Debug)]
struct LinuxBlockAttributes {
    logical_block_size: u32,
    physical_block_size: u32,
    optimal_io_size: u32,
    write_zeroes_max_bytes: u64,
    discard_alignment: u32,
    discard_alignment_offset: u32,
    supports_fua_natively: bool,
    flush_needed: bool,
}

impl LinuxBlockAttributes {
    fn from_device_number(device_number: dev_t) -> io::Result<LinuxBlockAttributes> {
        // build sysfs paths

        let dev_dir = PathBuf::from(format!(
            "/sys/dev/block/{}:{}",
            major(device_number),
            minor(device_number)
        ));

        let is_partition = match dev_dir.join("partition").metadata() {
            Ok(_) => true,
            Err(e) if e.kind() == ErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };

        let queue_dir = dev_dir.join(if is_partition { "../queue" } else { "queue" });

        let dev_attr = |name| sysfs_attr_read(dev_dir.join(name));
        let queue_attr_u32 = |name| -> io::Result<u32> { sysfs_attr_read(queue_dir.join(name)) };
        let queue_attr_u64 = |name| -> io::Result<u64> { sysfs_attr_read(queue_dir.join(name)) };
        let queue_attr_string = |name| sysfs_attr_read_string(queue_dir.join(name));

        // retrieve limits

        // As of Linux 5.17, several block drivers report the discard_alignment queue limit
        // incorrectly, setting it to the discard_granularity instead of 0, which we fix here.
        let discard_alignment = queue_attr_u32("discard_granularity")?;
        let discard_alignment_offset = {
            let value = dev_attr("discard_alignment")?;
            if value == discard_alignment {
                0
            } else {
                value
            }
        };

        let supports_fua_natively = queue_attr_u32("fua")? != 0;

        let flush_needed = {
            let value = queue_attr_string("write_cache")?;
            value == "write back"
        };

        // NOTE: When adding more fields to LinuxBlockAttributes, either make sure the queried sysfs
        // files exist in all kernel versions where io_uring may be available, or fall back to a
        // default value if they are missing.

        Ok(LinuxBlockAttributes {
            logical_block_size: queue_attr_u32("logical_block_size")?,
            physical_block_size: queue_attr_u32("physical_block_size")?,
            optimal_io_size: queue_attr_u32("optimal_io_size")?,
            write_zeroes_max_bytes: queue_attr_u64("write_zeroes_max_bytes")?,
            discard_alignment,
            discard_alignment_offset,
            supports_fua_natively,
            flush_needed,
        })
    }
}

/// Information about the block device or regular file that is the target of an [`IoUring`] instance
#[derive(Copy, Clone, Debug)]
struct TargetInfo {
    is_block_device: bool,
    direct: bool,
    read_only: bool,
    request_alignment: i32,
    optimal_io_alignment: i32,
    optimal_io_size: i32,
    supports_write_zeroes_without_fallback: bool,
    discard_alignment: i32,
    discard_alignment_offset: i32,
    supports_fua_natively: bool,
    can_grow: bool,
    flush_needed: bool,
}

impl TargetInfo {
    fn from_file(file: &File) -> io::Result<TargetInfo> {
        let meta = file.metadata()?;

        let file_status_flags = fcntl_getfl(file)?;

        let direct = file_status_flags.contains(OFlags::DIRECT);
        let read_only = !file_status_flags.intersects(OFlags::WRONLY | OFlags::RDWR);

        if meta.file_type().is_block_device() {
            let limits = LinuxBlockAttributes::from_device_number(meta.st_rdev())?;

            let request_alignment = if direct {
                limits.logical_block_size as i32
            } else {
                1
            };

            Ok(TargetInfo {
                is_block_device: true,
                direct,
                read_only,
                request_alignment,
                optimal_io_alignment: limits.physical_block_size as i32,
                optimal_io_size: limits.optimal_io_size as i32,
                supports_write_zeroes_without_fallback: limits.write_zeroes_max_bytes > 0,
                discard_alignment: limits.discard_alignment as i32,
                discard_alignment_offset: limits.discard_alignment_offset as i32,
                supports_fua_natively: limits.supports_fua_natively,
                can_grow: false,
                flush_needed: limits.flush_needed || !direct,
            })
        } else {
            // This can fail if the file system is not backed by a real block device.
            let block_limits = LinuxBlockAttributes::from_device_number(meta.st_dev()).ok();

            let request_alignment = if direct {
                // Fall back to the page size, which should always be enough alignment.
                match block_limits {
                    Some(limits) => limits.logical_block_size as i32,
                    None => {
                        let page_size = unsafe { sysconf(_SC_PAGE_SIZE) };
                        assert!(page_size >= 0);
                        page_size as i32
                    }
                }
            } else {
                1
            };

            // Conservatively fall back to reporting no native FUA support.
            let supports_fua_natively = match block_limits {
                Some(limits) => limits.supports_fua_natively,
                None => false,
            };

            let file_system_block_size = fstatfs(file)?.f_bsize as i32;

            Ok(TargetInfo {
                is_block_device: false,
                direct,
                read_only,
                request_alignment,
                optimal_io_alignment: cmp::max(file_system_block_size, request_alignment),
                optimal_io_size: 0,
                supports_write_zeroes_without_fallback: true,
                // (Correct) file systems don't place alignment restrictions on fallocate(), but
                // property "discard-alignment" must be a multiple of property "request-alignment".
                discard_alignment: request_alignment,
                discard_alignment_offset: 0,
                supports_fua_natively,
                can_grow: true,
                flush_needed: true,
            })
        }
    }
}

fn expected_return_value(req: &Request) -> usize {
    use RequestTypeArgs::{Read, Readv, Write, Writev};
    match req.args {
        Read { len, .. } | Write { len, .. } => len,
        Readv { ref iovec, .. } | Writev { ref iovec, .. } => unsafe { iovec.buffer_size() },
        _ => 0,
    }
}

fn is_read_or_write(req: &Request) -> bool {
    use RequestTypeArgs::{Read, Readv, Write, Writev};
    matches!(
        req.args,
        Read { .. } | Write { .. } | Readv { .. } | Writev { .. }
    )
}

struct Requests {
    all_req_slots: Box<[Option<Request>]>,
    free_req_slots: Vec<usize>,
}

impl Requests {
    fn new(capacity: usize) -> Self {
        let all_req_slots: Vec<Option<Request>> = vec![None; capacity];
        Self {
            all_req_slots: all_req_slots.into_boxed_slice(),
            free_req_slots: (0..capacity).collect(),
        }
    }

    fn len(&self) -> usize {
        self.all_req_slots.len() - self.free_req_slots.len()
    }

    fn insert(&mut self, req: Request) -> result::Result<u64, Request> {
        if self.free_req_slots.is_empty() {
            return Err(req);
        }

        let req_id = self.free_req_slots.pop().unwrap();
        self.all_req_slots[req_id] = req.into();
        Ok(req_id as u64)
    }

    #[allow(dead_code)]
    fn get(&self, req_id: u64) -> &Request {
        let req_id = usize::try_from(req_id).expect("Request ID must fit into a usize");
        self.all_req_slots
            .get(req_id)
            .expect("All in-flight requests are tracked")
            .as_ref()
            .expect("A valid context request access")
    }

    fn remove(&mut self, req_id: u64) -> Request {
        let req_id = usize::try_from(req_id).expect("Request ID must fit into a usize");
        self.free_req_slots.push(req_id);
        self.all_req_slots
            .get_mut(req_id)
            .expect("All in-flight requests are tracked")
            .take()
            .expect("A valid context requested for removal")
    }
}

#[derive(Debug, Copy, Clone)]
enum EntryError {
    OpError {
        errno: Errno,
        error_msg: &'static CStr,
    },
}

struct IoUringQueue {
    target_info: TargetInfo,
    ring: io_uring::IoUring,
    supports_read: bool,
    supports_write: bool,
    supports_fallocate: bool,
    eventfd: Option<OwnedFd>,
    requests: Requests,
}

impl IoUringQueue {
    pub fn new(poll: bool, num_entries: u32, fd: RawFd, target_info: &TargetInfo) -> Result<Self> {
        let mut builder = io_uring::IoUring::builder();
        if poll {
            builder.setup_iopoll();
        }

        let ring = builder
            .build(num_entries)
            .map_err(|e| Error::from_io_error(e, Errno::NOMEM))?;

        // io_uring functionality probing was introduced simultaneously with
        // support for (non-vectored) read, write, and fallocate in kernel
        // version 5.6. If the probe fails, we assume that the kernel doesn't
        // support any of these ops. Otherwise, we use it to check for their
        // availability, just in case the kernel in use was patched to include
        // probing but not those ops.

        let mut supports_read = false;
        let mut supports_write = false;
        let mut supports_fallocate = false;

        let mut probe = io_uring::Probe::new();

        if ring.submitter().register_probe(&mut probe).is_ok() {
            supports_read = probe.is_supported(Read::CODE);
            supports_write = probe.is_supported(Write::CODE);
            supports_fallocate = probe.is_supported(Fallocate::CODE);
        }

        ring.submitter()
            .register_files(&[fd])
            .map_err(|e| Error::from_io_error(e, Errno::NOTSUP))?;

        let eventfd = if poll {
            None
        } else {
            Some(eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?)
        };

        // Both the completion and submission queues can be full simultaneously, so in
        // the worst case we need to hold SQ + CQ number of entries.
        // However, we set the maximum requests capacity to CQ size otherwise risk
        // dropping CQEs prior to Linux 5.5
        let requests_capacity = ring.params().cq_entries();

        // create IoUringQueue here so eventfd is closed on error
        let queue = IoUringQueue {
            target_info: *target_info,
            ring,
            supports_read,
            supports_write,
            supports_fallocate,
            eventfd,
            requests: Requests::new(requests_capacity as usize),
        };

        if let Some(eventfd) = queue.eventfd.as_ref() {
            queue
                .ring
                .submitter()
                .register_eventfd(eventfd.as_raw_fd())
                .map_err(|e| Error::from_io_error(e, Errno::NOTSUP))?;
        }

        Ok(queue)
    }

    /// io_uring_enter(2) with EXT_ARG
    fn enter_with_ext_arg_timeout(
        &mut self,
        min_complete_hint: usize,
        timeout: Duration,
        sig: Option<&sigset_t>,
    ) -> Result<usize> {
        let ts = Timespec::new()
            .sec(timeout.as_secs())
            .nsec(timeout.subsec_nanos());
        let mut submit_args = SubmitArgs::new().timespec(&ts);
        if let Some(s) = sig {
            submit_args = submit_args.sigmask(s);
        }

        self.ring
            .submitter()
            .submit_with_args(min_complete_hint, &submit_args)
            .map_err(|e| Error::from_io_error(e, Errno::INVAL))
    }

    /// io_uring_enter(2) with ppoll(2) completion_fd waiting
    fn enter_with_ppoll_timeout(
        &mut self,
        min_complete_hint: usize,
        timeout: Duration,
        sig: Option<&sigset_t>,
        eventfd: RawFd,
    ) -> Result<usize> {
        // When IORING_FEAT_EXT_ARG is not available we need to implement timeouts ourselves.
        // IORING_OP_TIMEOUT can be used but is tricky because the timeouts themselves are async
        // requests. A slower but simpler approach is wait_for_completion_fd(), which uses ppoll()
        // on the completion fd. Since this is a fallback for pre-5.11 kernels it's okay to use the
        // slow approach.

        let n = self
            .ring
            .submit()
            .map_err(|e| Error::from_io_error(e, Errno::INVAL))?;

        if min_complete_hint > 0 {
            wait_for_completion_fd(eventfd, Some(timeout), sig)?;
        }

        Ok(n)
    }

    /// Submit and wait for completions. `min_complete_hint` indicates how many completions to wait
    /// for, but the function may return as soon as a completion becomes available (this way
    /// enter_with_ppoll_timeout() doesn't need to loop and update the timeout).
    fn enter_with_timeout(
        &mut self,
        min_complete_hint: usize,
        timeout: Duration,
        sig: Option<&sigset_t>,
    ) -> Result<usize> {
        if self.ring.params().is_feature_ext_arg() {
            self.enter_with_ext_arg_timeout(min_complete_hint, timeout, sig)
        } else if let Some(eventfd) = self.eventfd.as_ref() {
            let eventfd = eventfd.as_raw_fd();
            self.enter_with_ppoll_timeout(min_complete_hint, timeout, sig, eventfd)
        } else {
            Err(Error::new(
                Errno::NOTSUP,
                "driver \"io_uring\" only supports calling blkioq_do_io() on a poll queue with a timeout since mainline Linux kernel 5.11",
            ))
        }
    }

    // Fill completions[] from the cq ring and return the count
    fn drain_cqueue(
        &mut self,
        request_backlog: &mut RequestBacklog,
        completion_backlog: &mut CompletionBacklog,
        completions: &mut [std::mem::MaybeUninit<Completion>],
    ) -> usize {
        let mut i = 0;
        while i < completions.len() {
            let cqe = match self.ring.completion().next() {
                Some(cqe) => cqe,
                None => break,
            };

            let req = self.requests.remove(cqe.user_data());
            let expected_ret = expected_return_value(&req);
            let user_data = req.user_data;

            let ret = if cqe.result() < 0 {
                cqe.result()
            } else if expected_ret == cqe.result() as usize {
                if matches!(req.args, RequestTypeArgs::WriteZeroes { .. })
                    && req.flags.contains(ReqFlags::FUA)
                {
                    // Emulate FUA on write zeroes by submitting a flush request.
                    let request = Request {
                        args: RequestTypeArgs::Flush,
                        user_data: req.user_data,
                        flags: ReqFlags::empty(),
                    };
                    request_backlog.enqueue_or_backlog(self, completion_backlog, request);
                    continue;
                } else {
                    0
                }
            } else if expected_ret > cqe.result() as usize && is_read_or_write(&req) {
                if cqe.result() != 0 {
                    self.try_resubmit(
                        request_backlog,
                        completion_backlog,
                        req,
                        cqe.result() as usize,
                    );
                    continue;
                } else if self.target_info.can_grow {
                    self.zeroing_buffer(req.args);
                    0
                } else {
                    -libc::EIO
                }
            } else {
                -libc::EIO
            };

            let c = Completion {
                user_data,
                ret,
                error_msg: ptr::null(),
                reserved_: [0; 12],
            };
            unsafe { completions[i].as_mut_ptr().write(c) };
            i += 1;
        }
        i
    }

    fn try_resubmit(
        &mut self,
        request_backlog: &mut RequestBacklog,
        completion_backlog: &mut CompletionBacklog,
        req: Request,
        num_bytes: usize,
    ) {
        let remaining_bytes = expected_return_value(&req) - num_bytes;

        // advance the start offset to skip the bytes already read/written and
        // also the buffer, which may be partially filled/written
        let args = match req.args {
            RequestTypeArgs::Read { start, buf, .. } => {
                let start = start + num_bytes as u64;
                let offset_buf = unsafe { buf.add(num_bytes) };

                RequestTypeArgs::Read {
                    start,
                    buf: offset_buf,
                    len: remaining_bytes,
                }
            }
            RequestTypeArgs::Write { start, buf, .. } => {
                let start = start + num_bytes as u64;
                let offset_buf = unsafe { buf.add(num_bytes) };

                RequestTypeArgs::Write {
                    start,
                    buf: offset_buf,
                    len: remaining_bytes,
                }
            }
            RequestTypeArgs::Readv { start, iovec } => {
                let start = start + num_bytes as u64;
                let iovec = unsafe { iovec.offset(num_bytes) };

                RequestTypeArgs::Readv { start, iovec }
            }
            RequestTypeArgs::Writev { start, iovec } => {
                let start = start + num_bytes as u64;
                let iovec = unsafe { iovec.offset(num_bytes) };

                RequestTypeArgs::Writev { start, iovec }
            }
            _ => panic!("Operation does not support resubmission"),
        };

        let request = Request {
            args,
            user_data: req.user_data,
            flags: req.flags,
        };
        request_backlog.enqueue_or_backlog(self, completion_backlog, request);
    }

    fn zeroing_buffer(&self, mut args: RequestTypeArgs) {
        match args {
            RequestTypeArgs::Read { buf, len, .. } => unsafe {
                ptr::write_bytes(buf, 0, len);
            },
            RequestTypeArgs::Readv { ref mut iovec, .. } => unsafe { iovec.fill_with_zeroes() },
            RequestTypeArgs::Write { .. } | RequestTypeArgs::Writev { .. } => {} // just ignore write operations
            _ => {
                panic!("Operation not supported");
            }
        }
    }

    fn convert_if_not_supported(&self, req: Request) -> Request {
        match req {
            Request {
                args: RequestTypeArgs::Read { start, buf, len },
                user_data,
                flags,
            } if !self.supports_read => Request {
                args: RequestTypeArgs::Readv {
                    start,
                    iovec: IoVecArray::from_buffer(buf, len),
                },
                user_data,
                flags,
            },
            Request {
                args: RequestTypeArgs::Write { start, buf, len },
                user_data,
                flags,
            } if !self.supports_write => Request {
                args: RequestTypeArgs::Writev {
                    start,
                    iovec: IoVecArray::from_buffer(buf, len),
                },
                user_data,
                flags,
            },
            _ => req,
        }
    }

    fn get_entry(&self, req: &Request) -> result::Result<Entry, EntryError> {
        if self.is_poll_queue() {
            // TODO: io_uring may make more requests compatible with IORING_SETUP_IOPOLL in the
            // future. Programatically determine which requests are compatible somehow.
            let req_is_supported = match req.args {
                RequestTypeArgs::Read { .. }
                | RequestTypeArgs::Write { .. }
                | RequestTypeArgs::Readv { .. }
                | RequestTypeArgs::Writev { .. } => true,

                RequestTypeArgs::Flush { .. }
                | RequestTypeArgs::WriteZeroes { .. }
                | RequestTypeArgs::Discard { .. } => false,
            };

            if !req_is_supported {
                return Err(EntryError::OpError {
                    errno: Errno::NOTSUP,
                    error_msg: cstr!("request not supported on poll queues for this driver"),
                });
            }
        }

        let poll_rw_flags = if self.is_poll_queue() { RWF_HIPRI } else { 0 };

        let entry = match *req {
            Request {
                args: RequestTypeArgs::Read { start, buf, len },
                ..
            } => {
                if len > u32::MAX as usize {
                    return Err(EntryError::OpError {
                        errno: Errno::INVAL,
                        error_msg: cstr!("len must fit in an unsigned 32-bit integer"),
                    });
                }

                Read::new(Fixed(0), buf, len as u32)
                    .offset(start)
                    .rw_flags(poll_rw_flags)
                    .build()
            }
            Request {
                args: RequestTypeArgs::Write { start, buf, len },
                flags,
                ..
            } => {
                if len > u32::MAX as usize {
                    return Err(EntryError::OpError {
                        errno: Errno::INVAL,
                        error_msg: cstr!("len must fit in an unsigned 32-bit integer"),
                    });
                }

                let rw_flags = if flags.contains(ReqFlags::FUA) {
                    RWF_DSYNC
                } else {
                    0
                };

                Write::new(Fixed(0), buf, len as u32)
                    .offset(start)
                    .rw_flags(rw_flags | poll_rw_flags)
                    .build()
            }
            Request {
                args: RequestTypeArgs::Readv { start, ref iovec },
                ..
            } => Readv::new(Fixed(0), iovec.as_ptr(), iovec.len())
                .offset(start)
                .rw_flags(poll_rw_flags)
                .build(),
            Request {
                args: RequestTypeArgs::Writev { start, ref iovec },
                flags,
                ..
            } => {
                let rw_flags = if flags.contains(ReqFlags::FUA) {
                    RWF_DSYNC
                } else {
                    0
                };

                Writev::new(Fixed(0), iovec.as_ptr(), iovec.len())
                    .offset(start)
                    .rw_flags(rw_flags | poll_rw_flags)
                    .build()
            }
            Request {
                args: RequestTypeArgs::Flush,
                ..
            } => Fsync::new(Fixed(0)).flags(FsyncFlags::DATASYNC).build(),
            Request {
                args: RequestTypeArgs::WriteZeroes { start, len },
                flags,
                ..
            } => {
                if !self.supports_fallocate {
                    return Err(EntryError::OpError {
                        errno: Errno::NOTSUP,
                        error_msg: cstr!("the kernel does not support IORING_OP_FALLOCATE"),
                    });
                }

                #[allow(clippy::collapsible_else_if)]
                let mode = if self.target_info.is_block_device {
                    if self.target_info.supports_write_zeroes_without_fallback {
                        if flags.contains(ReqFlags::NO_UNMAP) {
                            // This will make the kernel call `blkdev_issue_zeroout(...,
                            // BLKDEV_ZERO_NOUNMAP)`, which in this case will simply submit a
                            // REQ_OP_WRITE_ZEROES request with flag REQ_NOUNMAP.
                            FALLOC_FL_ZERO_RANGE | FALLOC_FL_KEEP_SIZE
                        } else {
                            // This will make the kernel call `blkdev_issue_zeroout(...,
                            // BLKDEV_ZERO_NOFALLBACK)`, which in this case will simply submit a
                            // REQ_OP_WRITE_ZEROES request without flag REQ_NOUNMAP.
                            FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE
                        }
                    } else {
                        if flags.contains(ReqFlags::NO_FALLBACK) {
                            return Err(EntryError::OpError {
                                errno: Errno::NOTSUP,
                                error_msg: cstr!("the block device does not support write zeroes with BLKIO_REQ_NO_FALLBACK"),
                            });
                        }

                        // This will make the kernel call `blkdev_issue_zeroout(...,
                        // BLKDEV_ZERO_NOUNMAP)`, which in this case will emulate a write zeroes request
                        // using regular writes.
                        FALLOC_FL_ZERO_RANGE | FALLOC_FL_KEEP_SIZE
                    }
                } else {
                    if flags.contains(ReqFlags::NO_UNMAP) {
                        FALLOC_FL_ZERO_RANGE
                    } else {
                        // This does not update the file size when requests extend past EOF, but our docs
                        // specify that io_uring write zeroes requests on regular files may or may not
                        // update the file size, so this behavior is correct.
                        FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE
                    }
                };

                Fallocate::new(Fixed(0), len)
                    .offset(start)
                    .mode(mode)
                    .build()
            }
            Request {
                args: RequestTypeArgs::Discard { start, len },
                ..
            } => {
                if !self.supports_fallocate {
                    return Err(EntryError::OpError {
                        errno: Errno::NOTSUP,
                        error_msg: cstr!("the kernel does not support IORING_OP_FALLOCATE"),
                    });
                }

                const FALLOC_FL_NO_HIDE_STALE: c_int = 0x04;

                Fallocate::new(Fixed(0), len)
                    .offset(start)
                    .mode(FALLOC_FL_PUNCH_HOLE | FALLOC_FL_NO_HIDE_STALE | FALLOC_FL_KEEP_SIZE)
                    .build()
            }
        };
        Ok(entry)
    }
}

fn supports_iopoll(file: &File, target_info: &TargetInfo) -> Result<bool> {
    if !target_info.direct {
        return Ok(false);
    };

    // create ring

    let mut ring = io_uring::IoUring::<squeue::Entry, cqueue::Entry>::builder()
        .setup_iopoll()
        .build(1)
        .map_err(|e| Error::from_io_error(e, Errno::NOMEM))?;

    // submit request

    let iovec = iovec {
        iov_base: ptr::null_mut(),
        iov_len: 0,
    };

    let entry = Readv::new(Fd(file.as_raw_fd()), &iovec, 1).build();
    unsafe { ring.submission().push(&entry).unwrap() };

    // poll for completion

    loop {
        // May return before the cqe is availabe if the process has exceeded
        // its scheduler time slice.
        ring.submit_and_wait(1)
            .map_err(|e| Error::from_io_error(e, Errno::INVAL))?;

        if !ring.completion().is_empty() {
            break;
        }
    }

    let cqe = ring.completion().next().unwrap();

    // check result

    if cqe.result() == 0 {
        Ok(true)
    } else if cqe.result() == -ENOTSUP {
        Ok(false)
    } else {
        Err(Error::new(
            Errno::INVAL,
            format!(
                "Failed to check poll queue support: Readv failed: {}",
                cqe.result()
            ),
        ))
    }
}

impl Queue for IoUringQueue {
    fn is_poll_queue(&self) -> bool {
        self.eventfd.is_none()
    }

    fn get_completion_fd(&self) -> Option<RawFd> {
        Some(self.eventfd.as_ref()?.as_raw_fd())
    }

    fn set_completion_fd_enabled(&mut self, _enabled: bool) {
        // TODO: Set/unset IORING_CQ_EVENTFD_DISABLED. The io-uring crate
        // doesn't support this yet. Modify enter_with_ppoll_timeout() if necessary to ensure
        // completions are not lost when the user had the completion_fd disabled.
    }

    fn try_enqueue(
        &mut self,
        completion_backlog: &mut CompletionBacklog,
        req: Request,
    ) -> result::Result<(), Request> {
        let req = self.convert_if_not_supported(req);
        let request_id = self.requests.insert(req)?;
        let req = self.requests.get(request_id);

        let entry = match self.get_entry(req) {
            Ok(entry) => entry,
            Err(EntryError::OpError { errno, error_msg }) => {
                completion_backlog.push(Completion::for_failed_req(req, errno, error_msg));
                self.requests.remove(request_id);
                return Ok(());
            }
        };
        let entry = entry.user_data(request_id);

        let result = unsafe { self.ring.submission().push(&entry) };
        if result.is_err() {
            let req = self.requests.remove(request_id);
            return Err(req);
        }
        Ok(())
    }

    fn do_io(
        &mut self,
        request_backlog: &mut RequestBacklog,
        completion_backlog: &mut CompletionBacklog,
        completions: &mut [std::mem::MaybeUninit<Completion>],
        min_completions: usize,
        mut timeout_updater: Option<&mut TimeoutUpdater>,
        sig: Option<&sigset_t>,
    ) -> Result<usize> {
        // filled_completions tracks how many elements of completions[] have been filled in
        let mut filled_completions = completion_backlog.fill_completions(completions);

        let n = self.drain_cqueue(
            request_backlog,
            completion_backlog,
            &mut completions[filled_completions..],
        );
        filled_completions += n;

        if n > 0 {
            // Since the number of CQEs in the CQ can affect whether requests are backlogged, we
            // must enqueue backlogged requests after consuming CQEs to ensure we don't end up with
            // an empty SQ and non-empty backlog.
            request_backlog.process(self, completion_backlog);
        }

        if min_completions > filled_completions + self.requests.len() + request_backlog.len() {
            completion_backlog.unfill_completions(completions, filled_completions);
            return Err(Error::new(
                Errno::INVAL,
                "min_completions is larger than total outstanding requests",
            ));
        }

        let mut to_submit = self.ring.submission().len();

        // When the queue is a poll queue, even if filled_completions = 0, min_completions = 0, and
        // to_submit = 0, we must call io_uring_enter() once for any new completions to be found.
        // Otherwise, application-level polling using blkioq_do_io(min_completion = 0) might hang.
        let mut must_enter_once = self.is_poll_queue()
            && filled_completions == 0
            && min_completions == 0
            && to_submit == 0;

        while filled_completions < min_completions || to_submit > 0 || must_enter_once {
            let min_complete_hint = if filled_completions < min_completions {
                // Clamp to number of in-flight requests to avoid hangs when the user provides a
                // min_completions number that is too large.
                std::cmp::min(min_completions - filled_completions, self.requests.len())
            } else {
                0
            };

            let result = if let Some(timeout) = timeout_updater.as_mut().map(|t| t.next()) {
                self.enter_with_timeout(min_complete_hint, timeout, sig)
            } else {
                let flags = if min_complete_hint > 0 || self.is_poll_queue() {
                    IORING_ENTER_GETEVENTS
                } else {
                    0
                };

                unsafe {
                    self.ring
                        .submitter()
                        .enter(to_submit as u32, min_complete_hint as u32, flags, sig)
                        .map_err(|e| Error::from_io_error(e, Errno::INVAL))
                }
            };

            let num_submitted = match result {
                Ok(n) => n,

                // Requests may have been submitted, even on error, and we need to keep count
                Err(_) => to_submit - self.ring.submission().len(),
            };

            // TODO document EAGAIN/EBUSY or try again with to_submit=0 just to reap
            // completions and wait for enough resources to submit again?
            if let Err(err) = result {
                completion_backlog.unfill_completions(completions, filled_completions);
                return Err(err);
            }

            let n = self.drain_cqueue(
                request_backlog,
                completion_backlog,
                &mut completions[filled_completions..],
            );
            filled_completions += n;

            if num_submitted > 0 || n > 0 {
                request_backlog.process(self, completion_backlog);
            }

            to_submit = self.ring.submission().len();
            must_enter_once = false;
        }

        Ok(filled_completions)
    }
}

properties! {
    IOURING_PROPS: PropertyState for IoUring.props {
        fn buf_alignment: i32,
        can_add_queues: bool,
        fn capacity: u64,
        mut direct: bool,
        fn discard_alignment: i32,
        fn discard_alignment_offset: i32,
        driver: str,
        mut fd: i32,
        fn max_discard_len: u64,
        max_queues: i32,
        max_mem_regions: u64,
        fn max_segment_len: i32,
        fn max_segments: i32,
        fn max_transfer: i32,
        fn max_write_zeroes_len: u64,
        may_pin_mem_regions: bool,
        fn mem_region_alignment: u64,
        needs_mem_regions: bool,
        needs_mem_region_fd: bool,
        mut num_entries: i32,
        mut num_queues: i32,
        mut num_poll_queues: i32,
        fn optimal_io_alignment: i32,
        fn optimal_io_size: i32,
        fn optimal_buf_alignment: i32,
        mut path: str,
        mut read_only: bool,
        fn request_alignment: i32,
        supports_fua_natively: bool,
        supports_poll_queues: bool,
        can_grow: bool,
        flush_needed: bool
    }
}

pub struct IoUring {
    props: PropertyState,
    file: Option<File>,
    target_info: Option<TargetInfo>,
    state: State,
}

impl IoUring {
    pub fn new() -> Self {
        IoUring {
            props: PropertyState {
                can_add_queues: true,
                direct: false,
                driver: "io_uring".to_string(),
                fd: -1,
                max_queues: i32::MAX,
                max_mem_regions: u64::MAX,
                may_pin_mem_regions: false,
                needs_mem_regions: false,
                needs_mem_region_fd: false,
                num_entries: NUM_ENTRIES_DEFAULT,
                num_queues: 1,
                num_poll_queues: 0,
                path: String::new(),
                read_only: false,
                supports_fua_natively: false,
                supports_poll_queues: false,
                can_grow: false,
                flush_needed: true,
            },
            file: None,
            target_info: None,
            state: State::Created,
        }
    }

    fn cant_set_while_connected(&self) -> Result<()> {
        if self.state >= State::Connected {
            Err(properties::error_cant_set_while_connected())
        } else {
            Ok(())
        }
    }

    fn cant_set_while_started(&self) -> Result<()> {
        if self.state >= State::Started {
            Err(properties::error_cant_set_while_started())
        } else {
            Ok(())
        }
    }

    fn must_be_connected(&self) -> Result<()> {
        if self.state >= State::Connected {
            Ok(())
        } else {
            Err(properties::error_must_be_connected())
        }
    }

    fn must_be_started(&self) -> Result<()> {
        if self.state >= State::Started {
            Ok(())
        } else {
            Err(Error::new(Errno::BUSY, "Device must be started"))
        }
    }

    fn get_capacity(&self) -> Result<u64> {
        self.must_be_connected()?;
        let fd = unsafe { BorrowedFd::borrow_raw(self.props.fd) };
        Ok(seek(fd, SeekFrom::End(0))?)
    }

    fn set_direct(&mut self, value: bool) -> Result<()> {
        self.cant_set_while_connected()?;
        self.props.direct = value;
        Ok(())
    }

    fn set_fd(&mut self, value: i32) -> Result<()> {
        self.cant_set_while_connected()?;
        self.props.fd = value;
        Ok(())
    }

    // Open the file into self.fd
    fn open_file(&mut self) -> Result<()> {
        if !self.props.path.is_empty() {
            if self.props.fd != -1 {
                return Err(Error::new(
                    Errno::INVAL,
                    "path and fd cannot be set at the same time",
                ));
            }

            let open_flags = if self.props.direct { O_DIRECT } else { 0 };

            let file = OpenOptions::new()
                .custom_flags(open_flags)
                .read(true)
                .write(!self.props.read_only)
                .open(self.props.path.as_str())
                .map_err(|e| Error::from_io_error(e, Errno::INVAL))?;

            self.props.fd = file.as_raw_fd();
            self.assign_file(file)
        } else if self.props.fd != -1 {
            let file = unsafe { File::from_raw_fd(self.props.fd) };
            self.assign_file(file)
        } else {
            Err(Error::new(Errno::INVAL, "One of path and fd must be set"))
        }
    }

    fn assign_file(&mut self, file: File) -> Result<()> {
        let file_type = file
            .metadata()
            .map_err(|e| Error::from_io_error(e, Errno::INVAL))?
            .file_type();

        if !file_type.is_block_device() && !file_type.is_file() {
            return Err(Error::new(
                Errno::INVAL,
                "The file must be a block device or a regular file",
            ));
        }

        let target_info =
            TargetInfo::from_file(&file).map_err(|e| Error::from_io_error(e, Errno::INVAL))?;

        let supports_poll_queues = supports_iopoll(&file, &target_info)?;

        // Set the 'direct' and 'read-only' properties to match the file's actual status flags, in
        // case the user specified it through the 'fd' property.
        self.props.direct = target_info.direct;
        self.props.read_only = target_info.read_only;
        self.props.can_grow = target_info.can_grow;
        self.props.supports_fua_natively = target_info.supports_fua_natively;
        self.props.supports_poll_queues = supports_poll_queues;
        self.props.flush_needed = target_info.flush_needed;

        self.file = Some(file);
        self.target_info = Some(target_info);

        Ok(())
    }

    fn get_max_segment_len(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(0) // unlimited, Linux block layer will split requests if necessary
    }

    fn get_max_segments(&self) -> Result<i32> {
        self.must_be_connected()?;

        // Userspace can submit up to IOV_MAX and the Linux block layer will split requests as
        // needed.
        let iov_max = unsafe { sysconf(_SC_IOV_MAX) };
        assert!(iov_max >= 0);
        Ok(iov_max as i32)
    }

    fn get_max_transfer(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(0) // unlimited, Linux block layer will split requests if necessary
    }

    fn get_max_write_zeroes_len(&self) -> Result<u64> {
        self.must_be_connected()?;
        Ok(0) // unlimited, Linux block layer will split requests if necessary
    }

    fn get_max_discard_len(&self) -> Result<u64> {
        self.must_be_connected()?;
        Ok(0) // unlimited, Linux block layer will split requests if necessary
    }

    fn get_mem_region_alignment(&self) -> Result<u64> {
        // no alignment restrictions but must be multiple of buf-alignment
        Ok(self.get_buf_alignment()? as u64)
    }

    fn get_buf_alignment(&self) -> Result<i32> {
        self.must_be_connected()?;
        self.get_request_alignment()
    }

    fn set_num_entries(&mut self, value: i32) -> Result<()> {
        self.must_be_connected()?;
        self.cant_set_while_started()?;

        // TODO check power of two?
        if value <= 0 {
            return Err(Error::new(
                Errno::INVAL,
                "num-entries must be greater than 0",
            ));
        }

        self.props.num_entries = value;
        Ok(())
    }

    fn set_num_queues(&mut self, value: i32) -> Result<()> {
        self.must_be_connected()?;
        self.cant_set_while_started()?;

        if value < 0 {
            return Err(Error::new(
                Errno::INVAL,
                "num_queues must be equal to or greater than 0",
            ));
        }

        self.props.num_queues = value;
        Ok(())
    }

    fn set_num_poll_queues(&mut self, value: i32) -> Result<()> {
        self.must_be_connected()?;
        self.cant_set_while_started()?;

        if value < 0 {
            return Err(Error::new(
                Errno::INVAL,
                "num_poll_queues must be equal to or greater than 0",
            ));
        }

        self.props.num_poll_queues = value;
        Ok(())
    }

    fn get_optimal_io_alignment(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(self.target_info.as_ref().unwrap().optimal_io_alignment)
    }

    fn get_optimal_io_size(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(self.target_info.as_ref().unwrap().optimal_io_size)
    }

    fn get_optimal_buf_alignment(&self) -> Result<i32> {
        self.must_be_connected()?;
        let page_size = unsafe { sysconf(_SC_PAGE_SIZE) };
        assert!(page_size >= 0);
        Ok(page_size as i32)
    }

    fn set_path(&mut self, value: &str) -> Result<()> {
        self.cant_set_while_connected()?;
        self.props.path = value.to_string();
        Ok(())
    }

    fn set_read_only(&mut self, value: bool) -> Result<()> {
        self.cant_set_while_connected()?;
        self.props.read_only = value;
        Ok(())
    }

    fn get_request_alignment(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(self.target_info.as_ref().unwrap().request_alignment)
    }

    fn get_discard_alignment(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(self.target_info.as_ref().unwrap().discard_alignment)
    }

    fn get_discard_alignment_offset(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(self.target_info.as_ref().unwrap().discard_alignment_offset)
    }
}

impl Driver for IoUring {
    fn state(&self) -> State {
        self.state
    }

    fn connect(&mut self) -> Result<()> {
        self.cant_set_while_connected()?;

        self.open_file()?;
        self.state = State::Connected;
        Ok(())
    }

    fn start(&mut self) -> Result<DriverStartOutcome> {
        self.must_be_connected()?;
        self.cant_set_while_started()?;

        if !self.props.supports_poll_queues && self.props.num_poll_queues > 0 {
            return Err(Error::new(Errno::INVAL, "Poll queues not supported"));
        }

        let target_info = self.target_info.as_ref().unwrap();

        let create_queue = |poll| {
            let q = IoUringQueue::new(
                poll,
                self.props.num_entries as u32,
                self.props.fd,
                target_info,
            )?;
            Ok(Box::new(q) as Box<dyn Queue>)
        };

        let queues = iter::repeat_with(|| create_queue(false))
            .take(self.props.num_queues as usize)
            .collect::<Result<_>>()?;

        let poll_queues = iter::repeat_with(|| create_queue(true))
            .take(self.props.num_poll_queues as usize)
            .collect::<Result<_>>()?;

        self.state = State::Started;

        Ok(DriverStartOutcome {
            queues,
            poll_queues,
        })
    }

    fn add_queue(&mut self, poll_queue: bool) -> Result<Box<dyn Queue>> {
        self.must_be_started()?;

        if !self.props.supports_poll_queues && poll_queue {
            return Err(Error::new(Errno::INVAL, "Poll queues not supported"));
        }

        let q = IoUringQueue::new(
            poll_queue,
            self.props.num_entries as u32,
            self.props.fd,
            self.target_info.as_ref().unwrap(),
        )?;

        Ok(Box::new(q))
    }

    // IORING_REGISTER_BUFFERS could be used in the future to improve performance. Ignore memory
    // regions for now. Remember to set may_pin_mem_regions if necessary in the future.
    fn map_mem_region(&mut self, _region: &MemoryRegion) -> Result<()> {
        self.must_be_started()
    }

    fn unmap_mem_region(&mut self, _region: &MemoryRegion) {}
}
