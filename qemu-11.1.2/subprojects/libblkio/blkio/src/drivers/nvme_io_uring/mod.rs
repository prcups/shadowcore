// SPDX-License-Identifier: (MIT OR Apache-2.0)

mod ioctl;

#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unsafe_op_in_unsafe_fn // override the crate-level `deny(unsafe_op_in_unsafe_fn)`
)]
mod nvme_ioctl;

use crate::drivers::nvme_io_uring::ioctl::{
    nvme_ioctl_admin_cmd, nvme_ioctl_id, NVME_URING_CMD_IO, NVME_URING_CMD_IO_VEC,
};
use crate::drivers::nvme_io_uring::nvme_ioctl::{nvme_passthru_cmd, nvme_uring_cmd};
use crate::properties::{properties, PropertiesList, Property};
use crate::wait::TimeoutUpdater;
use crate::{properties, DriverStartOutcome};
use crate::{
    Completion, CompletionBacklog, Driver, Error, MemoryRegion, Queue, ReqFlags, Request,
    RequestBacklog, RequestTypeArgs, Result, State,
};
use io_uring::opcode::UringCmd80;
use io_uring::types::{Fixed, SubmitArgs, Timespec};
use io_uring::{cqueue, squeue, IoUring};
use libc::{sigset_t, sysconf, _SC_IOV_MAX, _SC_PAGE_SIZE};
use rustix::cstr;
use rustix::fd::OwnedFd;
use rustix::io::{eventfd, Errno, EventfdFlags};
use std::convert::TryInto;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::{cmp, iter, mem, ptr, result};

const IORING_ENTER_GETEVENTS: u32 = 1;

const NVME_SC_INVALID_OPCODE: i32 = 0x01;
const NVME_SC_INVALID_FIELD: i32 = 0x02;

const NVME_SC_LBA_RANGE: i32 = 0x80;
const NVME_SC_CAP_EXCEEDED: i32 = 0x81;

fn nvme_error_status(status: i32) -> i32 {
    let type_and_code = status & 0x7ff;
    match type_and_code {
        NVME_SC_INVALID_OPCODE | NVME_SC_INVALID_FIELD => -libc::EOPNOTSUPP,
        NVME_SC_LBA_RANGE => -libc::EINVAL,
        NVME_SC_CAP_EXCEEDED => -libc::ENOSPC,
        _ => -libc::EIO,
    }
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum CommandSetIdentifier {
    Nvm = 0x00,
    Zoned = 0x02,
}

#[derive(Copy, Clone)]
#[repr(u8)]
enum Opcode {
    GetLogPage = 0x02,
    Identify = 0x06,
    GetFeatures = 0x0A,
}

#[derive(Copy, Clone)]
#[repr(u32)]
enum ControllerNamespaceStructure {
    IdentifyNamespace = 0x00,
    IdentifyController = 0x01,
    NsIdDescriptorList = 0x03,
    CmdSetIdentifyNamespace = 0x05,
    CmdSetIdentifyController = 0x06,
}

#[derive(Copy, Clone)]
#[repr(u32)]
enum FeatureIdentifier {
    VolatileWriteCache = 0x06,
}

#[derive(Copy, Clone)]
#[repr(u32)]
enum LogPageIdentifier {
    CommandsEffects = 0x05,
}

fn nvme_identify(
    namespace_char_dev: &File,
    namespace_id: u32,
    cns: ControllerNamespaceStructure,
    csi: CommandSetIdentifier,
) -> io::Result<[u8; 4096]> {
    let mut data = [0u8; 4096];

    let mut cmd = nvme_passthru_cmd {
        opcode: Opcode::Identify as u8,
        nsid: namespace_id,
        addr: data.as_mut_ptr() as u64,
        data_len: mem::size_of_val(&data) as u32,
        cdw10: cns as u32,
        cdw11: (csi as u32) << 24,
        ..Default::default()
    };

    unsafe { nvme_ioctl_admin_cmd(namespace_char_dev.as_raw_fd(), &mut cmd)? };

    Ok(data)
}

fn nvme_get_features(namespace_char_dev: &File, fid: FeatureIdentifier) -> io::Result<u32> {
    let mut cmd = nvme_passthru_cmd {
        opcode: Opcode::GetFeatures as u8,
        cdw10: fid as u32,
        ..Default::default()
    };

    unsafe { nvme_ioctl_admin_cmd(namespace_char_dev.as_raw_fd(), &mut cmd)? };

    Ok(cmd.result)
}

fn nvme_get_log_page(
    namespace_char_dev: &File,
    namespace_id: u32,
    lid: LogPageIdentifier,
    csi: CommandSetIdentifier,
) -> io::Result<[u8; 4096]> {
    let mut data = [0u8; 4096];
    let mut cmd = nvme_passthru_cmd {
        opcode: Opcode::GetLogPage as u8,
        nsid: namespace_id,
        addr: data.as_mut_ptr() as u64,
        data_len: mem::size_of_val(&data) as u32,
        cdw10: lid as u32 | (1024 << 16),
        cdw14: (csi as u32) << 24,
        ..Default::default()
    };

    unsafe { nvme_ioctl_admin_cmd(namespace_char_dev.as_raw_fd(), &mut cmd)? };

    Ok(data)
}

#[derive(Debug)]
struct NsIdDescriptor<'a> {
    namespace_identifier_type: u8,
    namespace_identifier: &'a [u8],
}

