libblkio - block device I/O library
===================================

.. epigraph:: *Build high-performance storage applications quickly.*

libblkio provides an API for efficiently accessing block devices using modern
high-performance block I/O interfaces like Linux io_uring. Using libbklio
reduces the amount of code needed for interfacing with storage devices and lets
you focus on your application.

Here are some of the major features:

* **Drivers:** Linux io_uring, NVMe (io_uring cmd), virtio-blk (vhost-user, vhost-vdpa, and VFIO PCI)
* **Multi-queue** device support.
* **Blocking**, **event-driven**, and **polling** APIs to fit your application architecture.
* **Low overhead** comparable to custom code.
* **C API** accessible from most programming languages.
* Native **Rust API** for idiomatic code (experimental).

This library is licensed under either the MIT or Apache 2.0 license at your option.

Resources
=========

.. toctree::
   :maxdepth: 1

   API documentation <blkio>

- `GitLab project site <https://gitlab.com/libblkio/libblkio>`_
