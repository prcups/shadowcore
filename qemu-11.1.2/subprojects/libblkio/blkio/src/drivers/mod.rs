// SPDX-License-Identifier: (MIT OR Apache-2.0)

#[cfg(feature = "io_uring")]
pub mod iouring;

#[cfg(feature = "nvme-io_uring")]
pub mod nvme_io_uring;

#[cfg(any(
    feature = "virtio-blk-vfio-pci",
    feature = "virtio-blk-vhost-user",
    feature = "virtio-blk-vhost-vdpa"
))]
pub mod virtio_blk;
