// SPDX-License-Identifier: (MIT OR Apache-2.0)

use crate::properties::{properties, PropertiesList, Property};
use crate::wait::{loop_until, wait_for_completion_fd, TimeoutUpdater};
use crate::{properties, DriverStartOutcome};
use crate::{
    Completion, CompletionBacklog, Driver, Error, MemoryRegion, ReqFlags, Request, RequestBacklog,
    RequestTypeArgs, Result, State,
};
use libc::sigset_t;
use rustix::cstr;
use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::io::Errno;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;
use std::{ptr, result};
use virtio_driver::{
    EventFd, QueueNotifier, VirtioBlkFeatureFlags, VirtioBlkQueue, VirtioBlkTransport,
    VirtioFeatureFlags,
};

#[cfg(feature = "virtio-blk-vfio-pci")]
pub const VFIO_PCI_DRIVER: &str = "virtio-blk-vfio-pci";
#[cfg(feature = "virtio-blk-vhost-user")]
pub const VHOST_USER_DRIVER: &str = "virtio-blk-vhost-user";
#[cfg(feature = "virtio-blk-vhost-vdpa")]
pub const VHOST_VDPA_DRIVER: &str = "virtio-blk-vhost-vdpa";

// This is the maximum as defined in the virtio spec
const MAX_QUEUE_SIZE: i32 = 32768;
const DEFAULT_QUEUE_SIZE: i32 = 256;

struct ReqContext {
    needs_flush: bool,
    user_data: usize,
}

fn fail_req_due_to_read_only(completion_backlog: &mut CompletionBacklog, req: &Request) {
    completion_backlog.push(Completion::for_failed_req(
        req,
        Errno::BADF,
        cstr!("device is read-only"),
    ));
}

struct Queue<'a> {
    features: VirtioBlkFeatureFlags,
    vq: VirtioBlkQueue<'a, ReqContext>,
    submission_notifier: Box<dyn QueueNotifier>,
    completion_fd: Option<Arc<EventFd>>,
    completion_fd_enabled: bool,
    read_only: bool,
    submissions_to_notify: bool,
    submissions_in_flight: usize,
}

impl<'a> Queue<'a> {
    pub fn new(
        poll: bool,
        features: VirtioBlkFeatureFlags,
        mut vq: VirtioBlkQueue<'a, ReqContext>,
        submission_notifier: Box<dyn QueueNotifier>,
        completion_fd: Arc<EventFd>,
        read_only: bool,
    ) -> Self {
        let completion_fd = if poll { None } else { Some(completion_fd) };

        // Keep completion_fd_enabled and used_notif_enabled in sync.
        // Both are enabled via set_completion_fd_enabled().
        let completion_fd_enabled = false;
        vq.set_used_notif_enabled(completion_fd_enabled);

        Queue {
            features,
            vq,
            submission_notifier,
            completion_fd,
            completion_fd_enabled,
            read_only,
            submissions_to_notify: false,
            submissions_in_flight: 0,
        }
    }

    fn notify_requests(&mut self) -> Result<()> {
        if self.submissions_to_notify && self.vq.avail_notif_needed() {
            self.submission_notifier
                .notify()
                .map_err(|e| Error::from_io_error(e, Errno::IO))?;

            self.submissions_to_notify = false;
        }

        Ok(())
    }

    fn drain_completions(
        &mut self,
        request_backlog: &mut RequestBacklog,
        completion_backlog: &mut CompletionBacklog,
        completions: &mut [std::mem::MaybeUninit<Completion>],
    ) -> usize {
        let mut should_process_request_backlog = false;
        let mut drained = 0;

        // We call `VirtioBlkQueue::completions` on each iteration instead of reusing the iterator
        // so that we can borrow `self` mutably for the `enqueue_or_backlog()` call below.
        // `VirtioBlkQueue::completions` is cheap, so this shouldn't be too problematic.
        while drained < completions.len() {
            let completion = match self.vq.completions().next() {
                Some(c) => c,
                None => break,
            };

            self.submissions_in_flight -= 1;
            should_process_request_backlog = true;

            if !completion.context.needs_flush || completion.ret != 0 {
                unsafe {
                    completions[drained].as_mut_ptr().write(Completion {
                        user_data: completion.context.user_data,
                        ret: completion.ret,
                        error_msg: ptr::null(),
                        reserved_: [0; 12],
                    })
                };
                drained += 1;
            } else {
                let req = Request {
                    args: RequestTypeArgs::Flush,
                    user_data: completion.context.user_data,
                    flags: ReqFlags::empty(),
                };
                request_backlog.enqueue_or_backlog(self, completion_backlog, req);
            }
        }

        if should_process_request_backlog {
            request_backlog.process(self, completion_backlog);
        }

        drained
    }
}