struct NsIdDescriptorIterator<'a> {
    descriptor_list: &'a [u8],
}

impl<'a> NsIdDescriptorIterator<'a> {
    fn new(descriptor_list: &'a [u8]) -> Self {
        Self { descriptor_list }
    }
}

impl<'a> Iterator for NsIdDescriptorIterator<'a> {
    type Item = NsIdDescriptor<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let namespace_identifier_type = self.descriptor_list[0];
        let namespace_identifier_length = self.descriptor_list[1];
        if namespace_identifier_length == 0 {
            return None;
        }
        let namespace_identifier =
            &self.descriptor_list[4..=(3 + namespace_identifier_length) as usize];
        self.descriptor_list = &self.descriptor_list[(4 + namespace_identifier_length) as usize..];
        Some(NsIdDescriptor {
            namespace_identifier_type,
            namespace_identifier,
        })
    }
}

struct ZonedInfoArgs<'a> {
    namespace_char_dev: &'a File,
    namespace_id: u32,
    size: u64,
    cns00_data: &'a [u8; 4096],
    cns03_data: &'a [u8; 4096],
    max_transfer_size: u64,
    dev_min_page_shift: u8,
}

struct ZonedInfo {
    zoned: i32,
    max_active_zones: i32,
    max_open_zones: i32,
    zone_size: u64,
    nr_zones: u64,
    append_info: Option<u64>,
}

impl ZonedInfo {
    fn build(build_args: ZonedInfoArgs) -> io::Result<ZonedInfo> {
        fn zoned(cns03_data: &[u8; 4096]) -> i32 {
            // Zone Models
            // 0 - none
            // 1 - host-aware
            // 2 - host-managed
            let mut descriptors = NsIdDescriptorIterator::new(cns03_data);
            let csi_descriptor = descriptors.find(|d| d.namespace_identifier_type == 0x04);
            let current_csi = match csi_descriptor {
                Some(descriptor) => descriptor.namespace_identifier[0],
                None => CommandSetIdentifier::Nvm as u8,
            };
            let zoned = current_csi == (CommandSetIdentifier::Zoned as u8);

            if zoned {
                2
            } else {
                0
            }
        }

        fn max_active_zones(zoned: i32, cns05_zoned_data: &[u8; 4096]) -> i32 {
            if zoned != 0 {
                i32::from_le_bytes(cns05_zoned_data[4..8].try_into().unwrap()) + 1
            } else {
                0
            }
        }

        fn max_open_zones(zoned: i32, cns05_zoned_data: &[u8; 4096]) -> i32 {
            if zoned != 0 {
                i32::from_le_bytes(cns05_zoned_data[8..12].try_into().unwrap()) + 1
            } else {
                0
            }
        }

        fn zone_size(zoned: i32, cns00_data: &[u8; 4096], cns05_zoned_data: &[u8; 4096]) -> u64 {
            if zoned != 0 {
                let zone_size_in_lb =
                    u64::from_le_bytes(cns05_zoned_data[2816..2824].try_into().unwrap());
                let flbas0_3 = cns00_data[26] & 0b00001111;
                let flbas5_6 = cns00_data[26] & 0b01100000;
                let nlbaf = cns00_data[25] + 1;
                let flbas = if nlbaf <= 16 {
                    flbas0_3
                } else {
                    flbas0_3 | (flbas5_6 >> 1)
                };
                let lba_data_struct =
                    &cns00_data[(128 + 4 * flbas) as usize..(132 + 4 * flbas) as usize];
                let lbads = lba_data_struct[2];

                (1 << lbads as u64) * zone_size_in_lb
            } else {
                0
            }
        }

        fn nr_zones(zoned: i32, size: u64, zone_size: u64) -> u64 {
            if zoned != 0 {
                size / zone_size
            } else {
                0
            }
        }

        fn append_info(
            zoned: i32,
            log_page: &[u8; 4096],
            cns06_zoned_data: &[u8; 4096],
            max_transfer_size: u64,
            dev_min_page_shift: u8,
        ) -> Option<u64> {
            if zoned == 0 {
                return None;
            }

            let io_cmd_supported_append = &log_page[1524..1528];
            let cmd_append_support = io_cmd_supported_append[0] & 1;
            let append_support = cmd_append_support != 0;
            let zone_append_max_bytes;

            if append_support {
                let zasl = 1 << (cns06_zoned_data[0] + dev_min_page_shift);
                zone_append_max_bytes = if zasl == 0 { max_transfer_size } else { zasl };
                Some(zone_append_max_bytes)
            } else {
                None
            }
        }

        let cns05_zoned_data = nvme_identify(
            build_args.namespace_char_dev,
            build_args.namespace_id,
            ControllerNamespaceStructure::CmdSetIdentifyNamespace,
            CommandSetIdentifier::Zoned,
        )?;
        let cns06_zoned_data = nvme_identify(
            build_args.namespace_char_dev,
            build_args.namespace_id,
            ControllerNamespaceStructure::CmdSetIdentifyController,
            CommandSetIdentifier::Zoned,
        )?;
        let log_page = nvme_get_log_page(
            build_args.namespace_char_dev,
            build_args.namespace_id,
            LogPageIdentifier::CommandsEffects,
            CommandSetIdentifier::Zoned,
        )?;

        let zoned = zoned(build_args.cns03_data);
        let max_active_zones = max_active_zones(zoned, &cns05_zoned_data);
        let max_open_zones = max_open_zones(zoned, &cns05_zoned_data);
        let zone_size = zone_size(zoned, build_args.cns00_data, &cns05_zoned_data);
        let nr_zones = nr_zones(zoned, build_args.size, zone_size);
        let append_info = append_info(
            zoned,
            &log_page,
            &cns06_zoned_data,
            build_args.max_transfer_size,
            build_args.dev_min_page_shift,
        );

        Ok(ZonedInfo {
            zoned,
            max_active_zones,
            max_open_zones,
            zone_size,
            nr_zones,
            append_info,
        })
    }
}

