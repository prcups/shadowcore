// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::io;

use crate::drivers::nvme_io_uring::nvme_ioctl::{nvme_passthru_cmd, nvme_uring_cmd};
use libc::{c_ulong, ioctl};

const IOC_NRBITS: c_ulong = 8;
const IOC_TYPEBITS: c_ulong = 8;
const IOC_SIZEBITS: c_ulong = 14;

const IOC_NRSHIFT: c_ulong = 0;
const IOC_TYPESHIFT: c_ulong = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: c_ulong = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: c_ulong = IOC_SIZESHIFT + IOC_SIZEBITS;

const IOC_NONE: c_ulong = 0;
const IOC_WRITE: c_ulong = 1;
const IOC_READ: c_ulong = 2;

const fn ioctl_cmd<T>(dir: c_ulong, ty: c_ulong, nr: c_ulong) -> c_ulong {
    (dir << IOC_DIRSHIFT)
        | (ty << IOC_TYPESHIFT)
        | (nr << IOC_NRSHIFT)
        | ((std::mem::size_of::<T>() as c_ulong) << IOC_SIZESHIFT)
}

fn ioctl_return_to_result(ret: i32) -> io::Result<i32> {
    if ret >= 0 {
        Ok(ret)
    } else {
        Err(io::Error::last_os_error())
    }
}

macro_rules! ioctl_none {
    ($name:ident, $ty:expr, $nr:expr) => {
        pub unsafe fn $name(fd: ::std::os::unix::io::RawFd) -> ::std::io::Result<i32> {
            const CMD: c_ulong = ioctl_cmd::<()>(IOC_NONE, $ty as c_ulong, $nr as c_ulong);
            let ret = unsafe { ioctl(fd, CMD) };
            ioctl_return_to_result(ret)
        }
    };
}

macro_rules! ioctl_readwrite {
    ($name:ident, $ty:expr, $nr:expr, $arg:ty) => {
        pub unsafe fn $name(
            fd: ::std::os::unix::io::RawFd,
            arg: *mut $arg,
        ) -> ::std::io::Result<i32> {
            const CMD: c_ulong =
                ioctl_cmd::<$arg>(IOC_READ | IOC_WRITE, $ty as c_ulong, $nr as c_ulong);
            let ret = unsafe { ioctl(fd, CMD, arg) };
            ioctl_return_to_result(ret)
        }
    };
}

ioctl_none!(nvme_ioctl_id, 'N', 0x40);
ioctl_readwrite!(nvme_ioctl_admin_cmd, 'N', 0x41, nvme_passthru_cmd);

pub const NVME_URING_CMD_IO: u32 =
    ioctl_cmd::<nvme_uring_cmd>(IOC_READ | IOC_WRITE, 'N' as c_ulong, 0x80) as u32;
pub const NVME_URING_CMD_IO_VEC: u32 =
    ioctl_cmd::<nvme_uring_cmd>(IOC_READ | IOC_WRITE, 'N' as c_ulong, 0x81) as u32;