impl crate::Queue for Queue<'_> {
    fn is_poll_queue(&self) -> bool {
        self.completion_fd.is_none()
    }

    fn get_completion_fd(&self) -> Option<RawFd> {
        Some(self.completion_fd.as_ref()?.as_raw_fd())
    }

    fn set_completion_fd_enabled(&mut self, enabled: bool) {
        self.vq.set_used_notif_enabled(enabled);
        self.completion_fd_enabled = enabled;
    }

    fn try_enqueue(
        &mut self,
        completion_backlog: &mut CompletionBacklog,
        req: Request,
    ) -> result::Result<(), Request> {
        let context = ReqContext {
            user_data: req.user_data,
            needs_flush: req.flags.contains(ReqFlags::FUA)
                && self.features.contains(VirtioBlkFeatureFlags::FLUSH),
        };

        let result = match req.args {
            RequestTypeArgs::Read { start, buf, len } => {
                if virtio_driver::validate_lba(start).is_err() {
                    completion_backlog.push(Completion::for_failed_req(
                        &req,
                        Errno::INVAL,
                        cstr!("invalid start offset"),
                    ));
                    return Ok(());
                }

                unsafe { self.vq.read_raw(start, buf, len, context) }
            }
            RequestTypeArgs::Write { start, buf, len } => {
                if virtio_driver::validate_lba(start).is_err() {
                    completion_backlog.push(Completion::for_failed_req(
                        &req,
                        Errno::INVAL,
                        cstr!("invalid start offset"),
                    ));
                    return Ok(());
                }

                if self.read_only {
                    fail_req_due_to_read_only(completion_backlog, &req);
                    return Ok(());
                }

                unsafe { self.vq.write_raw(start, buf, len, context) }
            }
            RequestTypeArgs::Readv { start, ref iovec } => {
                if virtio_driver::validate_lba(start).is_err() {
                    completion_backlog.push(Completion::for_failed_req(
                        &req,
                        Errno::INVAL,
                        cstr!("invalid start offset"),
                    ));
                    return Ok(());
                }

                unsafe {
                    self.vq.readv(
                        start,
                        iovec.as_ptr().cast::<virtio_driver::iovec>(),
                        iovec.len() as usize,
                        context,
                    )
                }
            }
            RequestTypeArgs::Writev { start, ref iovec } => {
                if virtio_driver::validate_lba(start).is_err() {
                    completion_backlog.push(Completion::for_failed_req(
                        &req,
                        Errno::INVAL,
                        cstr!("invalid start offset"),
                    ));
                    return Ok(());
                }

                if self.read_only {
                    fail_req_due_to_read_only(completion_backlog, &req);
                    return Ok(());
                }

                unsafe {
                    self.vq.writev(
                        start,
                        iovec.as_ptr().cast::<virtio_driver::iovec>(),
                        iovec.len() as usize,
                        context,
                    )
                }
            }
            RequestTypeArgs::WriteZeroes { start, len } => {
                if !self.features.contains(VirtioBlkFeatureFlags::WRITE_ZEROES) {
                    completion_backlog.push(Completion::for_failed_req(
                        &req,
                        Errno::NOTSUP,
                        cstr!("write zeroes not supported"),
                    ));
                    return Ok(());
                }

                if virtio_driver::validate_lba(start).is_err()
                    || virtio_driver::validate_lba(start + len).is_err()
                {
                    completion_backlog.push(Completion::for_failed_req(
                        &req,
                        Errno::INVAL,
                        cstr!("invalid len or start offset"),
                    ));
                    return Ok(());
                }

                if self.read_only {
                    fail_req_due_to_read_only(completion_backlog, &req);
                    return Ok(());
                }

                let unmap = !req.flags.contains(ReqFlags::NO_UNMAP);

                self.vq.write_zeroes(start, len, unmap, context)
            }
            RequestTypeArgs::Discard { start, len } => {
                if !self.features.contains(VirtioBlkFeatureFlags::DISCARD) {
                    completion_backlog.push(Completion::for_failed_req(
                        &req,
                        Errno::NOTSUP,
                        cstr!("discard not supported"),
                    ));
                    return Ok(());
                }

                if virtio_driver::validate_lba(start).is_err()
                    || virtio_driver::validate_lba(start + len).is_err()
                {
                    completion_backlog.push(Completion::for_failed_req(
                        &req,
                        Errno::INVAL,
                        cstr!("invalid len or start offset"),
                    ));
                    return Ok(());
                }

                if self.read_only {
                    fail_req_due_to_read_only(completion_backlog, &req);
                    return Ok(());
                }

                self.vq.discard(start, len, context)
            }
            RequestTypeArgs::Flush => {
                if !self.features.contains(VirtioBlkFeatureFlags::FLUSH) {
                    completion_backlog.push(Completion::for_successful_req(&req));
                    return Ok(());
                }

                self.vq.flush(context)
            }
        };

        if result.is_ok() {
            self.submissions_to_notify = true;
            self.submissions_in_flight += 1;
        }

        result.map_err(|_| req)
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
        if min_completions
            > self.submissions_in_flight + completion_backlog.len() + request_backlog.len()
        {
            return Err(Error::new(
                Errno::INVAL,
                "min_completions is larger than total outstanding requests",
            ));
        }

        // If request_backlog is empty, it means that all requests submitted by
        // the user are already queued in the virtqueue, so we can notify the
        // device right away to minimize the latency, without waiting to
        // collect completions.
        if request_backlog.len() == 0 {
            self.notify_requests()?;
        }

        // filled_completions tracks how many elements of completions[] have been filled in
        let mut filled_completions = completion_backlog.fill_completions(completions);
        let mut completion_fd_reenabled = false;

        loop {
            let n = self.drain_completions(
                request_backlog,
                completion_backlog,
                &mut completions[filled_completions..],
            );

            filled_completions += n;

            // drain_completions() can submit new requests in the virtqueue, so
            // we notify the device.
            if let Err(err) = self.notify_requests() {
                completion_backlog.unfill_completions(completions, filled_completions);

                if completion_fd_reenabled {
                    self.set_completion_fd_enabled(false);
                }
                return Err(err);
            }
            if filled_completions >= min_completions {
                break;
            }

            let result = if let Some(fd) = &self.completion_fd {
                if !self.completion_fd_enabled {
                    self.set_completion_fd_enabled(true);
                    completion_fd_reenabled = true;

                    // Recheck for completions to avoid race
                    continue;
                }
                let timeout = timeout_updater.as_mut().map(|tu| tu.next());
                wait_for_completion_fd(fd.as_raw_fd(), timeout, sig)
            } else {
                loop_until(|| self.vq.completions().has_next(), &mut timeout_updater)
            };

            if let Err(err) = result {
                completion_backlog.unfill_completions(completions, filled_completions);

                if completion_fd_reenabled {
                    self.set_completion_fd_enabled(false);
                }
                return Err(err);
            }
        }

        if completion_fd_reenabled {
            self.set_completion_fd_enabled(false);
        }
        Ok(filled_completions)
    }
}