#[derive(Copy, Clone, Debug)]
struct NvmeNamespaceInfo {
    id: u32,
    size: u64,                 // in bytes
    block_size: u32,           // in bytes
    block_size_shift: u32,     // log2 of block_size
    max_read_write_len: u64,   // in bytes
    max_write_zeroes_len: u64, // in bytes; 0 iff unsupported
    max_discard_len: u64,      // in bytes; 0 iff unsupported
    flush_needed: bool,
    zoned: i32,
    max_open_zones: i32,
    max_active_zones: i32,
    zone_size: u64,
    nr_zones: u64,
    append_support: bool,
    zone_append_max_bytes: u64,
}

impl NvmeNamespaceInfo {
    pub fn from_file(namespace_char_dev: &File) -> io::Result<NvmeNamespaceInfo> {
        // TODO: Ensure namespace uses NVM command set.

        // get namespace id

        let namespace_id = unsafe { nvme_ioctl_id(namespace_char_dev.as_raw_fd())? } as u32;

        // submit Identify commands

        let cns00_data = nvme_identify(
            namespace_char_dev,
            namespace_id,
            ControllerNamespaceStructure::IdentifyNamespace,
            CommandSetIdentifier::Nvm,
        )?;
        let cns01_data = nvme_identify(
            namespace_char_dev,
            namespace_id,
            ControllerNamespaceStructure::IdentifyController,
            CommandSetIdentifier::Nvm,
        )?;
        let cns03_data = nvme_identify(
            namespace_char_dev,
            namespace_id,
            ControllerNamespaceStructure::NsIdDescriptorList,
            CommandSetIdentifier::Nvm,
        )?;
        let cns06_data = nvme_identify(
            namespace_char_dev,
            namespace_id,
            ControllerNamespaceStructure::CmdSetIdentifyController,
            CommandSetIdentifier::Nvm,
        )?;

        // interpret results

        let num_blocks = u64::from_le_bytes(cns00_data[0..8].try_into().unwrap());

        let block_size_shift = {
            let nlbaf = cns00_data[25];
            let flbas = cns00_data[26];

            let format_index = if nlbaf > 16 {
                ((flbas & 0b01100000) >> 1) | (flbas & 0b00001111)
            } else {
                flbas & 0b00001111
            };
            let format_offset = 128 + 4 * format_index as usize;
            let format = u32::from_le_bytes(
                cns00_data[format_offset..format_offset + 4]
                    .try_into()
                    .unwrap(),
            );

            let metadata_size = (format & 0x0000ffff) as u16;
            if metadata_size != 0 {
                return Err(io::Error::new(
                    ErrorKind::Other,
                    format!(
                        "Metadata Size (MS) is {}, expected 0 since the driver does not support per-LBA metadata",
                        metadata_size
                    ),
                ));
            }

            (format & 0x00ff0000) >> 16
        };
        if block_size_shift < 9 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "LBA Data Size (LBADS) is {}, expected 9 or greater",
                    block_size_shift
                ),
            ));
        }

        let block_size = 2u32.pow(block_size_shift);
        let size = block_size as u64 * num_blocks;

        // TODO: Figure out how to get CAP.MPSMIN so we can compute MDTS, to which this value should
        // be clamped.
        let max_read_write_len = (u16::MAX as u64 + 1) * block_size as u64;

        let oncs = u16::from_le_bytes(cns01_data[520..=521].try_into().unwrap());

        let max_write_zeroes_len = if oncs & (1 << 3) != 0 {
            max_read_write_len
        } else {
            let wzsl = cns06_data[1];
            if wzsl == 0 {
                0
            } else {
                // TODO: Figure out how to get CAP.MPSMIN so we can compute a proper value here.
                // 2u64.pow(wzsl.into()) * cap_mpsmin
                max_read_write_len
            }
        };

        let max_discard_len = if oncs & (1 << 2) != 0 {
            u32::MAX as u64 * block_size as u64
        } else {
            let dmrsl = u32::from_le_bytes(cns06_data[4..=7].try_into().unwrap());
            dmrsl as u64 * block_size as u64
        };

        let write_cache_present = cns01_data[525] & 1;
        let flush_needed = if write_cache_present != 0 {
            let fid_data =
                nvme_get_features(namespace_char_dev, FeatureIdentifier::VolatileWriteCache)?;
            let write_cache_set_to = fid_data & 1;
            write_cache_set_to != 0
        } else {
            false
        };

        // Maximum Data Transfer Size
        let dev_min_page_shift = 12;
        let mdts = cns01_data[77];
        let max_transfer_size = if mdts == 0 {
            0
        } else {
            1 << (mdts + dev_min_page_shift)
        };

        let zoned_info = ZonedInfo::build(ZonedInfoArgs {
            namespace_char_dev,
            namespace_id,
            size,
            cns00_data: &cns00_data,
            cns03_data: &cns03_data,
            max_transfer_size,
            dev_min_page_shift,
        })?;

        let zoned = zoned_info.zoned;
        let max_open_zones = zoned_info.max_open_zones;
        let max_active_zones = zoned_info.max_active_zones;
        let zone_size = zoned_info.zone_size;
        let nr_zones = zoned_info.nr_zones;
        let (append_support, zone_append_max_bytes) = match zoned_info.append_info {
            Some(max_bytes) => (true, max_bytes),
            None => (false, 0),
        };

        Ok(NvmeNamespaceInfo {
            id: namespace_id,
            size,
            block_size,
            block_size_shift,
            max_read_write_len,
            max_write_zeroes_len,
            max_discard_len,
            flush_needed,
            zoned,
            max_active_zones,
            max_open_zones,
            zone_size,
            nr_zones,
            append_support,
            zone_append_max_bytes,
        })
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct NvmeDatasetManagementRange {
    context_attributes: u32,
    length_in_logical_blocks: u32,
    starting_lba: u64,
}

#[derive(Clone, Copy)]
enum ReqContextData {
    NoData,
    Discard(NvmeDatasetManagementRange),
}

impl Default for ReqContextData {
    fn default() -> Self {
        Self::NoData
    }
}

#[derive(Clone, Copy, Default)]
struct ReqContext {
    user_data: usize,
    data: ReqContextData,
}

struct ReqContexts {
    all_slots: Box<[ReqContext]>,
    free_slots: Vec<usize>,
}

impl ReqContexts {
    fn new(capacity: u32) -> Self {
        Self {
            all_slots: iter::repeat(ReqContext::default())
                .take(capacity as usize)
                .collect(),
            free_slots: (0..capacity as usize).collect(),
        }
    }

    fn len(&self) -> usize {
        self.all_slots.len() - self.free_slots.len()
    }

    fn is_full(&self) -> bool {
        self.free_slots.is_empty()
    }

    fn get(&mut self) -> (u64, &mut ReqContext) {
        let id = self.free_slots.pop().unwrap();
        (id as u64, &mut self.all_slots[id])
    }

    fn put(&mut self, id: u64) -> ReqContext {
        let id = id.try_into().unwrap();
        self.free_slots.push(id);
        self.all_slots[id]
    }
}

struct NvmeIoUringQueue {
    namespace_info: NvmeNamespaceInfo,
    read_only: bool,
    ring: IoUring<squeue::Entry128, cqueue::Entry32>,
    eventfd: OwnedFd,
    in_flight_reqs: ReqContexts, // reqs enqueued to the SQ but whose CQEs have not been consumed
}

impl NvmeIoUringQueue {
    pub fn new(
        num_entries: u32,
        fd: RawFd,
        namespace_info: &NvmeNamespaceInfo,
        read_only: bool,
    ) -> Result<Self> {
        let ring = IoUring::builder()
            .build(num_entries)
            .map_err(|e| Error::from_io_error(e, Errno::NOMEM))?;

        ring.submitter()
            .register_files(&[fd])
            .map_err(|e| Error::from_io_error(e, Errno::NOTSUP))?;

        let eventfd = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;

        // We can use any size here, not just #SQEs + #CQEs as we're using now.
        let in_flight_reqs =
            ReqContexts::new(ring.params().sq_entries() + ring.params().cq_entries());

        // create NvmeIoUringQueue here so eventfd is closed on error
        let queue = NvmeIoUringQueue {
            namespace_info: *namespace_info,
            read_only,
            ring,
            eventfd,
            in_flight_reqs,
        };

        queue
            .ring
            .submitter()
            .register_eventfd(queue.eventfd.as_raw_fd())
            .map_err(|e| Error::from_io_error(e, Errno::NOTSUP))?;

        Ok(queue)
    }
}