properties! {
    VIRTIO_BLK_PROPS: PropertyState for VirtioBlk.props {
        buf_alignment: i32,
        can_add_queues: bool,
        fn capacity: u64,
        discard_alignment: i32,
        discard_alignment_offset: i32,
        driver: str,
        mut fd: i32,
        max_discard_len: u64,
        fn max_queues: i32,
        max_queue_size: i32,
        fn max_mem_regions: u64,
        max_segment_len: i32,
        max_segments: i32,
        max_transfer: i32,
        max_write_zeroes_len: u64,
        may_pin_mem_regions: bool,
        mem_region_alignment: u64,
        needs_mem_regions: bool,
        needs_mem_region_fd: bool,
        mut num_queues: i32,
        mut num_poll_queues: i32,
        optimal_buf_alignment: i32,
        optimal_io_alignment: i32,
        optimal_io_size: i32,
        mut path: str,
        mut queue_size: i32,
        mut read_only: bool,
        request_alignment: i32,
        supports_fua_natively: bool,
        supports_poll_queues: bool,
        can_grow: bool,
        flush_needed: bool
    }
}

pub struct VirtioBlk {
    state: State,
    props: PropertyState,
    features: Option<VirtioBlkFeatureFlags>,
    transport: Option<Box<VirtioBlkTransport>>,
}