fn prepare_req(
    namespace_info: &NvmeNamespaceInfo,
    read_only: bool,
    req: Request,
    context: &mut ReqContext,
    context_id: u64,
) -> result::Result<squeue::Entry128, Completion> {
    let validate_start_and_len = |start: u64, len: u64, write: bool| {
        if write && read_only {
            Some(Completion::for_failed_req(
                &req,
                Errno::BADF,
                cstr!("driver is in read-only mode"),
            ))
        } else if start & (namespace_info.block_size - 1) as u64 != 0
            || len & (namespace_info.block_size - 1) as u64 != 0
        {
            Some(Completion::for_failed_req(
                &req,
                Errno::INVAL,
                cstr!("start and len must be multiples of property \"request-alignment\""),
            ))
        } else if len == 0 {
            Some(Completion::for_failed_req(
                &req,
                Errno::INVAL,
                cstr!("len must be positive"),
            ))
        } else {
            None
        }
    };

    let validate_rw_start_and_len = |start: u64, len: usize, write: bool| {
        if let Some(c) = validate_start_and_len(start, len as u64, write) {
            Some(c)
        } else if len as u64 > namespace_info.max_read_write_len {
            Some(Completion::for_failed_req(
                &req,
                Errno::INVAL,
                cstr!("len must not exceed property \"max-transfer\""),
            ))
        } else {
            None
        }
    };

    let cmd_op: u32;
    let opcode: u8;
    let data_addr: u64;
    let data_len: u32;
    let cdw10: u32;
    let cdw11: u32;
    let cdw12: u32;

    match req.args {
        RequestTypeArgs::Read { start, buf, len } => {
            if let Some(c) = validate_rw_start_and_len(start, len, false) {
                return Err(c);
            }

            let lba = start >> namespace_info.block_size_shift;
            let num_blocks = (len >> namespace_info.block_size_shift) as u32;

            cmd_op = NVME_URING_CMD_IO;
            opcode = 0x02;
            data_addr = buf as u64;
            data_len = len as u32;
            cdw10 = (lba & 0xffffffff) as u32;
            cdw11 = (lba >> 32) as u32;
            cdw12 = num_blocks - 1;
        }
        RequestTypeArgs::Write { start, buf, len } => {
            if let Some(c) = validate_rw_start_and_len(start, len, true) {
                return Err(c);
            }

            let lba = start >> namespace_info.block_size_shift;
            let num_blocks = (len >> namespace_info.block_size_shift) as u32;

            let fua = req.flags.contains(ReqFlags::FUA);

            cmd_op = NVME_URING_CMD_IO;
            opcode = 0x01;
            data_addr = buf as u64;
            data_len = len as u32;
            cdw10 = (lba & 0xffffffff) as u32;
            cdw11 = (lba >> 32) as u32;
            cdw12 = (if fua { 1 << 30 } else { 0 }) | (num_blocks - 1);
        }
        RequestTypeArgs::Readv { start, ref iovec } => {
            let len = unsafe { iovec.buffer_size() };

            if let Some(c) = validate_rw_start_and_len(start, len, false) {
                return Err(c);
            }

            let lba = start >> namespace_info.block_size_shift;
            let num_blocks = (len >> namespace_info.block_size_shift) as u32;

            cmd_op = NVME_URING_CMD_IO_VEC;
            opcode = 0x02;
            data_addr = iovec.as_ptr() as u64;
            data_len = iovec.len();
            cdw10 = (lba & 0xffffffff) as u32;
            cdw11 = (lba >> 32) as u32;
            cdw12 = num_blocks - 1;
        }
        RequestTypeArgs::Writev { start, ref iovec } => {
            let len = unsafe { iovec.buffer_size() };

            if let Some(c) = validate_rw_start_and_len(start, len, true) {
                return Err(c);
            }

            let lba = start >> namespace_info.block_size_shift;
            let num_blocks = (len >> namespace_info.block_size_shift) as u32;

            let fua = req.flags.contains(ReqFlags::FUA);

            cmd_op = NVME_URING_CMD_IO_VEC;
            opcode = 0x01;
            data_addr = iovec.as_ptr() as u64;
            data_len = iovec.len();
            cdw10 = (lba & 0xffffffff) as u32;
            cdw11 = (lba >> 32) as u32;
            cdw12 = (if fua { 1 << 30 } else { 0 }) | (num_blocks - 1);
        }
        RequestTypeArgs::WriteZeroes { start, len } => {
            if namespace_info.max_write_zeroes_len == 0 {
                return Err(Completion::for_failed_req(
                    &req,
                    Errno::NOTSUP,
                    cstr!("write zeroes not supported"),
                ));
            } else if let Some(c) = validate_start_and_len(start, len, true) {
                return Err(c);
            } else if len > namespace_info.max_write_zeroes_len {
                return Err(Completion::for_failed_req(
                    &req,
                    Errno::INVAL,
                    cstr!("len must not exceed property \"max-write-zeroes-len\""),
                ));
            }

            let lba = start >> namespace_info.block_size_shift;
            let num_blocks = (len >> namespace_info.block_size_shift) as u32;

            let fua = req.flags.contains(ReqFlags::FUA);
            let no_unmap = req.flags.contains(ReqFlags::NO_UNMAP);

            cmd_op = NVME_URING_CMD_IO;
            opcode = 0x08;
            data_addr = 0;
            data_len = 0;
            cdw10 = (lba & 0xffffffff) as u32;
            cdw11 = (lba >> 32) as u32;
            cdw12 = (if fua { 1 << 30 } else { 0 })
                | (if no_unmap { 0 } else { 1 << 25 })
                | (num_blocks - 1);
        }
        RequestTypeArgs::Discard { start, len } => {
            if namespace_info.max_discard_len == 0 {
                return Err(Completion::for_failed_req(
                    &req,
                    Errno::NOTSUP,
                    cstr!("discard not supported"),
                ));
            } else if let Some(c) = validate_start_and_len(start, len, true) {
                return Err(c);
            } else if len > namespace_info.max_discard_len {
                return Err(Completion::for_failed_req(
                    &req,
                    Errno::INVAL,
                    cstr!("len must not exceed property \"max-discard-len\""),
                ));
            }

            let lba = start >> namespace_info.block_size_shift;
            let num_blocks = (len >> namespace_info.block_size_shift) as u32;

            context.data = ReqContextData::Discard(NvmeDatasetManagementRange {
                context_attributes: u32::to_le(0),
                length_in_logical_blocks: u32::to_le(num_blocks),
                starting_lba: u64::to_le(lba),
            });

            data_addr = match context.data {
                ReqContextData::Discard(ref dataset_mgmt_range) => {
                    dataset_mgmt_range as *const _ as u64
                }
                _ => unreachable!("The request is always discard"),
            };

            cmd_op = NVME_URING_CMD_IO;
            opcode = 0x09;
            data_len = 16;
            cdw10 = 0; // 1 discard range
            cdw11 = 1 << 2; // deallocate
            cdw12 = 0; // unused
        }
        RequestTypeArgs::Flush => {
            // TODO: Probably complete the request without submitting if the device does not have a
            // write-back cache.

            cmd_op = NVME_URING_CMD_IO;
            opcode = 0x00;
            data_addr = 0;
            data_len = 0;
            cdw10 = 0; // unused
            cdw11 = 0; // unused
            cdw12 = 0; // unused
        }
    }

    let cmd = nvme_uring_cmd {
        opcode,
        nsid: namespace_info.id,
        addr: data_addr,
        data_len,
        cdw10,
        cdw11,
        cdw12,
        ..Default::default()
    };

    let mut cmd_bytes = [0u8; 80];
    unsafe {
        cmd_bytes
            .as_mut_ptr()
            .cast::<nvme_uring_cmd>()
            .write_unaligned(cmd);
    }

    Ok(UringCmd80::new(Fixed(0), cmd_op)
        .cmd(cmd_bytes)
        .build()
        .user_data(context_id))
}