impl VirtioBlk {
    pub fn new(driver: &str) -> Self {
        VirtioBlk {
            props: PropertyState {
                buf_alignment: 1,
                can_add_queues: false,
                discard_alignment: 512,
                discard_alignment_offset: 0,
                driver: driver.to_string(),
                fd: -1,
                max_discard_len: 0,
                max_queue_size: MAX_QUEUE_SIZE,
                max_segment_len: 0,
                max_segments: 1,
                max_transfer: 0,
                max_write_zeroes_len: 0,
                may_pin_mem_regions: true,
                mem_region_alignment: 1,
                needs_mem_regions: true,
                needs_mem_region_fd: true,
                num_queues: 1,
                num_poll_queues: 0,
                optimal_buf_alignment: 1,
                optimal_io_alignment: 512,
                optimal_io_size: 0,
                queue_size: DEFAULT_QUEUE_SIZE,
                path: String::new(),
                read_only: false,
                request_alignment: 512,
                supports_fua_natively: false,
                supports_poll_queues: true,
                can_grow: false,
                flush_needed: true,
            },
            state: State::Created,
            features: None,
            transport: None,
        }
    }

    // FIXME Share this code with io_uring
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

        let cfg = self
            .transport
            .as_ref()
            .unwrap()
            .get_config()
            .map_err(|e| Error::from_io_error(e, Errno::IO))?;
        Ok(512 * u64::from(cfg.capacity))
    }

    fn get_max_queues(&self) -> Result<i32> {
        self.must_be_connected()?;

        let transport = self.transport.as_ref().unwrap();
        let max_queues = virtio_driver::virtio_blk_max_queues(&**transport)
            .map_err(|e| Error::from_io_error(e, Errno::IO))?;

        Ok(max_queues as i32)
    }

    fn get_max_mem_regions(&self) -> Result<u64> {
        self.must_be_connected()?;

        Ok(self.transport.as_ref().unwrap().max_mem_regions())
    }

    fn set_queue_size(&mut self, value: i32) -> Result<()> {
        self.must_be_connected()?;
        self.cant_set_while_started()?;

        if value <= 0 {
            return Err(Error::new(
                Errno::INVAL,
                "queue_size must be greater than 0",
            ));
        }
        if !(value as u32).is_power_of_two() {
            return Err(Error::new(
                Errno::INVAL,
                "queue_size must be a power of two",
            ));
        }
        if value > MAX_QUEUE_SIZE {
            return Err(Error::new(
                Errno::INVAL,
                format!("queue_size must be smaller than {}", MAX_QUEUE_SIZE),
            ));
        }

        self.props.queue_size = value;
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

    fn set_path(&mut self, value: &str) -> Result<()> {
        self.cant_set_while_connected()?;
        self.props.path = value.to_string();
        Ok(())
    }

    fn set_fd(&mut self, value: i32) -> Result<()> {
        self.cant_set_while_connected()?;

        match self.props.driver.as_str() {
            #[cfg(feature = "virtio-blk-vhost-vdpa")]
            VHOST_VDPA_DRIVER => {
                self.props.fd = value;
                Ok(())
            }
            _ => Err(Error::new(Errno::NOENT, "fd property not supported")),
        }
    }

    fn set_read_only(&mut self, value: bool) -> Result<()> {
        self.cant_set_while_connected()?;
        self.props.read_only = value;
        Ok(())
    }
}

impl Driver for VirtioBlk {
    fn state(&self) -> State {
        self.state
    }