impl Queue for NvmeIoUringQueue {
    fn is_poll_queue(&self) -> bool {
        false
    }

    fn get_completion_fd(&self) -> Option<RawFd> {
        Some(self.eventfd.as_raw_fd())
    }

    fn set_completion_fd_enabled(&mut self, _enabled: bool) {
        // TODO: Set/unset IORING_CQ_EVENTFD_DISABLED. The io-uring crate
        // doesn't support this yet.
    }

    fn try_enqueue(
        &mut self,
        completion_backlog: &mut CompletionBacklog,
        req: Request,
    ) -> result::Result<(), Request> {
        if self.ring.submission().is_full() || self.in_flight_reqs.is_full() {
            return Err(req);
        }

        let (context_id, context) = self.in_flight_reqs.get();
        context.user_data = req.user_data;

        let result = prepare_req(
            &self.namespace_info,
            self.read_only,
            req,
            context,
            context_id,
        );

        match result {
            Ok(entry) => {
                unsafe { self.ring.submission().push(&entry) }.unwrap();
            }
            Err(completion) => {
                completion_backlog.push(completion);
                self.in_flight_reqs.put(context_id);
            }
        };

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
        if min_completions
            > request_backlog.len() + self.in_flight_reqs.len() + completion_backlog.len()
        {
            return Err(Error::new(
                Errno::INVAL,
                "min_completions is larger than total outstanding requests",
            ));
        }

        // filled_completions tracks how many elements of completions[] have been filled in
        let mut filled_completions = completion_backlog.fill_completions(completions);

        // Fill completions[] from the cq ring and return the count
        fn drain_cqueue(
            q: &mut NvmeIoUringQueue,
            completions: &mut [std::mem::MaybeUninit<Completion>],
        ) -> usize {
            let mut cqueue = q.ring.completion();
            let mut i = 0;
            while i < completions.len() {
                if let Some(cqe) = cqueue.next() {
                    let context = q.in_flight_reqs.put(cqe.user_data());

                    let io_uring_ret: i32 = cqe.result();
                    let nvme_result: i32 = cqe.big_cqe()[1].try_into().unwrap();

                    let ret = if io_uring_ret == 0 && nvme_result == 0 {
                        0
                    } else if io_uring_ret > 0 {
                        nvme_error_status(io_uring_ret)
                    } else if io_uring_ret < 0 {
                        io_uring_ret
                    } else {
                        // TODO: Possibly interpret nvme_result.
                        -libc::EIO
                    };

                    let completion = Completion {
                        user_data: context.user_data,
                        ret,
                        error_msg: ptr::null(),
                        reserved_: [0; 12],
                    };
                    unsafe { completions[i].as_mut_ptr().write(completion) };
                    i += 1;
                } else {
                    break;
                }
            }
            i
        }

        let n = drain_cqueue(self, &mut completions[filled_completions..]);
        filled_completions += n;

        if n > 0 {
            request_backlog.process(self, completion_backlog);
        }

        let mut to_submit = self.ring.submission().len();

        while filled_completions < min_completions || to_submit > 0 {
            let min_complete = if filled_completions < min_completions {
                // Clamp to number of in-flight requests to avoid hangs when the user provides a
                // min_completions number that is too large.
                std::cmp::min(
                    min_completions - filled_completions,
                    self.in_flight_reqs.len(),
                )
            } else {
                0
            };

            let result = if let Some(timeout) = timeout_updater.as_mut().map(|t| t.next()) {
                let ts = Timespec::new()
                    .sec(timeout.as_secs())
                    .nsec(timeout.subsec_nanos());

                let mut submit_args = SubmitArgs::new().timespec(&ts);
                if let Some(s) = sig {
                    submit_args = submit_args.sigmask(s);
                }

                self.ring
                    .submitter()
                    .submit_with_args(min_complete, &submit_args)
                    .map_err(|e| Error::from_io_error(e, Errno::INVAL))
            } else {
                let flags = if min_complete > 0 {
                    IORING_ENTER_GETEVENTS
                } else {
                    0
                };

                unsafe {
                    self.ring
                        .submitter()
                        .enter(to_submit as u32, min_complete as u32, flags, sig)
                        .map_err(|e| Error::from_io_error(e, Errno::INVAL))
                }
            };

            let num_submitted = match result {
                Ok(n) => n,
                // TODO document EAGAIN/EBUSY or try again with to_submit=0 just to reap
                // completions and wait for enough resources to submit again?
                Err(err) => {
                    completion_backlog.unfill_completions(completions, filled_completions);
                    return Err(err);
                }
            };

            let n = drain_cqueue(self, &mut completions[filled_completions..]);
            filled_completions += n;

            if num_submitted > 0 || n > 0 {
                request_backlog.process(self, completion_backlog);
            }

            to_submit = self.ring.submission().len();
        }

        Ok(filled_completions)
    }
}