    fn connect(&mut self) -> Result<()> {
        self.cant_set_while_connected()?;

        if !self.props.path.is_empty() {
            if self.props.fd != -1 {
                return Err(Error::new(
                    Errno::INVAL,
                    "path and fd cannot be set at the same time",
                ));
            }
        } else if self.props.fd == -1 {
            return Err(Error::new(Errno::INVAL, "One of path and fd must be set"));
        }

        let blk_features = VirtioBlkFeatureFlags::SIZE_MAX
            | VirtioBlkFeatureFlags::SEG_MAX
            | VirtioBlkFeatureFlags::RO
            | VirtioBlkFeatureFlags::BLK_SIZE
            | VirtioBlkFeatureFlags::FLUSH
            | VirtioBlkFeatureFlags::TOPOLOGY
            | VirtioBlkFeatureFlags::MQ
            | VirtioBlkFeatureFlags::DISCARD
            | VirtioBlkFeatureFlags::WRITE_ZEROES
            | VirtioBlkFeatureFlags::CONFIG_WCE;

        let features = blk_features.bits()
            | VirtioFeatureFlags::VERSION_1.bits()
            | VirtioFeatureFlags::RING_EVENT_IDX.bits();
        let transport: Box<VirtioBlkTransport> = match self.props.driver.as_str() {
            #[cfg(feature = "virtio-blk-vfio-pci")]
            VFIO_PCI_DRIVER => {
                if self.props.fd != -1 {
                    return Err(Error::new(
                        Errno::INVAL,
                        "virtio-blk-vfio-pci doesn't support the fd property",
                    ));
                }

                self.props.needs_mem_region_fd = false;

                let device = pci_driver::backends::vfio::VfioPciDevice::open(&self.props.path)
                    .map_err(|e| Error::from_io_error(e, Errno::IO))?;
                let transport = virtio_driver::Pci::new(Arc::new(device), features)
                    .map_err(|e| Error::from_io_error(e, Errno::IO))?;
                Box::new(transport)
            }
            #[cfg(feature = "virtio-blk-vhost-user")]
            VHOST_USER_DRIVER => {
                if self.props.fd != -1 {
                    return Err(Error::new(
                        Errno::INVAL,
                        "virtio-blk-vhost-user doesn't support the fd property",
                    ));
                }

                Box::new(
                    // We could set self.props.may_pin_mem_regions = false here, but a vhost-user-blk
                    // backend server could pin pages so we don't know for sure.
                    virtio_driver::VhostUser::new(&self.props.path, features)
                        .map_err(|e| Error::from_io_error(e, Errno::IO))?,
                )
            }
            #[cfg(feature = "virtio-blk-vhost-vdpa")]
            VHOST_VDPA_DRIVER => {
                let transport = if !self.props.path.is_empty() {
                    virtio_driver::VhostVdpa::with_path(&self.props.path, features).map_err(
                        |_e| Error::new(Errno::IO, "Failed to connect to vDPA device path"),
                    )?
                } else {
                    unsafe {
                        virtio_driver::VhostVdpa::with_fd(self.props.fd, features).map_err(
                            |_e| Error::new(Errno::IO, "Failed to use to vDPA device fd"),
                        )?
                    }
                };

                Box::new(transport)
            }
            _ => return Err(Error::new(Errno::NOENT, "Unknown driver name")),
        };

        self.props.mem_region_alignment = transport.mem_region_alignment() as u64;

        let features = VirtioBlkFeatureFlags::from_bits_truncate(transport.get_features());
        let cfg = transport
            .get_config()
            .map_err(|e| Error::from_io_error(e, Errno::IO))?;

        if features.contains(VirtioBlkFeatureFlags::DISCARD) {
            self.props.discard_alignment = 512 * cfg.discard_sector_alignment.to_native() as i32;
            self.props.max_discard_len = 512 * cfg.max_discard_sectors.to_native() as u64;
        }

        if features.contains(VirtioBlkFeatureFlags::WRITE_ZEROES) {
            self.props.max_write_zeroes_len = 512 * cfg.max_write_zeroes_sectors.to_native() as u64;
        }

        if features.contains(VirtioBlkFeatureFlags::SIZE_MAX) {
            self.props.max_segment_len = cfg.size_max.to_native().min(i32::MAX as u32) as i32;
        }

        if features.contains(VirtioBlkFeatureFlags::SEG_MAX) {
            self.props.max_segments = cfg.seg_max.to_native().min(i32::MAX as u32) as i32;
        }

        if features.contains(VirtioBlkFeatureFlags::SIZE_MAX | VirtioBlkFeatureFlags::SEG_MAX) {
            self.props.max_transfer = self.props.max_segment_len * self.props.max_segments;
        }

        let blk_size = if features.contains(VirtioBlkFeatureFlags::BLK_SIZE) {
            cfg.blk_size.to_native() as i32
        } else {
            512
        };

        self.props.flush_needed = if features.contains(VirtioBlkFeatureFlags::CONFIG_WCE) {
            cfg.writeback != 0
        } else {
            features.contains(VirtioBlkFeatureFlags::FLUSH)
        };

        self.props.request_alignment = blk_size;
        self.props.optimal_io_alignment = blk_size;

        if features.contains(VirtioBlkFeatureFlags::TOPOLOGY) {
            self.props.optimal_io_alignment = blk_size * 2i32.pow(cfg.physical_block_exp as u32);
            self.props.optimal_io_size = blk_size * cfg.opt_io_size.to_native() as i32;
            self.props.discard_alignment_offset = blk_size * cfg.alignment_offset as i32;
        }

        self.features = Some(features);
        self.transport = Some(transport);
        self.state = State::Connected;

        Ok(())
    }

    fn start(&mut self) -> Result<DriverStartOutcome> {
        self.must_be_connected()?;
        self.cant_set_while_started()?;

        let max_queues = self.get_max_queues()?;
        let total_num_queues = self.props.num_queues + self.props.num_poll_queues;

        if total_num_queues == 0 {
            return Err(Error::new(
                Errno::INVAL,
                "At least one of num_queues and num_poll_queues must be greater than 0",
            ));
        }

        if total_num_queues > max_queues {
            return Err(Error::new(
                Errno::INVAL,
                format!(
                    "num_queues + num_poll_queues must not be greater than {}",
                    max_queues
                ),
            ));
        }

        let transport = self.transport.as_mut().unwrap();
        let features = VirtioBlkFeatureFlags::from_bits_truncate(transport.get_features());

        if features.contains(VirtioBlkFeatureFlags::RO) && !self.props.read_only {
            return Err(Error::new(Errno::ROFS, "Device is read-only"));
        }

        let mut queues: Vec<_> = VirtioBlkQueue::setup_queues(
            &mut **transport,
            total_num_queues as usize,
            self.props.queue_size as u16,
        )
        .map_err(|e| Error::from_io_error(e, Errno::IO))?
        .into_iter()
        .enumerate()
        .collect();

        let poll_queues = queues.split_off(self.props.num_queues as usize);

        fn set_nonblock(eventfd: &EventFd) -> Result<()> {
            let status_flags = fcntl_getfl(eventfd)?;
            fcntl_setfl(eventfd, status_flags | OFlags::NONBLOCK)?;
            Ok(())
        }

        let create_queue = |i, q, poll| {
            let transport = self.transport.as_ref().unwrap();

            let submission_notifier = transport.get_submission_notifier(i);
            let completion_fd = transport.get_completion_fd(i);

            set_nonblock(&completion_fd)?;

            let queue = Queue::new(
                poll,
                self.features.unwrap(),
                q,
                submission_notifier,
                Arc::clone(&completion_fd),
                self.props.read_only,
            );

            Ok(Box::new(queue) as Box<dyn crate::Queue>)
        };

        let queues = queues
            .into_iter()
            .map(|(i, q)| create_queue(i, q, false))
            .collect::<Result<_>>()?;

        let poll_queues = poll_queues
            .into_iter()
            .map(|(i, q)| create_queue(i, q, true))
            .collect::<Result<_>>()?;

        self.state = State::Started;

        Ok(DriverStartOutcome {
            queues,
            poll_queues,
        })
    }

    fn map_mem_region(&mut self, region: &MemoryRegion) -> Result<()> {
        self.must_be_started()?;
        self.transport
            .as_mut()
            .unwrap()
            .map_mem_region(region.addr, region.len, region.fd, region.fd_offset)
            .map_err(|e| Error::from_io_error(e, Errno::IO))?;
        Ok(())
    }

    fn unmap_mem_region(&mut self, region: &MemoryRegion) {
        if self.state >= State::Started {
            let _ = self
                .transport
                .as_mut()
                .unwrap()
                .unmap_mem_region(region.addr, region.len);
        }
    }
}