properties! {
    NVME_IO_URING_PROPS: PropertyState for NvmeIoUring.props {
        fn buf_alignment: i32,
        can_add_queues: bool,
        fn capacity: u64,
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
        flush_needed: bool,
        zoned: i32,
        max_active_zones: i32,
        max_open_zones: i32,
        zone_size: u64,
        nr_zones: u64,
        append_support: bool,
        zone_append_max_bytes: u64
    }
}

pub struct NvmeIoUring {
    props: PropertyState,
    file: Option<File>,
    namespace_info: Option<NvmeNamespaceInfo>,
    state: State,
}

impl NvmeIoUring {
    pub fn new() -> Self {
        NvmeIoUring {
            props: PropertyState {
                can_add_queues: true,
                driver: "nvme-io_uring".to_string(),
                fd: -1,
                max_queues: i32::MAX,
                max_mem_regions: u64::MAX,
                may_pin_mem_regions: false,
                needs_mem_regions: false,
                needs_mem_region_fd: false,
                num_entries: 128,
                num_queues: 1,
                num_poll_queues: 0,
                path: String::new(),
                read_only: false,
                supports_fua_natively: true,
                supports_poll_queues: false,
                can_grow: false,
                flush_needed: true,
                zoned: 0,
                max_open_zones: 0,
                max_active_zones: 0,
                zone_size: 0,
                nr_zones: 0,
                append_support: false,
                zone_append_max_bytes: 0,
            },
            file: None,
            namespace_info: None,
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
        Ok(self.namespace_info.as_ref().unwrap().size)
    }

    fn set_fd(&mut self, value: i32) -> Result<()> {
        self.cant_set_while_connected()?;
        self.props.fd = value;
        Ok(())
    }

    fn open_file(&mut self) -> Result<()> {
        if !self.props.path.is_empty() {
            if self.props.fd != -1 {
                return Err(Error::new(
                    Errno::INVAL,
                    "path and fd cannot be set at the same time",
                ));
            }

            let file = OpenOptions::new()
                .read(true)
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

        if !file_type.is_char_device() {
            return Err(Error::new(
                Errno::INVAL,
                "The file must be a character device",
            ));
        }

        let namespace_info = NvmeNamespaceInfo::from_file(&file)
            .map_err(|e| Error::from_io_error(e, Errno::INVAL))?;

        self.props.flush_needed = namespace_info.flush_needed;

        self.props.zoned = namespace_info.zoned;
        self.props.max_active_zones = namespace_info.max_active_zones;
        self.props.max_open_zones = namespace_info.max_open_zones;
        self.props.zone_size = namespace_info.zone_size;
        self.props.append_support = namespace_info.append_support;
        self.props.nr_zones = namespace_info.nr_zones;
        self.props.zone_append_max_bytes = namespace_info.zone_append_max_bytes;

        self.file = Some(file);
        self.namespace_info = Some(namespace_info);

        Ok(())
    }

    fn get_max_segment_len(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(0)
    }

    fn get_max_segments(&self) -> Result<i32> {
        self.must_be_connected()?;
        let iov_max = unsafe { sysconf(_SC_IOV_MAX) };
        assert!(iov_max >= 0);
        Ok(iov_max as i32)
    }

    fn get_max_transfer(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(self
            .namespace_info
            .as_ref()
            .unwrap()
            .max_read_write_len
            .try_into()
            .unwrap())
    }

    fn get_max_write_zeroes_len(&self) -> Result<u64> {
        self.must_be_connected()?;
        Ok(self.namespace_info.as_ref().unwrap().max_write_zeroes_len)
    }

    fn get_max_discard_len(&self) -> Result<u64> {
        self.must_be_connected()?;
        Ok(self.namespace_info.as_ref().unwrap().max_discard_len)
    }

    fn get_mem_region_alignment(&self) -> Result<u64> {
        Ok(self.get_buf_alignment()?.try_into().unwrap())
    }

    fn get_buf_alignment(&self) -> Result<i32> {
        self.get_request_alignment()
    }

    fn set_num_entries(&mut self, value: i32) -> Result<()> {
        self.must_be_connected()?;
        self.cant_set_while_started()?;

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
                "num-queues must be equal to or greater than 0",
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
        self.get_request_alignment() // TODO: Provide a useful value.
    }

    fn get_optimal_io_size(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(0) // TODO: Provide a useful value.
    }

    fn get_optimal_buf_alignment(&self) -> Result<i32> {
        self.must_be_connected()?;
        let page_size = unsafe { sysconf(_SC_PAGE_SIZE) };
        assert!(page_size >= 0);
        let request_alignment = self.get_request_alignment()?;
        Ok(cmp::max(page_size as i32, request_alignment))
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
        Ok(self.namespace_info.as_ref().unwrap().block_size as i32)
    }

    fn get_discard_alignment(&self) -> Result<i32> {
        self.get_request_alignment()
    }

    fn get_discard_alignment_offset(&self) -> Result<i32> {
        self.must_be_connected()?;
        Ok(0)
    }
}

impl Driver for NvmeIoUring {
    fn state(&self) -> State {
        self.state
    }

    fn connect(&mut self) -> Result<()> {
        self.cant_set_while_connected()?;

        // TODO: Can an NVMe NVM namespace be read-only? If so, require self.props.read_only to be
        // true when the device is read-only.

        self.open_file()?;
        self.state = State::Connected;
        Ok(())
    }

    fn start(&mut self) -> Result<DriverStartOutcome> {
        self.must_be_connected()?;
        self.cant_set_while_started()?;

        if self.props.num_poll_queues > 0 {
            return Err(Error::new(Errno::INVAL, "num_poll_queues must be 0"));
        }

        let create_queue = || {
            let q = NvmeIoUringQueue::new(
                self.props.num_entries as u32,
                self.props.fd,
                self.namespace_info.as_ref().unwrap(),
                self.props.read_only,
            )?;
            Ok(Box::new(q) as Box<dyn Queue>)
        };

        let queues = iter::repeat_with(create_queue)
            .take(self.props.num_queues as usize)
            .collect::<Result<_>>()?;

        self.state = State::Started;

        Ok(DriverStartOutcome {
            queues,
            poll_queues: Vec::new(),
        })
    }

    fn add_queue(&mut self, poll_queue: bool) -> Result<Box<dyn Queue>> {
        self.must_be_started()?;

        if poll_queue {
            return Err(Error::new(Errno::INVAL, "poll queues not supported"));
        }

        let q = NvmeIoUringQueue::new(
            self.props.num_entries as u32,
            self.props.fd,
            self.namespace_info.as_ref().unwrap(),
            self.props.read_only,
        )?;

        Ok(Box::new(q))
    }

    // IORING_REGISTER_BUFFERS could be used in the future to improve performance. Ignore
    // memory regions for now.
    fn map_mem_region(&mut self, _region: &MemoryRegion) -> Result<()> {
        self.must_be_started()
    }

    fn unmap_mem_region(&mut self, _region: &MemoryRegion) {}
}
